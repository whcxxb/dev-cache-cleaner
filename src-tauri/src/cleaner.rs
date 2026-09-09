use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeSet,
    env, fs, io,
    path::{Path, PathBuf},
    process::Command,
    time::{SystemTime, UNIX_EPOCH},
};
use tauri::Manager;

const CATEGORY_PACKAGE: &str = "package";
const CATEGORY_BUILD: &str = "build";
const CATEGORY_TOOL: &str = "tool";

#[derive(Clone, Debug)]
enum CleanupMode {
    RemoveContents,
    RemoveRoots,
    PnpmCache,
    PnpmStorePrune,
}

#[derive(Clone, Debug)]
struct TargetSpec {
    id: String,
    name: String,
    category: &'static str,
    description: String,
    cleanup_note: String,
    paths: Vec<PathBuf>,
    process_terms: Vec<String>,
    mode: CleanupMode,
}

impl TargetSpec {
    fn new(
        id: impl Into<String>,
        name: impl Into<String>,
        category: &'static str,
        description: impl Into<String>,
        cleanup_note: impl Into<String>,
        paths: Vec<PathBuf>,
        process_terms: Vec<String>,
        mode: CleanupMode,
    ) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            category,
            description: description.into(),
            cleanup_note: cleanup_note.into(),
            paths,
            process_terms,
            mode,
        }
    }
}

#[derive(Clone, Debug)]
struct ProcessInfo {
    pid: u32,
    command: String,
}

#[derive(Clone, Debug)]
struct OpenFile {
    pid: u32,
    command: String,
    path: String,
}

