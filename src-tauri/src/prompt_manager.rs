use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    env, fs, io,
    os::unix::fs::symlink,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};
use tauri::Manager;

const GLOBAL_FILE_NAME: &str = "global.md";
const STATE_FILE_NAME: &str = "state.json";
const GROK_GLOBAL_FILE_NAME: &str = "dev-cache-cleaner-global.md";
const GROK_PERSONAL_FILE_NAME: &str = "dev-cache-cleaner.md";

#[derive(Clone, Copy)]
enum ToolLocation {
    File(&'static str),
    GrokRules,
}

#[derive(Clone, Copy)]
struct ToolSpec {
    id: &'static str,
    name: &'static str,
    description: &'static str,
    location: ToolLocation,
}

const TOOL_SPECS: [ToolSpec; 6] = [
    ToolSpec {
        id: "codex",
        name: "Codex",
        description: "使用全局 AGENTS.md 作为开发协作指令。",
        location: ToolLocation::File(".codex/AGENTS.md"),
    },
    ToolSpec {
        id: "pi",
        name: "pi",
        description: "使用全局 AGENTS.md 作为开发协作指令。",
        location: ToolLocation::File(".pi/agent/AGENTS.md"),
    },
    ToolSpec {
        id: "opencode",
        name: "OpenCode",
        description: "使用全局 AGENTS.md 作为开发协作指令。",
        location: ToolLocation::File(".config/opencode/AGENTS.md"),
    },
    ToolSpec {
        id: "gemini",
        name: "Gemini",
        description: "使用 GEMINI.md，也可与项目 AGENTS.md 一起生效。",
        location: ToolLocation::File(".gemini/GEMINI.md"),
    },
    ToolSpec {
        id: "claude",
        name: "Claude Code",
        description: "默认使用 CLAUDE.md 作为全局指令。",
        location: ToolLocation::File(".claude/CLAUDE.md"),
    },
    ToolSpec {
        id: "grok",
        name: "grok",
        description: "从 rules 目录中的多个 Markdown 规则文件读取提示词。",
        location: ToolLocation::GrokRules,
    },
];

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct PromptManagerConfig {
    #[serde(default)]
    global_enabled: bool,
    #[serde(default)]
    use_global: BTreeMap<String, bool>,
    #[serde(default)]
    backups: BTreeMap<String, BackupTarget>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
enum BackupTarget {
    Missing,
    File { content: String },
    Symlink { target: String },
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PromptManagerState {
    global: GlobalPromptState,
    tools: Vec<PromptToolState>,
    omp_note: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct GlobalPromptState {
    enabled: bool,
    path: String,
    content: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct PromptToolState {
    id: String,
    name: String,
    description: String,
    path: String,
    kind: String,
    exists: bool,
    uses_global: bool,
    status: String,
    message: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolPromptContent {
    content: String,
    read_only: bool,
    exists: bool,
    files: Vec<PromptFile>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct PromptFile {
    name: String,
}

#[derive(Clone)]
struct PromptStore {
    home_dir: PathBuf,
    manager_dir: PathBuf,
}

impl PromptStore {
    fn from_app(app: &tauri::AppHandle) -> Result<Self, String> {
        let home_dir = home_dir()?;
        let app_data_dir = app
            .path()
            .app_data_dir()
            .map_err(|error| format!("无法定位应用数据目录：{error}"))?;

        Ok(Self {
            home_dir,
            manager_dir: app_data_dir.join("prompt-manager"),
        })
    }

    fn global_path(&self) -> PathBuf {
        self.manager_dir.join(GLOBAL_FILE_NAME)
    }

    fn state_path(&self) -> PathBuf {
        self.manager_dir.join(STATE_FILE_NAME)
    }

    fn ensure_global_file(&self) -> Result<(), String> {
        fs::create_dir_all(&self.manager_dir).map_err(|error| io_error("创建提示词目录", error))?;
        let global_path = self.global_path();
        if !global_path.exists() {
            write_text_atomic(&global_path, "")?;
        }
        Ok(())
    }

    fn load_config(&self) -> Result<PromptManagerConfig, String> {
        match fs::read_to_string(self.state_path()) {
            Ok(contents) => serde_json::from_str(&contents)
                .map_err(|error| format!("提示词管理状态文件格式无效：{error}")),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(PromptManagerConfig::default()),
            Err(error) => Err(io_error("读取提示词管理状态", error)),
        }
    }

    fn save_config(&self, config: &PromptManagerConfig) -> Result<(), String> {
        fs::create_dir_all(&self.manager_dir).map_err(|error| io_error("创建提示词目录", error))?;
        let content = serde_json::to_string_pretty(config)
            .map_err(|error| format!("保存提示词管理状态失败：{error}"))?;
        write_text_atomic(&self.state_path(), &content)
    }

    fn target_path(&self, spec: ToolSpec) -> PathBuf {
        match spec.location {
            ToolLocation::File(relative_path) => self.home_dir.join(relative_path),
            ToolLocation::GrokRules => self
                .home_dir
                .join(".grok/rules")
                .join(GROK_GLOBAL_FILE_NAME),
        }
    }

    fn display_path(&self, spec: ToolSpec) -> String {
        match spec.location {
            ToolLocation::File(relative_path) => format!("~/{relative_path}"),
            ToolLocation::GrokRules => "~/.grok/rules/*.md".to_string(),
        }
    }

    fn global_enabled_for(&self, config: &PromptManagerConfig, spec: ToolSpec) -> bool {
        config.global_enabled && config.use_global.get(spec.id).copied().unwrap_or(true)
    }

    fn build_state(&self) -> Result<PromptManagerState, String> {
        self.ensure_global_file()?;
        let config = self.load_config()?;
        let global_path = self.global_path();
        let tools = TOOL_SPECS
            .iter()
            .copied()
            .map(|spec| {
                let target_path = self.target_path(spec);
                let desired_global = self.global_enabled_for(&config, spec);
                let is_shared = is_managed_link(&target_path, &global_path);
                let exists = prompt_target_exists(spec, &target_path);
                let (status, message) = if desired_global && is_shared {
                    ("shared".to_string(), "正在使用公共提示词".to_string())
                } else if desired_global {
                    (
                        "issue".to_string(),
                        "公共提示词关联已被外部修改，请重新读取后检查文件。".to_string(),
                    )
                } else if exists {
                    ("personal".to_string(), "正在使用专属提示词".to_string())
                } else {
                    ("missing".to_string(), "尚未创建提示词文件".to_string())
                };

                PromptToolState {
                    id: spec.id.to_string(),
                    name: spec.name.to_string(),
                    description: spec.description.to_string(),
                    path: self.display_path(spec),
                    kind: match spec.location {
                        ToolLocation::File(_) => "file".to_string(),
                        ToolLocation::GrokRules => "rules".to_string(),
                    },
                    exists,
                    uses_global: desired_global && is_shared,
                    status,
                    message,
                }
            })
            .collect();

        Ok(PromptManagerState {
            global: GlobalPromptState {
                enabled: config.global_enabled,
                path: display_path(&self.global_path(), &self.home_dir),
                content: read_utf8_file(&self.global_path(), "读取公共提示词")?,
            },
            tools,
            omp_note: "omp 会聚合读取其他开发工具的 AGENTS.md 等规则文件，因此不单独管理提示词。"
                .to_string(),
        })
    }

    fn read_tool_prompt(&self, tool_id: &str, file_name: Option<&str>) -> Result<ToolPromptContent, String> {
        self.ensure_global_file()?;
        let spec = tool_spec(tool_id)?;
        let config = self.load_config()?;
        let target_path = self.target_path(spec);
        let global_path = self.global_path();
        let shared = self.global_enabled_for(&config, spec) && is_managed_link(&target_path, &global_path);
        let files = self.rule_files(spec)?;

        if shared {
            return Ok(ToolPromptContent {
                content: read_utf8_file(&global_path, "读取公共提示词")?,
                read_only: true,
                exists: true,
                files,
            });
        }

        let path = self.personal_prompt_path(spec, file_name)?;
        let exists = path.exists();
        let content = if exists {
            read_utf8_file(&path, "读取工具提示词")?
        } else {
            String::new()
        };
        Ok(ToolPromptContent {
            content,
            read_only: false,
            exists,
            files,
        })
    }

    fn save_global_prompt(&self, content: &str) -> Result<(), String> {
        self.ensure_global_file()?;
        write_text_atomic(&self.global_path(), content)
    }

    fn save_tool_prompt(
        &self,
        tool_id: &str,
        file_name: Option<&str>,
        content: &str,
    ) -> Result<(), String> {
        self.ensure_global_file()?;
        let spec = tool_spec(tool_id)?;
        let config = self.load_config()?;
        if self.global_enabled_for(&config, spec) && is_managed_link(&self.target_path(spec), &self.global_path()) {
            return Err("该工具正在使用公共提示词，请先停止共享后再编辑专属提示词。".to_string());
        }

        let path = self.personal_prompt_path(spec, file_name)?;
        ensure_regular_file_target(&path)?;
        write_text_atomic(&path, content)
    }

    fn set_global_enabled(&self, enabled: bool) -> Result<(), String> {
        self.ensure_global_file()?;
        let mut config = self.load_config()?;
        if config.global_enabled == enabled {
            return Ok(());
        }

        if enabled {
            let selected = TOOL_SPECS
                .iter()
                .copied()
                .filter(|spec| config.use_global.get(spec.id).copied().unwrap_or(true))
                .collect::<Vec<_>>();
            let backups = capture_backups(self, &selected)?;
            let mut linked: Vec<ToolSpec> = Vec::new();
            for spec in &selected {
                let target_path = self.target_path(*spec);
                if let Err(error) = replace_with_link(&target_path, &self.global_path()) {
                    for restored in linked {
                        if let Some(backup) = backups.get(restored.id) {
                            let _ = restore_backup(&self.target_path(restored), backup);
                        }
                    }
                    return Err(error);
                }
                linked.push(*spec);
            }
            config.backups.extend(backups);
            config.global_enabled = true;
            self.save_config(&config)
        } else {
            let selected = TOOL_SPECS
                .iter()
                .copied()
                .filter(|spec| config.use_global.get(spec.id).copied().unwrap_or(true))
                .collect::<Vec<_>>();
            for spec in &selected {
                let target_path = self.target_path(*spec);
                if !is_managed_link(&target_path, &self.global_path()) {
                    return Err(format!(
                        "{} 的公共提示词关联已被外部修改，已停止自动恢复以保护当前文件。",
                        spec.name
                    ));
                }
            }
            for spec in &selected {
                let backup = config.backups.get(spec.id).ok_or_else(|| {
                    format!("找不到 {} 的原始提示词备份，已停止恢复。", spec.name)
                })?;
                restore_backup(&self.target_path(*spec), backup)?;
            }
            config.global_enabled = false;
            self.save_config(&config)
        }
    }

    fn set_tool_global_enabled(&self, tool_id: &str, enabled: bool) -> Result<(), String> {
        self.ensure_global_file()?;
        let spec = tool_spec(tool_id)?;
        let mut config = self.load_config()?;
        if !config.global_enabled {
            return Err("请先开启公共提示词，再设置单个工具的共享状态。".to_string());
        }

        let target_path = self.target_path(spec);
        let global_path = self.global_path();
        let currently_shared = config.use_global.get(spec.id).copied().unwrap_or(true);
        if currently_shared == enabled {
            return Ok(());
        }

        if enabled {
            let backup = capture_backup(&target_path)?;
            replace_with_link(&target_path, &global_path)?;
            config.backups.insert(spec.id.to_string(), backup);
            config.use_global.insert(spec.id.to_string(), true);
        } else {
            if !is_managed_link(&target_path, &global_path) {
                return Err("公共提示词关联已被外部修改，已停止自动恢复以保护当前文件。".to_string());
            }
            let backup = config.backups.get(spec.id).ok_or_else(|| {
                "找不到此工具的原始提示词备份，已停止恢复。".to_string()
            })?;
            restore_backup(&target_path, backup)?;
            config.use_global.insert(spec.id.to_string(), false);
        }
        self.save_config(&config)
    }

    fn personal_prompt_path(&self, spec: ToolSpec, file_name: Option<&str>) -> Result<PathBuf, String> {
        match spec.location {
            ToolLocation::File(relative_path) => Ok(self.home_dir.join(relative_path)),
            ToolLocation::GrokRules => {
                let file_name = file_name.unwrap_or(GROK_PERSONAL_FILE_NAME);
                validate_rule_file_name(file_name)?;
                Ok(self.home_dir.join(".grok/rules").join(file_name))
            }
        }
    }

    fn rule_files(&self, spec: ToolSpec) -> Result<Vec<PromptFile>, String> {
        if !matches!(spec.location, ToolLocation::GrokRules) {
            return Ok(Vec::new());
        }
        let rules_dir = self.home_dir.join(".grok/rules");
        let entries = match fs::read_dir(&rules_dir) {
            Ok(entries) => entries,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(io_error("读取 grok 规则目录", error)),
        };
        let mut files = entries
            .filter_map(Result::ok)
            .filter_map(|entry| {
                let file_name = entry.file_name().to_string_lossy().to_string();
                (file_name.ends_with(".md")).then_some(PromptFile { name: file_name })
            })
            .collect::<Vec<_>>();
        files.sort_by(|left, right| left.name.cmp(&right.name));
        Ok(files)
    }
}

#[tauri::command]
pub async fn get_prompt_manager_state(app: tauri::AppHandle) -> Result<PromptManagerState, String> {
    tauri::async_runtime::spawn_blocking(move || PromptStore::from_app(&app)?.build_state())
        .await
        .map_err(|error| format!("读取提示词管理状态异常：{error}"))?
}

#[tauri::command]
pub async fn read_tool_prompt(
    app: tauri::AppHandle,
    tool_id: String,
    file_name: Option<String>,
) -> Result<ToolPromptContent, String> {
    tauri::async_runtime::spawn_blocking(move || {
        PromptStore::from_app(&app)?.read_tool_prompt(&tool_id, file_name.as_deref())
    })
    .await
    .map_err(|error| format!("读取工具提示词异常：{error}"))?
}

#[tauri::command]
pub async fn save_global_prompt(app: tauri::AppHandle, content: String) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || PromptStore::from_app(&app)?.save_global_prompt(&content))
        .await
        .map_err(|error| format!("保存公共提示词异常：{error}"))?
}

#[tauri::command]
pub async fn save_tool_prompt(
    app: tauri::AppHandle,
    tool_id: String,
    file_name: Option<String>,
    content: String,
) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        PromptStore::from_app(&app)?.save_tool_prompt(&tool_id, file_name.as_deref(), &content)
    })
    .await
    .map_err(|error| format!("保存工具提示词异常：{error}"))?
}

#[tauri::command]
pub async fn set_global_prompt_enabled(app: tauri::AppHandle, enabled: bool) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || PromptStore::from_app(&app)?.set_global_enabled(enabled))
        .await
        .map_err(|error| format!("切换公共提示词异常：{error}"))?
}

