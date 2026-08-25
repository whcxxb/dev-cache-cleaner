use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;
use std::process::Command;

#[derive(Clone, Copy)]
enum LatestSource {
    Npm(&'static str),
    Omp,
    Grok,
}

#[derive(Clone, Copy)]
struct ToolSpec {
    id: &'static str,
    name: &'static str,
    command: &'static str,
    npm_package: Option<&'static str>,
    latest_source: LatestSource,
}

const TOOLS: [ToolSpec; 7] = [
    ToolSpec {
        id: "codex",
        name: "Codex",
        command: "codex",
        npm_package: Some("@openai/codex"),
        latest_source: LatestSource::Npm("@openai/codex"),
    },
    ToolSpec {
        id: "pi",
        name: "pi",
        command: "pi",
        npm_package: Some("@earendil-works/pi-coding-agent"),
        latest_source: LatestSource::Npm("@earendil-works/pi-coding-agent"),
    },
    ToolSpec {
        id: "omp",
        name: "omp",
        command: "omp",
        npm_package: None,
        latest_source: LatestSource::Omp,
    },
    ToolSpec {
        id: "opencode",
        name: "OpenCode",
        command: "opencode",
        npm_package: Some("opencode-ai"),
        latest_source: LatestSource::Npm("opencode-ai"),
    },
    ToolSpec {
        id: "gemini",
        name: "Gemini",
        command: "gemini",
        npm_package: Some("@google/gemini-cli"),
        latest_source: LatestSource::Npm("@google/gemini-cli"),
    },
    ToolSpec {
        id: "claude",
        name: "Claude Code",
        command: "claude",
        npm_package: Some("@anthropic-ai/claude-code"),
        latest_source: LatestSource::Npm("@anthropic-ai/claude-code"),
    },
    ToolSpec {
        id: "grok",
        name: "grok",
        command: "grok",
        npm_package: None,
        latest_source: LatestSource::Grok,
    },
];

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolUpdateInfo {
    pub id: String,
    pub name: String,
    pub installed: bool,
    pub current_version: Option<String>,
    pub latest_version: Option<String>,
    pub status: String,
    pub message: String,
    pub source: String,
    pub install_method: Option<String>,
    pub upgrade_command: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolUpgradeResult {
    pub id: String,
    pub name: String,
    pub success: bool,
    pub message: String,
    pub previous_version: Option<String>,
    pub current_version: Option<String>,
    pub upgrade_command: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InstallMethod {
    Pnpm { package: String },
    Npm { package: String },
    Bun { package: String },
    Yarn { package: String },
    Homebrew { formula: String },
    Cargo { crate_name: String },
    SelfUpdate { command: String, args: Vec<String> },
    Unknown,
}

impl InstallMethod {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Pnpm { .. } => "pnpm 全局包",
            Self::Npm { .. } => "npm 全局包",
            Self::Bun { .. } => "Bun 全局包",
            Self::Yarn { .. } => "Yarn 全局包",
            Self::Homebrew { .. } => "Homebrew",
            Self::Cargo { .. } => "Cargo",
            Self::SelfUpdate { .. } => "原生安装",
            Self::Unknown => "系统命令",
        }
    }

    pub fn command_and_args(&self) -> Option<(String, Vec<String>)> {
        match self {
            Self::Pnpm { package } => Some((
                "pnpm".to_string(),
                vec!["add".into(), "-g".into(), format!("{package}@latest")],
            )),
            Self::Npm { package } => Some((
                "npm".to_string(),
                vec!["install".into(), "-g".into(), format!("{package}@latest")],
            )),
            Self::Bun { package } => Some((
                "bun".to_string(),
                vec!["add".into(), "-g".into(), format!("{package}@latest")],
            )),
            Self::Yarn { package } => Some((
                "yarn".to_string(),
                vec!["global".into(), "add".into(), format!("{package}@latest")],
            )),
            Self::Homebrew { formula } => {
                Some(("brew".to_string(), vec!["upgrade".into(), formula.clone()]))
            }
            Self::Cargo { crate_name } => Some((
                "cargo".to_string(),
                vec!["install".into(), crate_name.clone()],
            )),
            Self::SelfUpdate { command, args } => Some((command.clone(), args.clone())),
            Self::Unknown => None,
        }
    }

    pub fn command_display(&self) -> Option<String> {
        self.command_and_args().map(|(cmd, args)| {
            if args.is_empty() {
                cmd
            } else {
                format!("{cmd} {}", args.join(" "))
            }
        })
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GrokCheck {
    current_version: String,
    latest_version: String,
    update_available: bool,
    error: Option<String>,
}

#[tauri::command]
pub async fn scan_tool_updates() -> Result<Vec<ToolUpdateInfo>, String> {
    tauri::async_runtime::spawn_blocking(|| TOOLS.par_iter().map(scan_one).collect())
        .await
        .map_err(|error| format!("检查工具更新异常：{error}"))
}

#[tauri::command]
pub async fn upgrade_tool(tool_id: String) -> Result<ToolUpgradeResult, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let spec = TOOLS
            .iter()
            .find(|s| s.id == tool_id)
            .ok_or_else(|| format!("未找到工具：{tool_id}"))?;
        execute_upgrade(spec)
    })
    .await
    .map_err(|error| format!("升级工具异常：{error}"))?
}