#[derive(Clone, Debug, Default)]
struct SystemSnapshot {
    processes: Vec<ProcessInfo>,
    open_files: Vec<OpenFile>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CacheItem {
    id: String,
    name: String,
    category: String,
    description: String,
    cleanup_note: String,
    paths: Vec<String>,
    size_bytes: u64,
    state: String,
    can_clean: bool,
    blockers: Vec<String>,
    scan_error: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanResult {
    items: Vec<CacheItem>,
    total_bytes: u64,
    reclaimable_bytes: u64,
    scanned_at: u64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CleanResult {
    id: String,
    before_bytes: u64,
    remaining_bytes: u64,
    freed_bytes: u64,
    skipped_entries: Vec<String>,
    message: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CleanupHistoryEntry {
    id: String,
    target_name: String,
    status: String,
    freed_bytes: u64,
    skipped_entries: Vec<String>,
    message: String,
    created_at: u64,
}

#[tauri::command]
pub async fn scan_cache_targets() -> Result<ScanResult, String> {
    tauri::async_runtime::spawn_blocking(scan_cache_targets_sync)
        .await
        .map_err(|error| format!("扫描任务异常：{error}"))?
}

#[tauri::command]
pub async fn clean_cache_target(app: tauri::AppHandle, id: String) -> Result<CleanResult, String> {
    let history_dir = app.path().app_data_dir().ok();
    tauri::async_runtime::spawn_blocking(move || {
        let target_name = target_name_for(&id);
        let result = clean_cache_target_sync(&id);
        if let Some(history_dir) = history_dir {
            let entry = history_entry_from_result(&id, target_name, &result);
            if let Err(error) = append_cleanup_history(&history_dir, entry) {
                eprintln!("无法写入清理历史：{error}");
            }
        }
        result
    })
    .await
    .map_err(|error| format!("清理任务异常：{error}"))?
}

#[tauri::command]
pub async fn get_cleanup_history(
    app: tauri::AppHandle,
) -> Result<Vec<CleanupHistoryEntry>, String> {
    let history_dir = app
        .path()
        .app_data_dir()
        .map_err(|error| format!("无法定位应用数据目录：{error}"))?;
    tauri::async_runtime::spawn_blocking(move || read_cleanup_history(&history_dir))
        .await
        .map_err(|error| format!("读取清理历史异常：{error}"))?
}

#[tauri::command]
pub async fn clear_cleanup_history(app: tauri::AppHandle) -> Result<(), String> {
    let history_dir = app
        .path()
        .app_data_dir()
        .map_err(|error| format!("无法定位应用数据目录：{error}"))?;
    tauri::async_runtime::spawn_blocking(move || clear_cleanup_history_sync(&history_dir))
        .await
        .map_err(|error| format!("清空清理历史异常：{error}"))?
}

fn scan_cache_targets_sync() -> Result<ScanResult, String> {
    let specs = target_specs()?;
    let snapshot = SystemSnapshot::capture();
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(4)
        .build()
        .map_err(|error| format!("无法创建扫描线程池：{error}"))?;

    let mut items = pool.install(|| {
        specs
            .par_iter()
            .map(|spec| scan_one(spec, &snapshot))
            .collect::<Vec<_>>()
    });

    items.sort_by_key(|item| item.name.clone());
    let total_bytes = items.iter().map(|item| item.size_bytes).sum();
    let reclaimable_bytes = items
        .iter()
        .filter(|item| item.can_clean)
        .map(|item| item.size_bytes)
        .sum();

    Ok(ScanResult {
        items,
        total_bytes,
        reclaimable_bytes,
        scanned_at: now_timestamp(),
    })
}

fn clean_cache_target_sync(id: &str) -> Result<CleanResult, String> {
    let spec = target_specs()?
        .into_iter()
        .find(|spec| spec.id == id)
        .ok_or_else(|| "未知清理目标，操作已拒绝".to_string())?;

    let snapshot = SystemSnapshot::capture();
    let current = scan_one(&spec, &snapshot);

    if current.state == "inUse" {
        let detail = if current.blockers.is_empty() {
            "相关进程".to_string()
        } else {
            current.blockers.join("、")
        };
        return Err(format!("{} 正在使用中，请先退出：{}", spec.name, detail));
    }

    if current.state == "unavailable" {
        return Err(current
            .scan_error
            .unwrap_or_else(|| "当前环境不支持此清理操作".to_string()));
    }

    let before_bytes = current.size_bytes;
    if before_bytes == 0 {
        return Ok(CleanResult {
            id: spec.id.to_string(),
            before_bytes,
            remaining_bytes: 0,
            freed_bytes: 0,
            skipped_entries: Vec::new(),
            message: "当前没有可清理内容".to_string(),
        });
    }

    let skipped_entries = match spec.mode {
        CleanupMode::RemoveContents => {
            for path in &spec.paths {
                remove_contents(path, path)?;
            }
            Vec::new()
        }
        CleanupMode::RemoveRoots => {
            for path in &spec.paths {
                remove_entry(path, path)?;
            }
            Vec::new()
        }
        CleanupMode::PnpmCache => clean_pnpm_cache(&spec.paths[0], &snapshot)?,
        CleanupMode::PnpmStorePrune => {
            run_pnpm_store_prune()?;
            Vec::new()
        }
    };

    let remaining_bytes = spec
        .paths
        .iter()
        .map(|path| directory_size(path).unwrap_or(0))
        .sum();
    let freed_bytes = before_bytes.saturating_sub(remaining_bytes);
    let message = if skipped_entries.is_empty() {
        "清理完成".to_string()
    } else {
        format!(
            "清理完成，已保留 {} 个正在使用的条目",
            skipped_entries.len()
        )
    };

    Ok(CleanResult {
        id: spec.id.to_string(),
        before_bytes,
        remaining_bytes,
        freed_bytes,
        skipped_entries,
        message,
    })
}

fn target_name_for(id: &str) -> String {
    target_specs()
        .ok()
        .and_then(|specs| {
            specs
                .into_iter()
                .find(|spec| spec.id == id)
                .map(|spec| spec.name.to_string())
        })
        .unwrap_or_else(|| id.to_string())
}

fn history_entry_from_result(
    id: &str,
    target_name: String,
    result: &Result<CleanResult, String>,
) -> CleanupHistoryEntry {
    match result {
        Ok(result) => CleanupHistoryEntry {
            id: id.to_string(),
            target_name,
            status: "success".to_string(),
            freed_bytes: result.freed_bytes,
            skipped_entries: result.skipped_entries.clone(),
            message: result.message.clone(),
            created_at: now_timestamp(),
        },
        Err(message) => CleanupHistoryEntry {
            id: id.to_string(),
            target_name,
            status: "failed".to_string(),
            freed_bytes: 0,
            skipped_entries: Vec::new(),
            message: message.clone(),
            created_at: now_timestamp(),
        },
    }
}

fn history_file(history_dir: &Path) -> PathBuf {
    history_dir.join("cleanup-history.json")
}

fn read_cleanup_history(history_dir: &Path) -> Result<Vec<CleanupHistoryEntry>, String> {
    let path = history_file(history_dir);
    match fs::read(&path) {
        Ok(contents) => serde_json::from_slice(&contents)
            .map_err(|error| format!("清理历史文件格式无效：{error}")),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(error) => Err(io_error("读取清理历史", error)),
    }
}

fn append_cleanup_history(history_dir: &Path, entry: CleanupHistoryEntry) -> Result<(), String> {
    fs::create_dir_all(history_dir).map_err(|error| io_error("创建应用数据目录", error))?;
    let path = history_file(history_dir);
    let mut history = read_cleanup_history(history_dir)?;
    history.insert(0, entry);
    history.truncate(100);

    let contents = serde_json::to_vec_pretty(&history)
        .map_err(|error| format!("序列化清理历史失败：{error}"))?;
    let temporary_path = history_dir.join("cleanup-history.tmp");
    fs::write(&temporary_path, contents).map_err(|error| io_error("写入清理历史", error))?;
    fs::rename(&temporary_path, &path).map_err(|error| io_error("保存清理历史", error))
}

fn clear_cleanup_history_sync(history_dir: &Path) -> Result<(), String> {
    let path = history_file(history_dir);
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(io_error("清空清理历史", error)),
    }
}

fn scan_one(spec: &TargetSpec, snapshot: &SystemSnapshot) -> CacheItem {
    let exists = spec.paths.iter().any(|path| path.exists());
    let mut scan_errors = Vec::new();
    let size_bytes = spec
        .paths
        .iter()
        .map(|path| match directory_size(path) {
            Ok(size) => size,
            Err(error) => {
                scan_errors.push(error);
                0
            }
        })
        .sum();

    let (usage_state, blockers) = usage_for(spec, snapshot);
    let tool_missing =
        matches!(spec.mode, CleanupMode::PnpmStorePrune) && find_pnpm_executable().is_none();

    let state = if tool_missing || (!scan_errors.is_empty() && size_bytes == 0) {
        "unavailable"
    } else if !exists {
        "missing"
    } else {
        usage_state
    };

    let can_clean = size_bytes > 0 && matches!(state, "ready" | "partial");
    let scan_error = if tool_missing {
        Some("未找到 pnpm，无法执行安全裁剪".to_string())
    } else if scan_errors.is_empty() {
        None
    } else {
        Some(scan_errors.join("；"))
    };

    CacheItem {
        id: spec.id.to_string(),
        name: spec.name.to_string(),
        category: spec.category.to_string(),
        description: spec.description.to_string(),
        cleanup_note: spec.cleanup_note.to_string(),
        paths: spec.paths.iter().map(|path| display_path(path)).collect(),
        size_bytes,
        state: state.to_string(),
        can_clean,
        blockers,
        scan_error,
    }
}

fn usage_for(spec: &TargetSpec, snapshot: &SystemSnapshot) -> (&'static str, Vec<String>) {
    let mut blockers = BTreeSet::new();
    let path_strings = spec
        .paths
        .iter()
        .map(|path| path.to_string_lossy().to_string())
        .collect::<Vec<_>>();

    for open_file in &snapshot.open_files {
        if path_strings
            .iter()
            .any(|path| path_is_within(&open_file.path, path))
        {
            blockers.insert(process_label(open_file.pid, &open_file.command));
        }
    }

    for process in &snapshot.processes {
        let command = process.command.to_lowercase();
        if spec.process_terms.iter().any(|term| command.contains(term)) {
            blockers.insert(process_label(process.pid, &process.command));
        }
    }

    if matches!(spec.mode, CleanupMode::PnpmCache) {
        let has_mutating_pnpm = snapshot.processes.iter().any(|process| {
            let command = process.command.to_lowercase();
            is_mutating_pnpm_command(&command)
        });
        if has_mutating_pnpm {
            return ("inUse", blockers.into_iter().take(4).collect());
        }
        if !blockers.is_empty() {
            return ("partial", blockers.into_iter().take(4).collect());
        }
    }

    if blockers.is_empty() {
        ("ready", Vec::new())
    } else {
        ("inUse", blockers.into_iter().take(4).collect())
    }
}

fn clean_pnpm_cache(root: &Path, snapshot: &SystemSnapshot) -> Result<Vec<String>, String> {
    ensure_allowed_root(root)?;
    if !root.exists() {
        return Ok(Vec::new());
    }
    reject_symlink_root(root)?;

    let usage_text = snapshot
        .processes
        .iter()
        .map(|process| process.command.as_str())
        .chain(snapshot.open_files.iter().map(|file| file.path.as_str()))
        .collect::<Vec<_>>()
        .join("\n");
    let mut skipped = Vec::new();

    for entry in fs::read_dir(root).map_err(|error| io_error("读取 pnpm 缓存", error))? {
        let entry = entry.map_err(|error| io_error("读取 pnpm 缓存条目", error))?;
        let path = entry.path();
        if entry.file_name() == "dlx" {
            if !path.exists() {
                continue;
            }
            for dlx_entry in
                fs::read_dir(&path).map_err(|error| io_error("读取 pnpm dlx 缓存", error))?
            {
                let dlx_entry = dlx_entry.map_err(|error| io_error("读取 pnpm dlx 条目", error))?;
                let dlx_path = dlx_entry.path();
                let dlx_text = dlx_path.to_string_lossy();
                if usage_text.contains(dlx_text.as_ref()) {
                    skipped.push(display_path(&dlx_path));
                    continue;
                }
                remove_entry(&dlx_path, root)?;
            }
            continue;
        }

        let path_text = path.to_string_lossy();
        if usage_text.contains(path_text.as_ref()) {
            skipped.push(display_path(&path));
            continue;
        }
        remove_entry(&path, root)?;
    }

    Ok(skipped)
}

fn remove_contents(path: &Path, allowed_root: &Path) -> Result<(), String> {
    ensure_allowed_root(allowed_root)?;
    if !path.exists() {
        return Ok(());
    }
    reject_symlink_root(path)?;
    for entry in fs::read_dir(path).map_err(|error| io_error("读取缓存目录", error))? {
        let entry = entry.map_err(|error| io_error("读取缓存条目", error))?;
        remove_entry(&entry.path(), allowed_root)?;
    }
    Ok(())
}

fn remove_entry(path: &Path, allowed_root: &Path) -> Result<(), String> {
    ensure_safe_child(path, allowed_root)?;
    if !path.exists() && fs::symlink_metadata(path).is_err() {
        return Ok(());
    }

    let metadata = fs::symlink_metadata(path).map_err(|error| io_error("读取缓存信息", error))?;
    if metadata.file_type().is_symlink() || metadata.is_file() {
        fs::remove_file(path).map_err(|error| io_error("删除缓存文件", error))
    } else if metadata.is_dir() {
        force_remove_dir_all(path).map_err(|error| io_error("删除缓存目录", error))
    } else {
        fs::remove_file(path).map_err(|error| io_error("删除特殊缓存文件", error))
    }
}
fn force_remove_dir_all(path: &Path) -> io::Result<()> {
    if let Err(err) = fs::remove_dir_all(path) {
        if err.kind() == io::ErrorKind::PermissionDenied {
            set_writable_recursive(path)?;
            fs::remove_dir_all(path)
        } else {
            Err(err)
        }
    } else {
        Ok(())
    }
}

fn set_writable_recursive(path: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(metadata) = fs::symlink_metadata(path) {
            if metadata.file_type().is_symlink() {
                return Ok(());
            }
            let mut permissions = metadata.permissions();
            let mode = permissions.mode();
            if mode & 0o200 == 0 {
                permissions.set_mode(mode | 0o700);
                let _ = fs::set_permissions(path, permissions);
            }
            if metadata.is_dir() {
                if let Ok(entries) = fs::read_dir(path) {
                    for entry in entries.flatten() {
                        let _ = set_writable_recursive(&entry.path());
                    }
                }
            }
        }
    }
    Ok(())
}


fn ensure_allowed_root(path: &Path) -> Result<(), String> {
    let home = home_dir()?;
    if path == Path::new("/") || path == home || !path.starts_with(&home) {
        return Err("路径安全校验失败，操作已拒绝".to_string());
    }
    let trash = home.join(".Trash");
    let component_count = path.components().count();
    if path != trash && component_count < 5 {
        return Err("路径安全校验失败，操作已拒绝".to_string());
    }
    Ok(())
}

fn ensure_safe_child(path: &Path, allowed_root: &Path) -> Result<(), String> {
    ensure_allowed_root(allowed_root)?;
    if path != allowed_root && !path.starts_with(allowed_root) {
        return Err("清理路径超出白名单范围，操作已拒绝".to_string());
    }
    Ok(())
}

fn reject_symlink_root(path: &Path) -> Result<(), String> {
    let metadata = fs::symlink_metadata(path).map_err(|error| io_error("读取目录信息", error))?;
    if metadata.file_type().is_symlink() {
        return Err("目标目录是符号链接，为避免越界清理已拒绝操作".to_string());
    }
    Ok(())
}

fn run_pnpm_store_prune() -> Result<(), String> {
    let executable = find_pnpm_executable().ok_or_else(|| "未找到 pnpm".to_string())?;
    let mut command = Command::new(executable);
    command.args(["store", "prune"]);

    // GUI 应用由 Finder/LaunchServices 启动，进程 PATH 只有系统默认值
    // (/usr/bin:/bin:...)，不包含个人安装的 node。pnpm 的 shim 脚本在
    // `bin/node` 不存在时会回退到 `exec node ...`，因此必须把常见 node
    // 安装目录注入子进程 PATH，否则在图形界面下清理 pnpm store 会报
    // `exec: node: not found`。
    if let Some(home) = home_dir().ok() {
        if let Some(path) = node_aware_path(&home) {
            command.env("PATH", path);
        }
    }

    let output = command
        .output()
        .map_err(|error| format!("无法启动 pnpm store prune：{error}"))?;
    if output.status.success() {
        Ok(())
    } else {
        // pnpm 部分错误(如 ERR_PNPM_MODIFIED_DEPENDENCY)会写入 stdout 而非 stderr，
        // 仅读取 stderr 会得到空串而落入笼统的"执行失败"。因此先读 stderr，
        // 为空时回退到 stdout，让用户看到真实原因。
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
        let detail = if !stderr.is_empty() { stderr } else { stdout };
        let detail = detail.trim();
        Err(if detail.is_empty() {
            "pnpm store prune 执行失败".to_string()
        } else if detail.contains("MODIFIED_DEPENDENCY") || detail.contains("mutated") {
            format!(
                "{detail}\n提示：pnpm store 中存在被修改的包，请先运行 `pnpm install --force` 修复依赖后重试。"
            )
        } else {
            detail.to_string()
        })
    }
}

fn find_pnpm_executable() -> Option<PathBuf> {
    let home = home_dir().ok();
    let mut candidates = vec![
        PathBuf::from("/opt/homebrew/bin/pnpm"),
        PathBuf::from("/usr/local/bin/pnpm"),
    ];
    if let Some(home) = home {
        candidates.push(home.join("Library/pnpm/bin/pnpm"));
        candidates.push(home.join("Library/pnpm/pnpm"));
    }
    candidates.into_iter().find(|path| path.is_file())
}

/// 收集常见 node 可执行文件所在目录，将其注入子进程 PATH。
/// 覆盖 nvm、.local/bin、hermes、Homebrew、bun 等个人安装位置。
fn node_aware_path(home: &Path) -> Option<String> {
    let mut dirs: Vec<PathBuf> = Vec::new();
    dirs.push(home.join(".local/bin"));
    dirs.push(home.join(".hermes/node/bin"));
    dirs.push(home.join(".bun/bin"));
    dirs.push(home.join("Library/pnpm/bin"));

    // 展开 nvm 各版本目录，取每个版本下的 bin。
    let nvm_versions = home.join(".nvm/versions/node");
    if let Ok(entries) = fs::read_dir(&nvm_versions) {
        for entry in entries.flatten() {
            let bin = entry.path().join("bin");
            if bin.is_dir() {
                dirs.push(bin);
            }
        }
    }

    dirs.push(PathBuf::from("/opt/homebrew/bin"));
    dirs.push(PathBuf::from("/usr/local/bin"));

    let segments: Vec<String> = dirs
        .into_iter()
        .map(|dir| dir.to_string_lossy().to_string())
        .collect();
    if segments.is_empty() {
        return None;
    }

    let mut path = segments.join(":");
    if let Ok(existing) = env::var("PATH") {
        if !existing.is_empty() {
            path.push(':');
            path.push_str(&existing);
        }
    }
    Some(path)
}

fn directory_size(path: &Path) -> Result<u64, String> {
    if !path.exists() {
        return Ok(0);
    }
    let output = Command::new("/usr/bin/du")
        .args(["-sk"])
        .arg(path)
        .output()
        .map_err(|error| format!("无法统计 {}：{error}", display_path(path)))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(if stderr.is_empty() {
            format!("无法读取 {}", display_path(path))
        } else {
            stderr
        });
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let kib = stdout
        .split_whitespace()
        .next()
        .and_then(|value| value.parse::<u64>().ok())
        .ok_or_else(|| format!("无法解析 {} 的大小", display_path(path)))?;
    Ok(kib.saturating_mul(1024))
}

impl SystemSnapshot {
    fn capture() -> Self {
        Self {
            processes: capture_processes(),
            open_files: capture_open_files(),
        }
    }
}

fn capture_processes() -> Vec<ProcessInfo> {
    let output = match Command::new("/bin/ps")
        .args(["ax", "-o", "pid=,command="])
        .output()
    {
        Ok(output) => output,
        Err(_) => return Vec::new(),
    };
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            let split_at = line.find(char::is_whitespace)?;
            let pid = line[..split_at].parse().ok()?;
            let command = line[split_at..].trim().to_string();
            if command.is_empty() {
                None
            } else {
                Some(ProcessInfo { pid, command })
            }
        })
        .collect()
}