#[tauri::command]
pub async fn set_tool_global_prompt_enabled(
    app: tauri::AppHandle,
    tool_id: String,
    enabled: bool,
) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        PromptStore::from_app(&app)?.set_tool_global_enabled(&tool_id, enabled)
    })
    .await
    .map_err(|error| format!("切换工具共享状态异常：{error}"))?
}

fn capture_backups(store: &PromptStore, specs: &[ToolSpec]) -> Result<BTreeMap<String, BackupTarget>, String> {
    specs
        .iter()
        .map(|spec| Ok((spec.id.to_string(), capture_backup(&store.target_path(*spec))?)))
        .collect()
}

fn capture_backup(path: &Path) -> Result<BackupTarget, String> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            let target = fs::read_link(path).map_err(|error| io_error("读取原始提示词链接", error))?;
            Ok(BackupTarget::Symlink {
                target: target.to_string_lossy().to_string(),
            })
        }
        Ok(metadata) if metadata.is_file() => Ok(BackupTarget::File {
            content: read_utf8_file(path, "读取原始提示词")?,
        }),
        Ok(_) => Err(format!("提示词路径不是普通文件：{}", path.display())),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(BackupTarget::Missing),
        Err(error) => Err(io_error("检查提示词路径", error)),
    }
}

fn restore_backup(path: &Path, backup: &BackupTarget) -> Result<(), String> {
    remove_file_or_link(path)?;
    match backup {
        BackupTarget::Missing => Ok(()),
        BackupTarget::File { content } => write_text_atomic(path, content),
        BackupTarget::Symlink { target } => {
            let parent = path
                .parent()
                .ok_or_else(|| "无法定位提示词文件父目录".to_string())?;
            fs::create_dir_all(parent).map_err(|error| io_error("创建提示词目录", error))?;
            symlink(target, path).map_err(|error| io_error("恢复原始提示词链接", error))
        }
    }
}