#[tauri::command]
pub async fn upgrade_tools(tool_ids: Vec<String>) -> Result<Vec<ToolUpgradeResult>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let mut results = Vec::new();
        for id in tool_ids {
            if let Some(spec) = TOOLS.iter().find(|s| s.id == id) {
                match execute_upgrade(spec) {
                    Ok(res) => results.push(res),
                    Err(err) => results.push(ToolUpgradeResult {
                        id: spec.id.to_string(),
                        name: spec.name.to_string(),
                        success: false,
                        message: err,
                        previous_version: None,
                        current_version: None,
                        upgrade_command: String::new(),
                    }),
                }
            }
        }
        Ok(results)
    })
    .await
    .map_err(|error| format!("批量升级工具异常：{error}"))?
}

fn execute_upgrade(spec: &ToolSpec) -> Result<ToolUpgradeResult, String> {
    let method = detect_install_method(spec);
    let (cmd, args) = method.command_and_args().ok_or_else(|| {
        format!("无法确定 {} 的安装来源或升级方式", spec.name)
    })?;

    let upgrade_command_str = if args.is_empty() {
        cmd.clone()
    } else {
        format!("{cmd} {}", args.join(" "))
    };

    let pre_info = scan_one(spec);
    let previous_version = pre_info.current_version;

    let arg_refs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
    let run_res = run(&cmd, &arg_refs);

    let post_info = scan_one(spec);
    let current_version = post_info.current_version;

    match run_res {
        Ok(_) => {
            let msg = if let (Some(prev), Some(curr)) = (&previous_version, &current_version) {
                if prev != curr {
                    format!("已成功从 {prev} 升级至 {curr}")
                } else {
                    format!("执行升级完成，当前版本为 {curr}")
                }
            } else if let Some(curr) = &current_version {
                format!("升级完成，当前版本为 {curr}")
            } else {
                "升级命令执行成功".to_string()
            };

            Ok(ToolUpgradeResult {
                id: spec.id.to_string(),
                name: spec.name.to_string(),
                success: true,
                message: msg,
                previous_version,
                current_version,
                upgrade_command: upgrade_command_str,
            })
        }
        Err(err) => {
            if previous_version.is_some()
                && current_version.is_some()
                && previous_version != current_version
            {
                let prev = previous_version.as_deref().unwrap_or("");
                let curr = current_version.as_deref().unwrap_or("");
                return Ok(ToolUpgradeResult {
                    id: spec.id.to_string(),
                    name: spec.name.to_string(),
                    success: true,
                    message: format!("已成功从 {prev} 升级至 {curr}"),
                    previous_version,
                    current_version,
                    upgrade_command: upgrade_command_str,
                });
            }

            Ok(ToolUpgradeResult {
                id: spec.id.to_string(),
                name: spec.name.to_string(),
                success: false,
                message: format!("升级失败：{err}"),
                previous_version,
                current_version,
                upgrade_command: upgrade_command_str,
            })
        }
    }
}