fn capture_open_files() -> Vec<OpenFile> {
    let output = match Command::new("/usr/sbin/lsof")
        .args(["-nP", "-Fpcn"])
        .output()
    {
        Ok(output) => output,
        Err(_) => return Vec::new(),
    };
    let mut pid = 0;
    let mut command = String::new();
    let mut files = Vec::new();
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        match line.chars().next() {
            Some('p') => pid = line[1..].parse().unwrap_or(0),
            Some('c') => command = line[1..].to_string(),
            Some('n') if line.len() > 1 && line[1..].starts_with('/') => files.push(OpenFile {
                pid,
                command: command.clone(),
                path: line[1..].to_string(),
            }),
            _ => {}
        }
    }
    files
}

fn static_target_specs(home: &Path) -> Result<Vec<TargetSpec>, String> {
    let cache = |path: &str| home.join("Library/Caches").join(path);
    let home_path = |path: &str| home.join(path);

    Ok(vec![
        // ==================== Package Managers ====================
        TargetSpec::new(
            "uv-cache",
            "uv Python 缓存",
            CATEGORY_PACKAGE,
            "Python 包下载与解压缓存，可按需重新生成。",
            "不会删除虚拟环境或项目依赖。",
            vec![home_path(".cache/uv")],
            vec![" uv sync".into(), " uv pip".into(), "/uv ".into()],
            CleanupMode::RemoveContents,
        ),
        TargetSpec::new(
            "npm-cache",
            "npm 与 npx 缓存",
            CATEGORY_PACKAGE,
            "npm 包缓存与 npx 临时执行目录。",
            "不会删除全局包或项目 node_modules。",
            vec![home_path(".npm/_cacache"), home_path(".npm/_npx")],
            vec![
                "npm-cli.js".into(),
                "npx-cli.js".into(),
                "/.npm/_npx/".into(),
            ],
            CleanupMode::RemoveRoots,
        ),
        TargetSpec::new(
            "pnpm-cache",
            "pnpm 元数据与 dlx 缓存",
            CATEGORY_PACKAGE,
            "包元数据和临时执行缓存，正在使用的 dlx 条目会保留。",
            "不会删除 pnpm store 或项目依赖。",
            vec![cache("pnpm")],
            vec!["pnpm.mjs dlx".into(), "/caches/pnpm/".into()],
            CleanupMode::PnpmCache,
        ),
        TargetSpec::new(
            "pnpm-store",
            "pnpm 未引用包",
            CATEGORY_PACKAGE,
            "通过 pnpm store prune 仅裁剪未被项目引用的包。",
            "保留仍被项目链接的内容。",
            vec![
                home_path("Library/pnpm/store/v11"),
                home_path("Library/pnpm/store/v3"),
            ],
            vec![
                "pnpm.mjs install".into(),
                "pnpm.mjs add".into(),
                "pnpm.mjs update".into(),
                "pnpm.mjs remove".into(),
            ],
            CleanupMode::PnpmStorePrune,
        ),
        TargetSpec::new(
            "yarn-cache",
            "Yarn 缓存",
            CATEGORY_PACKAGE,
            "Yarn 的离线包与下载缓存。",
            "不会删除项目 node_modules。",
            vec![cache("Yarn"), home_path(".yarn/cache")],
            vec!["/yarn ".into(), "yarn.js".into()],
            CleanupMode::RemoveRoots,
        ),
        TargetSpec::new(
            "bun-cache",
            "Bun 包安装缓存",
            CATEGORY_PACKAGE,
            "Bun 包管理器下载的离线包与 install 缓存。",
            "不会影响全局 Bun 运行时或项目 node_modules。",
            vec![home_path(".bun/install/cache")],
            vec!["bun install".into(), "bun add".into(), "bun pm".into()],
            CleanupMode::RemoveContents,
        ),
        TargetSpec::new(
            "cargo-cache",
            "Cargo 依赖包与索引缓存",
            CATEGORY_PACKAGE,
            "Rust Cargo 下载的 crates.io 源码压缩包与 Git 检出库。",
            "不删除 ~/.cargo/bin 全局程序或本地项目源码。",
            vec![
                home_path(".cargo/registry/cache"),
                home_path(".cargo/git/db"),
            ],
            vec!["cargo ".into(), "rustc".into(), "cargo-clippy".into()],
            CleanupMode::RemoveContents,
        ),
        TargetSpec::new(
            "go-cache",
            "Go 编译与模块下载缓存",
            CATEGORY_PACKAGE,
            "Go 编译构建缓存与已下载模块的只读压缩包。",
            "不删除 GOPATH/bin 程序；后续构建按需重新下载编译。",
            vec![
                cache("go-build"),
                home_path(".cache/go-build"),
                home_path("go/pkg/mod/cache"),
            ],
            vec![
                "go build".into(),
                "go test".into(),
                "go run".into(),
                "go get".into(),
                "gopls".into(),
            ],
            CleanupMode::RemoveContents,
        ),
        TargetSpec::new(
            "pip-cache",
            "pip 缓存",
            CATEGORY_PACKAGE,
            "Python pip 下载的 wheel 包与源码归档缓存。",
            "不会删除已安装的全局或虚拟环境依赖。",
            vec![cache("pip"), home_path(".cache/pip")],
            vec!["pip install".into(), "pip download".into()],
            CleanupMode::RemoveContents,
        ),
        TargetSpec::new(
            "conda-cache",
            "Conda 包下载归档",
            CATEGORY_PACKAGE,
            "Conda、Miniconda 与 Miniforge 的 tarball 压缩包与索引缓存。",
            "不会删除任何已创建的虚拟环境。",
            vec![
                home_path(".conda/pkgs"),
                home_path("miniconda3/pkgs"),
                home_path("miniforge3/pkgs"),
                home_path("anaconda3/pkgs"),
            ],
            vec!["conda ".into(), "mamba ".into(), "micromamba".into()],
            CleanupMode::RemoveContents,
        ),
        TargetSpec::new(
            "cocoapods-cache",
            "CocoaPods 缓存",
            CATEGORY_PACKAGE,
            "CocoaPods 下载的 Pod 源码包与 Specs 索引缓存。",
            "不会影响项目的 Pods 目录；再次 pod install 会按需拉取。",
            vec![cache("CocoaPods")],
            vec!["pod install".into(), "pod update".into(), "cocoapods".into()],
            CleanupMode::RemoveContents,
        ),
        TargetSpec::new(
            "homebrew-cache",
            "Homebrew 下载缓存",
            CATEGORY_PACKAGE,
            "Homebrew 下载的 bottle 安装包、源码与 API 缓存。",
            "不会卸载已安装的软件包。",
            vec![cache("Homebrew/downloads"), cache("Homebrew")],
            vec!["/brew ".into(), "homebrew".into()],
            CleanupMode::RemoveContents,
        ),
        TargetSpec::new(
            "maven-cache",
            "Maven 依赖缓存",
            CATEGORY_PACKAGE,
            "Maven 本地仓库下载的 jar 依赖与元数据缓存。",
            "构建时会根据 pom.xml 自动重新下载所需依赖。",
            vec![home_path(".m2/repository")],
            vec!["mvn ".into(), "maven".into(), "org.apache.maven".into()],
            CleanupMode::RemoveContents,
        ),
        TargetSpec::new(
            "composer-cache",
            "Composer 缓存",
            CATEGORY_PACKAGE,
            "PHP Composer 下载的包归档与 VCS 缓存。",
            "不会删除项目的 vendor 目录。",
            vec![cache("composer"), home_path(".composer/cache")],
            vec!["composer ".into()],
            CleanupMode::RemoveContents,
        ),
        TargetSpec::new(
            "gem-cache",
            "Ruby Gem 与 Bundler 缓存",
            CATEGORY_PACKAGE,
            "RubyGems 规范索引与 Bundler 下载缓存。",
            "不会删除已安装的全局 Gem 程序。",
            vec![home_path(".gem/specs"), home_path(".bundle/cache")],
            vec!["gem install".into(), "bundle install".into()],
            CleanupMode::RemoveContents,
        ),
        TargetSpec::new(
            "node-gyp-cache",
            "node-gyp 原生模块构建缓存",
            CATEGORY_PACKAGE,
            "Node.js 编译原生 C/C++ 扩展时下载的 Node 头文件与 SDK 缓存。",
            "不会影响已编译完成的项目 node_modules。",
            vec![cache("node-gyp")],
            vec!["node-gyp".into()],
            CleanupMode::RemoveContents,
        ),

        // ==================== Build Caches ====================
        TargetSpec::new(
            "xcode-derived-data",
            "Xcode DerivedData",
            CATEGORY_BUILD,
            "Xcode 索引、中间文件与项目构建缓存。",
            "不删除项目；下次构建与索引耗时会增加。",
            vec![home_path("Library/Developer/Xcode/DerivedData")],
            vec!["/applications/xcode.app/".into(), "xcodebuild".into()],
            CleanupMode::RemoveContents,
        ),
        TargetSpec::new(
            "xcode-archives",
            "Xcode 历史归档产物",
            CATEGORY_BUILD,
            "Xcode 生成的本地 xcarchive 打包归档文件。",
            "仅清理本地打包归档历史，不影响线上已发布版本。",
            vec![home_path("Library/Developer/Xcode/Archives")],
            vec!["/applications/xcode.app/".into(), "xcodebuild".into()],
            CleanupMode::RemoveContents,
        ),
        TargetSpec::new(
            "ios-simulator-cache",
            "iOS 模拟器运行时缓存",
            CATEGORY_BUILD,
            "CoreSimulator 生成的运行时镜像与临时数据缓存。",
            "不会删除模拟器固件或已安装应用。",
            vec![
                home_path("Library/Developer/CoreSimulator/Caches"),
                cache("com.apple.CoreSimulator"),
            ],
            vec![
                "simulator.app".into(),
                "coresimulatord".into(),
                "xcode.app".into(),
            ],
            CleanupMode::RemoveContents,
        ),
        TargetSpec::new(
            "gradle-cache",
            "Gradle 构建缓存",
            CATEGORY_BUILD,
            "Android 与 JVM 项目的依赖和编译缓存。",
            "不会删除项目；后续构建可能重新下载依赖。",
            vec![home_path(".gradle/caches")],
            vec![
                "gradledaemon".into(),
                "org.gradle".into(),
                "/gradle ".into(),
            ],
            CleanupMode::RemoveContents,
        ),
        TargetSpec::new(
            "android-build-cache",
            "Android SDK 与构建缓存",
            CATEGORY_BUILD,
            "Android Studio 与构建工具生成的缓存文件。",
            "不会删除 Android SDK 核心工具或项目代码。",
            vec![
                home_path(".android/build-cache"),
                home_path(".android/cache"),
            ],
            vec![
                "studio.app".into(),
                "android studio".into(),
                "adb ".into(),
            ],
            CleanupMode::RemoveContents,
        ),
        TargetSpec::new(
            "electron-builder-cache",
            "Electron 二进制包缓存",
            CATEGORY_BUILD,
            "electron-builder 与打包工具预下载的各平台二进制。",
            "不影响当前项目依赖，打包时按需重新拉取。",
            vec![cache("electron"), cache("electron-builder")],
            vec!["electron-builder".into(), "electron-forge".into()],
            CleanupMode::RemoveContents,
        ),
        TargetSpec::new(
            "turbo-cache",
            "Turborepo 构建缓存",
            CATEGORY_BUILD,
            "Turborepo 生成的本地任务编译产物缓存。",
            "下次执行 turbo build 会按需重新计算。",
            vec![cache("turbo")],
            vec!["turbo ".into(), "turborepo".into()],
            CleanupMode::RemoveContents,
        ),

        // ==================== Tool Caches & Logs ====================
        TargetSpec::new(
            "playwright-cache",
            "Playwright 浏览器缓存",
            CATEGORY_TOOL,
            "Playwright 下载的 Chromium、Firefox 与 WebKit。",
            "测试时会重新下载所需浏览器。",
            vec![cache("ms-playwright"), cache("ms-playwright-go")],
            vec!["playwright".into(), "/ms-playwright/".into()],
            CleanupMode::RemoveRoots,
        ),
        TargetSpec::new(
            "cypress-cache",
            "Cypress 测试运行环境",
            CATEGORY_TOOL,
            "Cypress 自动化测试框架下载的浏览器与二进制运行时缓存。",
            "运行 cypress 时若缺失会自动重新下载。",
            vec![cache("Cypress")],
            vec!["cypress".into()],
            CleanupMode::RemoveContents,
        ),
        TargetSpec::new(
            "wechat-devtools-cache",
            "微信开发者工具缓存",
            CATEGORY_TOOL,
            "微信开发者工具的编译与运行缓存。",
            "不会删除小程序项目。",
            vec![cache("微信开发者工具")],
            vec!["wechatwebdevtools".into(), "微信开发者工具".into()],
            CleanupMode::RemoveContents,
        ),
        TargetSpec::new(
            "vscode-update-cache",
            "VS Code 更新缓存",
            CATEGORY_TOOL,
            "VS Code ShipIt 下载与更新暂存文件。",
            "不会删除扩展、设置或工作区数据。",
            vec![cache("com.microsoft.VSCode.ShipIt")],
            vec!["visual studio code.app".into(), "code helper".into()],
            CleanupMode::RemoveRoots,
        ),
        TargetSpec::new(
            "cursor-cache",
            "Cursor 运行与更新缓存",
            CATEGORY_TOOL,
            "Cursor AI 编辑器的运行缓存与更新暂存文件。",
            "不会删除扩展、设置、对话历史或工作区配置。",
            vec![
                cache("Cursor"),
                cache("com.todesktop.230313mzl4w4u92"),
                cache("com.todesktop.230313mzl4w4u92.ShipIt"),
            ],
            vec!["cursor.app".into(), "cursor helper".into()],
            CleanupMode::RemoveContents,
        ),
        TargetSpec::new(
            "claude-cache",
            "Claude 桌面与 CLI 缓存",
            CATEGORY_TOOL,
            "Claude 客户端与 CLI 工具的临时运行缓存与更新暂存。",
            "不会删除登录状态、对话记录或本地凭证配置。",
            vec![
                cache("com.anthropic.claudefordesktop"),
                cache("com.anthropic.claudefordesktop.ShipIt"),
                home_path(".claude/cache"),
            ],
            vec!["claude.app".into(), "claude helper".into()],
            CleanupMode::RemoveContents,
        ),
        TargetSpec::new(
            "codex-cache",
            "Codex 应用缓存",
            CATEGORY_TOOL,
            "Codex 的可再生应用缓存。",
            "不清理任务、记忆、插件或日志数据库。",
            vec![cache("Codex")],
            vec!["/applications/codex.app/".into()],
            CleanupMode::RemoveContents,
        ),
        TargetSpec::new(
            "docker-cache",
            "Docker 与容器管理工具缓存",
            CATEGORY_TOOL,
            "Docker BuildX 缓存及 Docker Desktop / OrbStack 日志与诊断缓存。",
            "不会删除容器镜像、数据卷或运行中的容器。",
            vec![
                home_path(".docker/buildx/cache"),
                cache("com.docker.docker"),
                cache("dev.kdrag0n.MacVirt"),
            ],
            vec![
                "docker.app".into(),
                "com.docker".into(),
                "orbstack".into(),
            ],
            CleanupMode::RemoveContents,
        ),
        TargetSpec::new(
            "jetbrains-cache",
            "JetBrains IDE 索引与缓存",
            CATEGORY_TOOL,
            "IntelliJ IDEA、WebStorm、PyCharm、GoLand 等 IDE 的本地索引与临时缓存。",
            "不会删除插件、配置或项目源码；下次打开项目会自动重建索引。",
            vec![cache("JetBrains")],
            vec![
                "idea.app".into(),
                "webstorm.app".into(),
                "pycharm.app".into(),
                "goland.app".into(),
                "clion.app".into(),
                "datagrip.app".into(),
                "fleet.app".into(),
            ],
            CleanupMode::RemoveContents,
        ),
        TargetSpec::new(
            "postman-cache",
            "API 调试工具缓存",
            CATEGORY_TOOL,
            "Postman 与 Apifox 等工具的临时运行日志与请求缓存。",
            "不会删除接口集合、环境变量或本地保存的 API 数据。",
            vec![cache("com.postmanlabs.mac"), cache("cn.apifox.app")],
            vec![
                "postman.app".into(),
                "postman helper".into(),
                "apifox.app".into(),
                "apifox helper".into(),
            ],
            CleanupMode::RemoveContents,
        ),
        TargetSpec::new(
            "chrome-cache",
            "Chrome 缓存",
            CATEGORY_TOOL,
            "Chrome 与相关 Google 组件的用户缓存。",
            "不会删除书签和浏览器配置，网页资源会重新加载。",
            vec![cache("Google")],
            vec![
                "/applications/google chrome.app/".into(),
                "google chrome helper".into(),
            ],
            CleanupMode::RemoveContents,
        ),
        TargetSpec::new(
            "hbuilder-cache",
            "HBuilderX 缓存",
            CATEGORY_TOOL,
            "HBuilderX 的下载、索引与临时缓存。",
            "不会删除 uni-app 项目。",
            vec![cache("HBuilder X")],
            vec!["/applications/hbuilderx.app/".into(), "hbuilderx".into()],
            CleanupMode::RemoveContents,
        ),
        TargetSpec::new(
            "user-app-logs",
            "用户与应用系统日志",
            CATEGORY_TOOL,
            "应用运行日志、诊断报告与崩溃日志 (DiagnosticReports)。",
            "不会影响应用正常运行或用户数据配置。",
            vec![
                home_path("Library/Logs"),
                home_path("Library/Application Support/CrashReporter"),
            ],
            vec!["logd".into()],
            CleanupMode::RemoveContents,
        ),
        TargetSpec::new(
            "typescript-cache",
            "TypeScript 语言服务缓存",
            CATEGORY_TOOL,
            "TypeScript 自动类型获取 (ATA) 与语言服务语法树缓存。",
            "下次打开编辑器时会自动重新建立语法类型索引。",
            vec![cache("typescript")],
            vec!["tsserver".into(), "typescript".into()],
            CleanupMode::RemoveContents,
        ),
        TargetSpec::new(
            "electron-updater-cache",
            "应用更新安装包暂存",
            CATEGORY_TOOL,
            "基于 Electron 与 ShipIt 的应用下载的历史版本升级安装包。",
            "不会影响已安装的当前版本程序。",
            vec![
                cache("antigravity-updater"),
                cache("@openchamberelectron-updater"),
                cache("@opencode-aidesktop-updater"),
                cache("@zcodedesktop-updater"),
                cache("tdappdesktop-updater"),
            ],
            vec!["shipit".into()],
            CleanupMode::RemoveRoots,
        ),
        TargetSpec::new(
            "android-studio-cache",
            "Android Studio 索引与缓存",
            CATEGORY_TOOL,
            "Android Studio IDE 的编译索引、GAV 依赖缓存与插件临时文件。",
            "不会删除 SDK 或项目代码；打开项目时会自动重建索引。",
            vec![
                cache("Google/AndroidStudio2023.3"),
                cache("Google/AndroidStudio2024.1"),
                cache("Google/AndroidStudio2024.2"),
                cache("Google/AndroidStudio2024.3"),
            ],
            vec!["studio.app".into(), "android studio".into()],
            CleanupMode::RemoveContents,
        ),
        TargetSpec::new(
            "saved-app-state",
            "应用窗口恢复状态 (Saved State)",
            CATEGORY_TOOL,
            "macOS 保存的已关闭应用程序窗口状态与会话缓存。",
            "不会删除应用数据，应用启动时将以默认窗口打开。",
            vec![home_path("Library/Saved Application State")],
            vec![],
            CleanupMode::RemoveContents,
        ),
        TargetSpec::new(
            "system-trash",
            "macOS 废纸篓",
            CATEGORY_TOOL,
            "当前用户废纸篓中待永久删除的文件。",
            "永久清空废纸篓中所有已删除条目。",
            vec![home_path(".Trash")],
            vec![],
            CleanupMode::RemoveContents,
        ),
    ])
}

