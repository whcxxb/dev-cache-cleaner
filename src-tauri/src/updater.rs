use rayon::prelude::*;
use serde::{Deserialize, Serialize};
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
    latest_source: LatestSource,
}

const TOOLS: [ToolSpec; 7] = [
    ToolSpec { id: "codex", name: "Codex", command: "codex", latest_source: LatestSource::Npm("@openai/codex") },
    ToolSpec { id: "pi", name: "pi", command: "pi", latest_source: LatestSource::Npm("@earendil-works/pi-coding-agent") },
    ToolSpec { id: "omp", name: "omp", command: "omp", latest_source: LatestSource::Omp },
    ToolSpec { id: "opencode", name: "OpenCode", command: "opencode", latest_source: LatestSource::Npm("opencode-ai") },
    ToolSpec { id: "gemini", name: "Gemini", command: "gemini", latest_source: LatestSource::Npm("@google/gemini-cli") },
    ToolSpec { id: "claude", name: "Claude Code", command: "claude", latest_source: LatestSource::Npm("@anthropic-ai/claude-code") },
    ToolSpec { id: "grok", name: "grok", command: "grok", latest_source: LatestSource::Grok },
];

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolUpdateInfo {
    id: String,
    name: String,
    installed: bool,
    current_version: Option<String>,
    latest_version: Option<String>,
    status: String,
    message: String,
    source: String,
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
                message: if message == "命令不存在" { "当前机器未安装".to_string() } else { message },
                source: source_label(spec.latest_source).to_string(),
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
        Ok(output) => build_info(spec, current_version, first_version(&output), "npm registry", None),
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
            Err(_) => build_info(spec, fallback_current, None, "grok 更新检查", Some("无法解析更新检查结果".to_string())),
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
    let (status, message) = match (current_version.as_deref(), latest_version.as_deref(), error) {
        (_, _, Some(error)) => ("unavailable".to_string(), format!("检查失败：{error}")),
        (Some(current), Some(latest), None) if current == latest => ("latest".to_string(), "已是最新版本".to_string()),
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
    }
}

fn run(command: &str, args: &[&str]) -> Result<String, String> {
    let output = Command::new(command).args(args).output().map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound { "命令不存在".to_string() } else { error.to_string() }
    })?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_string());
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn first_version(input: &str) -> Option<String> {
    input.split(|character: char| !(character.is_ascii_alphanumeric() || character == '.' || character == '-'))
        .find(|part| part.chars().next().is_some_and(|character| character.is_ascii_digit()) && part.matches('.').count() >= 2)
        .map(|part| part.trim_start_matches('v').to_string())
}

fn version_after(input: &str, label: &str) -> Option<String> {
    input.lines().find_map(|line| line.strip_prefix(label).map(str::trim).and_then(first_version))
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
    use super::{first_version, version_after};

    #[test]
    fn parses_cli_and_update_versions() {
        assert_eq!(first_version("codex-cli 0.147.0"), Some("0.147.0".to_string()));
        assert_eq!(first_version("omp/17.3.7"), Some("17.3.7".to_string()));
        assert_eq!(version_after("Current version: 17.3.7\nNew version available: 17.4.0", "New version available:"), Some("17.4.0".to_string()));
    }
}