fn detect_install_method(spec: &ToolSpec) -> InstallMethod {
    let raw_path = match resolve_executable_path(spec.command) {
        Some(path) => path,
        None => return InstallMethod::Unknown,
    };

    let canonical_path = fs::canonicalize(&raw_path).unwrap_or_else(|_| raw_path.clone());
    let raw_str = raw_path.to_string_lossy();
    let canon_str = canonical_path.to_string_lossy();

    // 1. Homebrew Check
    if (canon_str.contains("/Cellar/")
        || canon_str.contains("/opt/homebrew/")
        || raw_str.contains("/opt/homebrew/")
        || canon_str.contains("/usr/local/Cellar/"))
        && !canon_str.contains("/node_modules/")
        && !canon_str.contains("/Library/pnpm/")
    {
        return InstallMethod::Homebrew {
            formula: spec.command.to_string(),
        };
    }

    // 2. pnpm Global Check
    if raw_str.contains("Library/pnpm")
        || raw_str.contains(".pnpm-global")
        || raw_str.contains("pnpm/global")
        || raw_str.contains("pnpm/bin")
        || canon_str.contains("Library/pnpm")
        || canon_str.contains(".pnpm-global")
        || canon_str.contains("node_modules/.pnpm")
    {
        if let Some(pkg) = spec.npm_package {
            return InstallMethod::Pnpm {
                package: pkg.to_string(),
            };
        }
    }

    // 3. Bun Global Check
    if raw_str.contains(".bun/bin") || canon_str.contains(".bun/install") {
        if let Some(pkg) = spec.npm_package {
            return InstallMethod::Bun {
                package: pkg.to_string(),
            };
        }
    }

    // 4. Yarn Global Check
    if raw_str.contains(".yarn/bin")
        || raw_str.contains("yarn/global")
        || canon_str.contains(".config/yarn/global")
    {
        if let Some(pkg) = spec.npm_package {
            return InstallMethod::Yarn {
                package: pkg.to_string(),
            };
        }
    }

    // 5. npm Global Check
    if canon_str.contains("/node_modules/")
        || canon_str.contains("/lib/node_modules/")
        || raw_str.contains(".nvm/")
        || raw_str.contains(".fnm/")
        || raw_str.contains(".nodenv/")
    {
        if let Some(pkg) = spec.npm_package {
            return InstallMethod::Npm {
                package: pkg.to_string(),
            };
        }
    }

    // 6. Cargo Check
    if raw_str.contains(".cargo/bin") || canon_str.contains(".cargo/bin") {
        return InstallMethod::Cargo {
            crate_name: spec.command.to_string(),
        };
    }

    // 7. Tool Native Updater / Standalone directory
    if spec.id == "omp" {
        return InstallMethod::SelfUpdate {
            command: "omp".to_string(),
            args: vec!["update".to_string()],
        };
    }
    if spec.id == "grok" {
        return InstallMethod::SelfUpdate {
            command: "grok".to_string(),
            args: vec!["update".to_string()],
        };
    }
    if spec.id == "codex"
        && (canon_str.contains(".codex/packages/standalone") || canon_str.contains(".codex/"))
    {
        return InstallMethod::SelfUpdate {
            command: "codex".to_string(),
            args: vec!["update".to_string()],
        };
    }
    if spec.id == "opencode"
        && (canon_str.contains(".opencode/bin") || canon_str.contains(".opencode/"))
    {
        return InstallMethod::SelfUpdate {
            command: "opencode".to_string(),
            args: vec!["upgrade".to_string()],
        };
    }
    if spec.id == "pi" && !canon_str.contains("node_modules") {
        return InstallMethod::SelfUpdate {
            command: "pi".to_string(),
            args: vec!["update".to_string()],
        };
    }
    if spec.id == "claude" && !canon_str.contains("node_modules") && !raw_str.contains("pnpm") {
        return InstallMethod::SelfUpdate {
            command: "claude".to_string(),
            args: vec!["update".to_string()],
        };
    }

    // 8. Fallback for NPM packages
    if let Some(pkg) = spec.npm_package {
        if resolve_executable_path("pnpm").is_some() {
            return InstallMethod::Pnpm {
                package: pkg.to_string(),
            };
        }
        return InstallMethod::Npm {
            package: pkg.to_string(),
        };
    }

    InstallMethod::Unknown
}