fn candidate_workspace_roots(home: &Path) -> Vec<PathBuf> {
    vec![
        home.join("Desktop/code"),
        home.join("Projects"),
        home.join("code"),
        home.join("workspace"),
        home.join("Developer"),
        home.join("Desktop/projects"),
    ]
}

fn discover_workspace_project_specs(home: &Path) -> Vec<TargetSpec> {
    let mut specs = Vec::new();
    let roots = candidate_workspace_roots(home);

    for root in roots {
        if !root.is_dir() {
            continue;
        }

        let entries = match fs::read_dir(&root) {
            Ok(entries) => entries,
            Err(_) => continue,
        };

        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }

            let name = match path.file_name().and_then(|n| n.to_str()) {
                Some(name) if !name.starts_with('.') => name,
                _ => continue,
            };

            let mut cache_paths = Vec::new();

            let node_cache = path.join("node_modules/.cache");
            if node_cache.is_dir() {
                cache_paths.push(node_cache);
            }

            let next_cache = path.join(".next/cache");
            if next_cache.is_dir() {
                cache_paths.push(next_cache);
            }

            let turbo_cache = path.join(".turbo");
            if turbo_cache.is_dir() {
                cache_paths.push(turbo_cache);
            }

            let nuxt_cache = path.join(".nuxt");
            if nuxt_cache.is_dir() {
                cache_paths.push(nuxt_cache);
            }

            let tauri_target = path.join("src-tauri/target");
            if tauri_target.is_dir() {
                cache_paths.push(tauri_target);
            }

            if path.join("Cargo.toml").is_file() {
                let rust_target = path.join("target");
                if rust_target.is_dir() {
                    cache_paths.push(rust_target);
                }
            }

            if cache_paths.is_empty() {
                continue;
            }

            let id = if let Ok(rel) = path.strip_prefix(home) {
                let slug = rel.to_string_lossy().replace(['/', '\\', ' ', '.'], "-");
                format!("project-build-{slug}")
            } else {
                format!("project-build-{name}")
            };

            let name_display = format!("{name} 构建缓存");
            let desc = format!("{name} 项目的本地中间编译产物与构建缓存（如 Webpack/Next/Vite/Target）。");

            specs.push(TargetSpec::new(
                id,
                name_display,
                CATEGORY_BUILD,
                desc,
                "保留项目源码与 node_modules 依赖；重新构建时会自动生成。",
                cache_paths,
                vec![format!("/{}/", name.to_lowercase())],
                CleanupMode::RemoveRoots,
            ));
        }
    }

    specs
}