fn replace_with_link(path: &Path, global_path: &Path) -> Result<(), String> {
    remove_file_or_link(path)?;
    let parent = path
        .parent()
        .ok_or_else(|| "无法定位提示词文件父目录".to_string())?;
    fs::create_dir_all(parent).map_err(|error| io_error("创建提示词目录", error))?;
    symlink(global_path, path).map_err(|error| io_error("关联公共提示词", error))
}

fn remove_file_or_link(path: &Path) -> Result<(), String> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_file() || metadata.file_type().is_symlink() => {
            fs::remove_file(path).map_err(|error| io_error("替换提示词文件", error))
        }
        Ok(_) => Err(format!("提示词路径不是普通文件：{}", path.display())),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(io_error("检查提示词文件", error)),
    }
}

fn ensure_regular_file_target(path: &Path) -> Result<(), String> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_file() => Ok(()),
        Ok(metadata) if metadata.file_type().is_symlink() => Err("提示词文件仍是链接，请先重新读取并检查共享状态。".to_string()),
        Ok(_) => Err(format!("提示词路径不是普通文件：{}", path.display())),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(io_error("检查提示词文件", error)),
    }
}

fn is_managed_link(path: &Path, global_path: &Path) -> bool {
    fs::read_link(path).is_ok_and(|target| target == global_path)
}