fn resolve_executable_path(command: &str) -> Option<PathBuf> {
    if let Ok(output) = run("which", &[command]) {
        let trimmed = output.trim();
        if !trimmed.is_empty() {
            let path = PathBuf::from(trimmed);
            if path.exists() {
                return Some(path);
            }
        }
    }
    if let Some(path_var) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&path_var) {
            let candidate = dir.join(command);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

fn scan_one(spec: &ToolSpec) -> ToolUpdateInfo {
    let version_output = match run(spec.command, &["--version"]) {
        Ok(output) => output,
        Err(message) => {
            return ToolUpdateInfo {
                id: spec.id.to_string(),
                name: spec.name.to_string(),
                installed: false,
                current_version: None,
                latest_version: None,
                status: "notInstalled".to_string(),
                message: if message == "命令不存在" {
                    "当前机器未安装".to_string()
                } else {
                    message
                },
                source: source_label(spec.latest_source).to_string(),
                install_method: None,
                upgrade_command: None,
            };
        }
    };

    let current_version = first_version(&version_output);
    match spec.latest_source {
        LatestSource::Npm(package) => from_npm(spec, current_version, package),
        LatestSource::Omp => from_omp(spec, current_version),
        LatestSource::Grok => from_grok(spec, current_version),
    }
}

fn from_npm(spec: &ToolSpec, current_version: Option<String>, package: &str) -> ToolUpdateInfo {
    match run("pnpm", &["view", package, "version", "--json"]) {
        Ok(output) => {
            build_info(spec, current_version, first_version(&output), "npm registry", None)
        }
        Err(error) => build_info(spec, current_version, None, "npm registry", Some(error)),
    }
}

fn from_omp(spec: &ToolSpec, fallback_current: Option<String>) -> ToolUpdateInfo {
    let output = run("omp", &["update", "--check"]);
    match output {
        Ok(output) => {
            let current = version_after(&output, "Current version:").or(fallback_current);
            let latest = version_after(&output, "New version available:").or_else(|| current.clone());
            build_info(spec, current, latest, "omp 更新检查", None)
        }
        Err(error) => build_info(spec, fallback_current, None, "omp 更新检查", Some(error)),
    }
}

fn from_grok(spec: &ToolSpec, fallback_current: Option<String>) -> ToolUpdateInfo {
    let output = run("grok", &["update", "--check", "--json"]);
    match output {
        Ok(output) => match serde_json::from_str::<GrokCheck>(&output) {
            Ok(result) if result.error.is_none() => {
                let latest_version = if result.update_available {
                    result.latest_version
                } else {
                    result.current_version.clone()
                };
                build_info(
                    spec,
                    Some(result.current_version),
                    Some(latest_version),
                    "grok 更新检查",
                    None,
                )
            }
            Ok(result) => build_info(spec, fallback_current, None, "grok 更新检查", result.error),
            Err(_) => build_info(
                spec,
                fallback_current,
                None,
                "grok 更新检查",
                Some("无法解析更新检查结果".to_string()),
            ),
        },
        Err(error) => build_info(spec, fallback_current, None, "grok 更新检查", Some(error)),
    }
}

fn build_info(
    spec: &ToolSpec,
    current_version: Option<String>,
    latest_version: Option<String>,
    source: &str,
    error: Option<String>,
) -> ToolUpdateInfo {
    let install_method = detect_install_method(spec);
    let (status, message) = match (current_version.as_deref(), latest_version.as_deref(), error) {
        (_, _, Some(error)) => ("unavailable".to_string(), format!("检查失败：{error}")),
        (Some(current), Some(latest), None) if current == latest => {
            ("latest".to_string(), "已是最新版本".to_string())
        }
        (Some(_), Some(_), None) => ("updateAvailable".to_string(), "发现可用更新".to_string()),
        _ => ("unavailable".to_string(), "无法识别当前或最新版本".to_string()),
    };
    ToolUpdateInfo {
        id: spec.id.to_string(),
        name: spec.name.to_string(),
        installed: true,
        current_version,
        latest_version,
        status,
        message,
        source: source.to_string(),
        install_method: Some(install_method.label().to_string()),
        upgrade_command: install_method.command_display(),
    }
}

fn run(command: &str, args: &[&str]) -> Result<String, String> {
    let output = Command::new(command).args(args).output().map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            "命令不存在".to_string()
        } else {
            error.to_string()
        }
    })?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_string());
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn first_version(input: &str) -> Option<String> {
    input
        .split(|character: char| {
            !(character.is_ascii_alphanumeric() || character == '.' || character == '-')
        })
        .find(|part| {
            part.chars().next().is_some_and(|character| character.is_ascii_digit())
                && part.matches('.').count() >= 2
        })
        .map(|part| part.trim_start_matches('v').to_string())
}