fn target_specs() -> Result<Vec<TargetSpec>, String> {
    let home = home_dir()?;
    let mut specs = static_target_specs(&home)?;
    let dynamic_specs = discover_workspace_project_specs(&home);
    specs.extend(dynamic_specs);
    Ok(specs)
}

fn home_dir() -> Result<PathBuf, String> {
    env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or_else(|| "无法确定当前用户目录".to_string())
}

fn display_path(path: &Path) -> String {
    if let Ok(home) = home_dir() {
        if let Ok(relative) = path.strip_prefix(&home) {
            return format!("~/{}", relative.display());
        }
    }
    path.display().to_string()
}

fn process_label(pid: u32, command: &str) -> String {
    let lower = command.to_lowercase();
    let name = if lower.contains("visual studio code") || lower.contains("code helper") {
        "VS Code"
    } else if lower.contains("cursor") {
        "Cursor"
    } else if lower.contains("claude") {
        "Claude"
    } else if lower.contains("google chrome") {
        "Chrome"
    } else if lower.contains("wechatwebdevtools") || lower.contains("微信开发者工具") {
        "微信开发者工具"
    } else if lower.contains("playwright") {
        "Playwright"
    } else if lower.contains("cypress") {
        "Cypress"
    } else if lower.contains("xcode") {
        "Xcode"
    } else if lower.contains("simulator") || lower.contains("coresimulatord") {
        "iOS 模拟器"
    } else if lower.contains("studio") || lower.contains("adb") {
        "Android Studio"
    } else if lower.contains("gradle") {
        "Gradle"
    } else if lower.contains("pnpm") {
        "pnpm"
    } else if lower.contains("npm") || lower.contains("npx") {
        "npm"
    } else if lower.contains("bun") {
        "Bun"
    } else if lower.contains("cargo") || lower.contains("rustc") {
        "Cargo / Rust"
    } else if lower.contains("go build") || lower.contains("gopls") {
        "Go"
    } else if lower.contains("conda") || lower.contains("mamba") {
        "Conda"
    } else if lower.contains("docker") || lower.contains("orbstack") {
        "Docker / 容器"
    } else if lower.contains("idea")
        || lower.contains("webstorm")
        || lower.contains("pycharm")
        || lower.contains("goland")
        || lower.contains("clion")
    {
        "JetBrains IDE"
    } else if lower.contains("postman") {
        "Postman"
    } else if lower.contains("apifox") {
        "Apifox"
    } else if lower.contains("hbuilder") {
        "HBuilderX"
    } else if lower.contains("codex") {
        "Codex"
    } else {
        command
            .split_whitespace()
            .next()
            .and_then(|value| Path::new(value).file_name())
            .and_then(|value| value.to_str())
            .unwrap_or("未知进程")
    };
    format!("{name} (PID {pid})")
}