fn prompt_target_exists(spec: ToolSpec, target_path: &Path) -> bool {
    match spec.location {
        ToolLocation::File(_) => fs::symlink_metadata(target_path).is_ok(),
        ToolLocation::GrokRules => target_path.parent().is_some_and(Path::exists),
    }
}

fn read_utf8_file(path: &Path, action: &str) -> Result<String, String> {
    fs::read_to_string(path).map_err(|error| io_error(action, error))
}

fn write_text_atomic(path: &Path, content: &str) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| "无法定位提示词文件父目录".to_string())?;
    fs::create_dir_all(parent).map_err(|error| io_error("创建提示词目录", error))?;
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    let temporary_path = parent.join(format!(".{}.{}.tmp", path.file_name().unwrap_or_default().to_string_lossy(), unique));
    fs::write(&temporary_path, content).map_err(|error| io_error("写入提示词", error))?;
    fs::rename(&temporary_path, path).map_err(|error| io_error("保存提示词", error))
}

fn tool_spec(id: &str) -> Result<ToolSpec, String> {
    TOOL_SPECS
        .iter()
        .copied()
        .find(|spec| spec.id == id)
        .ok_or_else(|| "未知开发工具，操作已拒绝。".to_string())
}

fn validate_rule_file_name(file_name: &str) -> Result<(), String> {
    let path = Path::new(file_name);
    if file_name.is_empty()
        || !file_name.ends_with(".md")
        || path.file_name().is_none_or(|name| name != file_name)
    {
        return Err("grok 规则文件名无效。".to_string());
    }
    Ok(())
}