fn version_after(input: &str, label: &str) -> Option<String> {
    input
        .lines()
        .find_map(|line| line.strip_prefix(label).map(str::trim).and_then(first_version))
}

fn source_label(source: LatestSource) -> &'static str {
    match source {
        LatestSource::Npm(_) => "npm registry",
        LatestSource::Omp => "omp 更新检查",
        LatestSource::Grok => "grok 更新检查",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_cli_and_update_versions() {
        assert_eq!(first_version("codex-cli 0.147.0"), Some("0.147.0".to_string()));
        assert_eq!(first_version("omp/17.3.7"), Some("17.3.7".to_string()));
        assert_eq!(
            version_after(
                "Current version: 17.3.7\nNew version available: 17.4.0",
                "New version available:"
            ),
            Some("17.4.0".to_string())
        );
    }

    #[test]
    fn install_method_command_generation() {
        let pnpm = InstallMethod::Pnpm {
            package: "@anthropic-ai/claude-code".to_string(),
        };
        assert_eq!(pnpm.label(), "pnpm 全局包");
        assert_eq!(
            pnpm.command_display(),
            Some("pnpm add -g @anthropic-ai/claude-code@latest".to_string())
        );

        let npm = InstallMethod::Npm {
            package: "@earendil-works/pi-coding-agent".to_string(),
        };
        assert_eq!(npm.label(), "npm 全局包");
        assert_eq!(
            npm.command_display(),
            Some("npm install -g @earendil-works/pi-coding-agent@latest".to_string())
        );

        let omp_self = InstallMethod::SelfUpdate {
            command: "omp".to_string(),
            args: vec!["update".to_string()],
        };
        assert_eq!(omp_self.label(), "原生安装");
        assert_eq!(omp_self.command_display(), Some("omp update".to_string()));

        let brew = InstallMethod::Homebrew {
            formula: "codex".to_string(),
        };
        assert_eq!(brew.label(), "Homebrew");
        assert_eq!(brew.command_display(), Some("brew upgrade codex".to_string()));
    }
}