fn is_mutating_pnpm_command(command: &str) -> bool {
    [
        " install",
        " add",
        " update",
        " remove",
        " import",
        " deploy",
        " store prune",
    ]
    .iter()
    .any(|verb| command.contains("pnpm") && command.contains(verb))
}

fn path_is_within(candidate: &str, root: &str) -> bool {
    candidate == root
        || candidate
            .strip_prefix(root)
            .is_some_and(|suffix| suffix.starts_with('/'))
}

fn io_error(action: &str, error: io::Error) -> String {
    format!("{action}失败：{error}")
}

fn now_timestamp() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::{
        append_cleanup_history, clear_cleanup_history_sync, ensure_allowed_root,
        is_mutating_pnpm_command, node_aware_path, path_is_within, read_cleanup_history,
        target_specs, CleanupHistoryEntry,
    };
    use std::{
        collections::BTreeSet,
        env, fs,
        process::Command,
        time::{SystemTime, UNIX_EPOCH},
    };

    #[test]
    fn path_boundary_does_not_match_similar_prefix() {
        assert!(path_is_within("/tmp/cache/entry", "/tmp/cache"));
        assert!(path_is_within("/tmp/cache", "/tmp/cache"));
        assert!(!path_is_within("/tmp/cache-copy", "/tmp/cache"));
    }

    #[test]
    fn pnpm_dlx_is_not_treated_as_mutating_install() {
        assert!(!is_mutating_pnpm_command("node pnpm.mjs dlx playwright"));
        assert!(is_mutating_pnpm_command("node pnpm.mjs install"));
        assert!(is_mutating_pnpm_command("pnpm store prune"));
    }

    #[test]
    fn node_aware_path_resolves_node_executable() {
        let home = super::home_dir().unwrap();
        let Some(path) = node_aware_path(&home) else {
            return;
        };
        // GUI 进程 PATH 只有默认系统目录；注入后的 PATH 应在 sh 中定位到 node。
        let output = Command::new("/bin/sh")
            .arg("-c")
            .arg("command -v node")
            .env("PATH", &path)
            .output()
            .unwrap();
        if output.status.success() {
            let found = String::from_utf8_lossy(&output.stdout);
            assert!(!found.trim().is_empty(), "node should be resolvable after PATH injection");
        }
    }

    #[test]
    fn cleanup_history_keeps_the_latest_hundred_entries() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let history_dir = env::temp_dir().join(format!("dev-cache-cleaner-history-{unique}"));

        for index in 0..101 {
            append_cleanup_history(
                &history_dir,
                CleanupHistoryEntry {
                    id: index.to_string(),
                    target_name: "测试缓存".to_string(),
                    status: "success".to_string(),
                    freed_bytes: index,
                    skipped_entries: Vec::new(),
                    message: "清理完成".to_string(),
                    created_at: index,
                },
            )
            .unwrap();
        }

        let history = read_cleanup_history(&history_dir).unwrap();
        assert_eq!(history.len(), 100);
        assert_eq!(history.first().unwrap().id, "100");
        assert_eq!(history.last().unwrap().id, "1");

        clear_cleanup_history_sync(&history_dir).unwrap();
        assert!(read_cleanup_history(&history_dir).unwrap().is_empty());
        fs::remove_dir_all(history_dir).unwrap();
    }

    #[test]
    fn target_specs_have_unique_ids_and_valid_allowed_roots() {
        let specs = target_specs().unwrap();
        let mut ids = BTreeSet::new();
        for spec in specs {
            assert!(ids.insert(spec.id.clone()), "Duplicate target id found: {}", spec.id);
            for path in spec.paths {
                ensure_allowed_root(&path).expect("Allowed root verification failed");
            }
        }
    }

    #[test]
    fn workspace_project_caches_are_dynamically_discovered() {
        let home = super::home_dir().unwrap();
        let specs = super::discover_workspace_project_specs(&home);
        if home.join("Desktop/code").is_dir() {
            assert!(!specs.is_empty(), "Expected to discover projects under Desktop/code");
            if home.join("Desktop/code/pts-business/node_modules/.cache").is_dir() {
                let has_pts_business = specs.iter().any(|spec| spec.name.contains("pts-business"));
                assert!(has_pts_business, "Expected pts-business to be discovered dynamically");
            }
        }
    }

}
