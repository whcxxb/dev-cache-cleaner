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
    id: &'static str,
    name: &'static str,
    category: &'static str,
    description: &'static str,
    cleanup_note: &'static str,
    paths: Vec<PathBuf>,
    process_terms: Vec<String>,
    mode: CleanupMode,
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
        fs::remove_dir_all(path).map_err(|error| io_error("删除缓存目录", error))
    } else {
        fs::remove_file(path).map_err(|error| io_error("删除特殊缓存文件", error))
    }
}

fn ensure_allowed_root(path: &Path) -> Result<(), String> {
    let home = home_dir()?;
    let component_count = path.components().count();
    if path == Path::new("/") || path == home || !path.starts_with(&home) || component_count < 5 {
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
    let output = Command::new(executable)
        .args(["store", "prune"])
        .output()
        .map_err(|error| format!("无法启动 pnpm store prune：{error}"))?;
    if output.status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        Err(if stderr.is_empty() {
            "pnpm store prune 执行失败".to_string()
        } else {
            stderr
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

fn target_specs() -> Result<Vec<TargetSpec>, String> {
    let home = home_dir()?;
    let cache = |path: &str| home.join("Library/Caches").join(path);
    let home_path = |path: &str| home.join(path);

    Ok(vec![
        TargetSpec {
            id: "uv-cache",
            name: "uv Python 缓存",
            category: CATEGORY_PACKAGE,
            description: "Python 包下载与解压缓存，可按需重新生成。",
            cleanup_note: "不会删除虚拟环境或项目依赖。",
            paths: vec![home_path(".cache/uv")],
            process_terms: vec![" uv sync".into(), " uv pip".into(), "/uv ".into()],
            mode: CleanupMode::RemoveContents,
        },
        TargetSpec {
            id: "npm-cache",
            name: "npm 与 npx 缓存",
            category: CATEGORY_PACKAGE,
            description: "npm 包缓存与 npx 临时执行目录。",
            cleanup_note: "不会删除全局包或项目 node_modules。",
            paths: vec![home_path(".npm/_cacache"), home_path(".npm/_npx")],
            process_terms: vec![
                "npm-cli.js".into(),
                "npx-cli.js".into(),
                "/.npm/_npx/".into(),
            ],
            mode: CleanupMode::RemoveRoots,
        },
        TargetSpec {
            id: "pnpm-cache",
            name: "pnpm 元数据与 dlx 缓存",
            category: CATEGORY_PACKAGE,
            description: "包元数据和临时执行缓存，正在使用的 dlx 条目会保留。",
            cleanup_note: "不会删除 pnpm store 或项目依赖。",
            paths: vec![cache("pnpm")],
            process_terms: vec!["pnpm.mjs dlx".into(), "/caches/pnpm/".into()],
            mode: CleanupMode::PnpmCache,
        },
        TargetSpec {
            id: "pnpm-store",
            name: "pnpm 未引用包",
            category: CATEGORY_PACKAGE,
            description: "通过 pnpm store prune 仅裁剪未被项目引用的包。",
            cleanup_note: "保留仍被项目链接的内容。",
            paths: vec![home_path("Library/pnpm/store/v11")],
            process_terms: vec![
                "pnpm.mjs install".into(),
                "pnpm.mjs add".into(),
                "pnpm.mjs update".into(),
                "pnpm.mjs remove".into(),
            ],
            mode: CleanupMode::PnpmStorePrune,
        },
        TargetSpec {
            id: "pts-business-cache",
            name: "pts-business 构建缓存",
            category: CATEGORY_BUILD,
            description: "Umi MFSU 与 Webpack 的本地构建缓存。",
            cleanup_note: "保留 node_modules；首次启动会重新构建缓存。",
            paths: vec![home_path("Desktop/code/pts-business/node_modules/.cache")],
            process_terms: vec!["/desktop/code/pts-business/".into()],
            mode: CleanupMode::RemoveRoots,
        },
        TargetSpec {
            id: "reimux-tauri-target",
            name: "reimux-tools Tauri 产物",
            category: CATEGORY_BUILD,
            description: "Rust debug 与 release 编译产物。",
            cleanup_note: "不删除源码；下次构建会重新编译。",
            paths: vec![home_path("Desktop/code/reimux-tools/src-tauri/target")],
            process_terms: vec!["/desktop/code/reimux-tools/".into()],
            mode: CleanupMode::RemoveRoots,
        },
        TargetSpec {
            id: "xcode-derived-data",
            name: "Xcode DerivedData",
            category: CATEGORY_BUILD,
            description: "Xcode 索引、中间文件与项目构建缓存。",
            cleanup_note: "不删除项目；下次构建与索引耗时会增加。",
            paths: vec![home_path("Library/Developer/Xcode/DerivedData")],
            process_terms: vec!["/applications/xcode.app/".into(), "xcodebuild".into()],
            mode: CleanupMode::RemoveContents,
        },
        TargetSpec {
            id: "gradle-cache",
            name: "Gradle 构建缓存",
            category: CATEGORY_BUILD,
            description: "Android 与 JVM 项目的依赖和编译缓存。",
            cleanup_note: "不会删除项目；后续构建可能重新下载依赖。",
            paths: vec![home_path(".gradle/caches")],
            process_terms: vec![
                "gradledaemon".into(),
                "org.gradle".into(),
                "/gradle ".into(),
            ],
            mode: CleanupMode::RemoveContents,
        },
        TargetSpec {
            id: "playwright-cache",
            name: "Playwright 浏览器缓存",
            category: CATEGORY_TOOL,
            description: "Playwright 下载的 Chromium、Firefox 与 WebKit。",
            cleanup_note: "测试时会重新下载所需浏览器。",
            paths: vec![cache("ms-playwright"), cache("ms-playwright-go")],
            process_terms: vec!["playwright".into(), "/ms-playwright/".into()],
            mode: CleanupMode::RemoveRoots,
        },
        TargetSpec {
            id: "wechat-devtools-cache",
            name: "微信开发者工具缓存",
            category: CATEGORY_TOOL,
            description: "微信开发者工具的编译与运行缓存。",
            cleanup_note: "不会删除小程序项目。",
            paths: vec![cache("微信开发者工具")],
            process_terms: vec!["wechatwebdevtools".into(), "微信开发者工具".into()],
            mode: CleanupMode::RemoveContents,
        },
        TargetSpec {
            id: "vscode-update-cache",
            name: "VS Code 更新缓存",
            category: CATEGORY_TOOL,
            description: "VS Code ShipIt 下载与更新暂存文件。",
            cleanup_note: "不会删除扩展、设置或工作区数据。",
            paths: vec![cache("com.microsoft.VSCode.ShipIt")],
            process_terms: vec!["visual studio code.app".into(), "code helper".into()],
            mode: CleanupMode::RemoveRoots,
        },
        TargetSpec {
            id: "codex-cache",
            name: "Codex 应用缓存",
            category: CATEGORY_TOOL,
            description: "Codex 的可再生应用缓存。",
            cleanup_note: "不清理任务、记忆、插件或日志数据库。",
            paths: vec![cache("Codex")],
            process_terms: vec!["/applications/codex.app/".into()],
            mode: CleanupMode::RemoveContents,
        },
        TargetSpec {
            id: "chrome-cache",
            name: "Chrome 缓存",
            category: CATEGORY_TOOL,
            description: "Chrome 与相关 Google 组件的用户缓存。",
            cleanup_note: "不会删除书签和浏览器配置，网页资源会重新加载。",
            paths: vec![cache("Google")],
            process_terms: vec![
                "/applications/google chrome.app/".into(),
                "google chrome helper".into(),
            ],
            mode: CleanupMode::RemoveContents,
        },
        TargetSpec {
            id: "homebrew-cache",
            name: "Homebrew 下载缓存",
            category: CATEGORY_PACKAGE,
            description: "Homebrew 下载的包、源码与 API 缓存。",
            cleanup_note: "不会卸载已安装的软件包。",
            paths: vec![cache("Homebrew")],
            process_terms: vec!["/brew ".into(), "homebrew".into()],
            mode: CleanupMode::RemoveContents,
        },
        TargetSpec {
            id: "yarn-cache",
            name: "Yarn 缓存",
            category: CATEGORY_PACKAGE,
            description: "Yarn 的离线包与下载缓存。",
            cleanup_note: "不会删除项目 node_modules。",
            paths: vec![cache("Yarn"), home_path(".yarn/cache")],
            process_terms: vec!["/yarn ".into(), "yarn.js".into()],
            mode: CleanupMode::RemoveRoots,
        },
        TargetSpec {
            id: "hbuilder-cache",
            name: "HBuilderX 缓存",
            category: CATEGORY_TOOL,
            description: "HBuilderX 的下载、索引与临时缓存。",
            cleanup_note: "不会删除 uni-app 项目。",
            paths: vec![cache("HBuilder X")],
            process_terms: vec!["/applications/hbuilderx.app/".into(), "hbuilderx".into()],
            mode: CleanupMode::RemoveContents,
        },
    ])
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
    } else if lower.contains("google chrome") {
        "Chrome"
    } else if lower.contains("wechatwebdevtools") || lower.contains("微信开发者工具") {
        "微信开发者工具"
    } else if lower.contains("playwright") {
        "Playwright"
    } else if lower.contains("xcode") {
        "Xcode"
    } else if lower.contains("gradle") {
        "Gradle"
    } else if lower.contains("pnpm") {
        "pnpm"
    } else if lower.contains("npm") || lower.contains("npx") {
        "npm"
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
        append_cleanup_history, clear_cleanup_history_sync, is_mutating_pnpm_command,
        path_is_within, read_cleanup_history, CleanupHistoryEntry,
    };
    use std::{
        env, fs,
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
}