fn home_dir() -> Result<PathBuf, String> {
    env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or_else(|| "无法确定当前用户目录。".to_string())
}

fn display_path(path: &Path, home: &Path) -> String {
    path.strip_prefix(home)
        .map(|relative| format!("~/{}", relative.display()))
        .unwrap_or_else(|_| path.display().to_string())
}

fn io_error(action: &str, error: io::Error) -> String {
    format!("{action}失败：{error}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_store(label: &str) -> PromptStore {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = env::temp_dir().join(format!("dev-cache-cleaner-prompt-{label}-{unique}"));
        PromptStore {
            home_dir: root.join("home"),
            manager_dir: root.join("app-data/prompt-manager"),
        }
    }

    fn cleanup(store: &PromptStore) {
        let root = store
            .manager_dir
            .ancestors()
            .nth(2)
            .expect("test root")
            .to_path_buf();
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn global_enable_and_disable_restore_existing_file() {
        let store = test_store("restore-file");
        let codex = tool_spec("codex").unwrap();
        let path = store.target_path(codex);
        write_text_atomic(&path, "original rules").unwrap();

        store.set_global_enabled(true).unwrap();
        assert!(is_managed_link(&path, &store.global_path()));

        store.set_global_enabled(false).unwrap();
        assert_eq!(read_utf8_file(&path, "test").unwrap(), "original rules");
        cleanup(&store);
    }

    #[test]
    fn tool_can_leave_and_rejoin_global_prompt() {
        let store = test_store("tool-toggle");
        store.set_global_enabled(true).unwrap();
        let pi = tool_spec("pi").unwrap();
        let path = store.target_path(pi);
        assert!(is_managed_link(&path, &store.global_path()));

        store.set_tool_global_enabled("pi", false).unwrap();
        assert!(!path.exists());
        store.save_tool_prompt("pi", None, "pi only").unwrap();
        store.set_tool_global_enabled("pi", true).unwrap();
        store.set_tool_global_enabled("pi", false).unwrap();
        assert_eq!(read_utf8_file(&path, "test").unwrap(), "pi only");
        cleanup(&store);
    }

    #[test]
    fn grok_global_rule_does_not_touch_other_rule_files() {
        let store = test_store("grok-isolated");
        let rules_dir = store.home_dir.join(".grok/rules");
        write_text_atomic(&rules_dir.join("existing.md"), "keep this").unwrap();

        store.set_global_enabled(true).unwrap();
        let grok = tool_spec("grok").unwrap();
        assert!(is_managed_link(&store.target_path(grok), &store.global_path()));
        assert_eq!(read_utf8_file(&rules_dir.join("existing.md"), "test").unwrap(), "keep this");

        store.set_global_enabled(false).unwrap();
        assert!(!rules_dir.join(GROK_GLOBAL_FILE_NAME).exists());
        assert_eq!(read_utf8_file(&rules_dir.join("existing.md"), "test").unwrap(), "keep this");
        cleanup(&store);
    }

    #[test]
    fn directories_and_unsafe_grok_filenames_are_rejected() {
        let store = test_store("safety");
        let codex_path = store.target_path(tool_spec("codex").unwrap());
        fs::create_dir_all(&codex_path).unwrap();
        assert!(store.set_global_enabled(true).is_err());
        assert!(validate_rule_file_name("../outside.md").is_err());
        assert!(validate_rule_file_name("rules.txt").is_err());
        cleanup(&store);
    }
}
