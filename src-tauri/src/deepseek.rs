use serde::{Deserialize, Serialize};
use std::{
    fs,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
    time::Duration,
};
use tauri::{
    menu::{Menu, MenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    Emitter, Manager,
};

const CONFIG_FILE_NAME: &str = "deepseek-config.json";
const API_URL: &str = "https://api.deepseek.com/user/balance";

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct DeepSeekConfig {
    pub api_key: Option<String>,
    pub show_tray_balance: Option<bool>,
    pub auto_sync: Option<bool>,
    pub sync_interval_minutes: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AutoSyncConfig {
    pub enabled: bool,
    pub interval_minutes: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BalanceInfo {
    pub currency: String,
    pub total_balance: String,
    pub granted_balance: String,
    pub topped_up_balance: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DeepSeekBalanceResponse {
    pub is_available: bool,
    pub balance_infos: Vec<BalanceInfo>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DeepSeekBalanceResult {
    pub success: bool,
    pub is_available: bool,
    pub balance_infos: Vec<BalanceInfo>,
    pub updated_at: u64,
    pub error_message: Option<String>,
}


#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ApiKeyInfo {
    pub api_key: Option<String>,
    pub is_from_env: bool,
}
fn config_path(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    let dir = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("无法定位应用数据目录：{e}"))?;
    Ok(dir.join(CONFIG_FILE_NAME))
}

fn load_config_from_path(path: &PathBuf) -> DeepSeekConfig {
    if let Ok(content) = fs::read_to_string(path) {
        if let Ok(cfg) = serde_json::from_str::<DeepSeekConfig>(&content) {
            return cfg;
        }
    }
    DeepSeekConfig::default()
}

fn save_config_to_path(path: &PathBuf, config: &DeepSeekConfig) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("创建配置目录失败：{e}"))?;
    }
    let json = serde_json::to_string_pretty(config).map_err(|e| format!("序列化配置失败：{e}"))?;
    let temp_path = path.with_extension("tmp");
    fs::write(&temp_path, json).map_err(|e| format!("写入临时配置失败：{e}"))?;
    fs::rename(&temp_path, path).map_err(|e| format!("保存配置失败：{e}"))?;
    Ok(())
}

pub fn detect_env_api_key() -> Option<String> {
    if let Ok(key) = std::env::var("DEEPSEEK_API_KEY") {
        let trimmed = key.trim();
        if !trimmed.is_empty() {
            return Some(trimmed.to_string());
        }
    }

    if let Ok(home) = std::env::var("HOME") {
        let home_path = PathBuf::from(home);
        for profile in &[".zshrc", ".zprofile", ".bash_profile", ".bashrc", ".profile"] {
            let file = home_path.join(profile);
            if let Ok(contents) = fs::read_to_string(&file) {
                for line in contents.lines() {
                    let trimmed = line.trim();
                    if trimmed.starts_with('#') {
                        continue;
                    }
                    if let Some(rest) = trimmed
                        .strip_prefix("export DEEPSEEK_API_KEY=")
                        .or_else(|| trimmed.strip_prefix("DEEPSEEK_API_KEY="))
                    {
                        let unquoted = rest.trim().trim_matches('"').trim_matches('\'').trim();
                        if !unquoted.is_empty() {
                            return Some(unquoted.to_string());
                        }
                    }
                }
            }
        }
    }

    None
}

#[tauri::command]
pub fn get_deepseek_api_key(app: tauri::AppHandle) -> Result<ApiKeyInfo, String> {
    let path = config_path(&app)?;
    let cfg = load_config_from_path(&path);
    if let Some(key) = cfg.api_key {
        if !key.trim().is_empty() {
            return Ok(ApiKeyInfo {
                api_key: Some(key),
                is_from_env: false,
            });
        }
    }
    if let Some(env_key) = detect_env_api_key() {
        return Ok(ApiKeyInfo {
            api_key: Some(env_key),
            is_from_env: true,
        });
    }
    Ok(ApiKeyInfo {
        api_key: None,
        is_from_env: false,
    })
}

#[tauri::command]
pub fn save_deepseek_api_key(app: tauri::AppHandle, api_key: String) -> Result<(), String> {
    let path = config_path(&app)?;
    let trimmed = api_key.trim().to_string();
    let mut cfg = load_config_from_path(&path);
    cfg.api_key = if trimmed.is_empty() {
        None
    } else {
        Some(trimmed)
    };
    save_config_to_path(&path, &cfg)
}

#[tauri::command]
pub fn clear_deepseek_api_key(app: tauri::AppHandle) -> Result<(), String> {
    let path = config_path(&app)?;
    let mut cfg = load_config_from_path(&path);
    cfg.api_key = None;
    save_config_to_path(&path, &cfg)
}

pub const TRAY_ID: &str = "deepseek-tray";

pub fn update_tray_balance(
    app: &tauri::AppHandle,
    balance_str: Option<&str>,
    _is_available: bool,
) -> Result<(), String> {
    let path = config_path(app)?;
    let cfg = load_config_from_path(&path);
    let show_tray = cfg.show_tray_balance.unwrap_or(false);

    if !show_tray {
        if let Some(tray) = app.tray_by_id(TRAY_ID) {
            let _ = tray.set_visible(false);
        }
        return Ok(());
    }

    let title = match balance_str {
        Some(b) => format!("¥{}", b),
        None => "¥--.--".to_string(),
    };

    if let Some(tray) = app.tray_by_id(TRAY_ID) {
        let _ = tray.set_icon(None);
        let _ = tray.set_title(Some(&title));
        let _ = tray.set_visible(true);
    } else {
        let show_item = MenuItem::with_id(app, "show", "打开 DevTidy", true, None::<&str>)
            .map_err(|e| e.to_string())?;
        let refresh_item =
            MenuItem::with_id(app, "refresh", "刷新 DeepSeek 余额", true, None::<&str>)
                .map_err(|e| e.to_string())?;
        let quit_item = MenuItem::with_id(app, "quit", "退出应用", true, None::<&str>)
            .map_err(|e| e.to_string())?;
        let menu = Menu::with_items(app, &[&show_item, &refresh_item, &quit_item])
            .map_err(|e| e.to_string())?;

        let _ = TrayIconBuilder::with_id(TRAY_ID)
            .title(&title)
            .menu(&menu)
            .show_menu_on_left_click(false)
            .on_tray_icon_event(|tray, event| {
                if let TrayIconEvent::Click {
                    button: MouseButton::Left,
                    button_state: MouseButtonState::Up,
                    ..
                } = event
                {
                    let app = tray.app_handle();
                    if let Some(window) = app.get_webview_window("main") {
                        let _ = window.unminimize();
                        let _ = window.show();
                        let _ = window.set_focus();
                    }
                }
            })
            .on_menu_event(|app, event| match event.id.as_ref() {
                "show" => {
                    if let Some(window) = app.get_webview_window("main") {
                        let _ = window.unminimize();
                        let _ = window.show();
                        let _ = window.set_focus();
                    }
                }
                "refresh" => {
                    let app_clone = app.clone();
                    tauri::async_runtime::spawn(async move {
                        let _ = fetch_deepseek_balance(app_clone, None).await;
                    });
                }
                "quit" => {
                    app.exit(0);
                }
                _ => {}
            })
            .build(app)
            .map_err(|e| format!("创建状态栏托盘失败：{e}"))?;
    }

    Ok(())
}

#[tauri::command]
pub fn get_deepseek_tray_switch(app: tauri::AppHandle) -> Result<bool, String> {
    let path = config_path(&app)?;
    let cfg = load_config_from_path(&path);
    Ok(cfg.show_tray_balance.unwrap_or(false))
}

#[tauri::command]
pub async fn set_deepseek_tray_switch(
    app: tauri::AppHandle,
    enabled: bool,
) -> Result<bool, String> {
    let path = config_path(&app)?;
    let mut cfg = load_config_from_path(&path);
    cfg.show_tray_balance = Some(enabled);
    save_config_to_path(&path, &cfg)?;

    if enabled {
        let app_clone = app.clone();
        tauri::async_runtime::spawn(async move {
            let _ = fetch_deepseek_balance(app_clone, None).await;
        });
    } else {
        let _ = update_tray_balance(&app, None, false);
    }

    Ok(enabled)
}

static SYNC_WORKER_VERSION: AtomicU64 = AtomicU64::new(0);

pub fn start_auto_sync_worker(app: tauri::AppHandle) {
    let path = match config_path(&app) {
        Ok(p) => p,
        Err(_) => return,
    };
    let cfg = load_config_from_path(&path);
    let enabled = cfg.auto_sync.unwrap_or(false);
    let interval_minutes = match cfg.sync_interval_minutes.unwrap_or(30) {
        60 => 60,
        _ => 30,
    };

    let version = SYNC_WORKER_VERSION.fetch_add(1, Ordering::SeqCst) + 1;

    if !enabled {
        return;
    }

    tauri::async_runtime::spawn(async move {
        let sleep_interval = Duration::from_secs((interval_minutes as u64) * 60);
        loop {
            let mut elapsed = Duration::from_secs(0);
            while elapsed < sleep_interval {
                tokio::time::sleep(Duration::from_secs(5)).await;
                if SYNC_WORKER_VERSION.load(Ordering::SeqCst) != version {
                    return;
                }
                elapsed += Duration::from_secs(5);
            }

            if SYNC_WORKER_VERSION.load(Ordering::SeqCst) != version {
                return;
            }

            if let Ok(res) = fetch_deepseek_balance(app.clone(), None).await {
                let _ = app.emit("deepseek-balance-updated", &res);
            }
        }
    });
}

#[tauri::command]
pub fn get_deepseek_auto_sync_config(app: tauri::AppHandle) -> Result<AutoSyncConfig, String> {
    let path = config_path(&app)?;
    let cfg = load_config_from_path(&path);
    let enabled = cfg.auto_sync.unwrap_or(false);
    let interval_minutes = match cfg.sync_interval_minutes.unwrap_or(30) {
        60 => 60,
        _ => 30,
    };
    Ok(AutoSyncConfig {
        enabled,
        interval_minutes,
    })
}

#[tauri::command]
pub async fn set_deepseek_auto_sync_config(
    app: tauri::AppHandle,
    enabled: bool,
    interval_minutes: u32,
) -> Result<AutoSyncConfig, String> {
    let valid_interval = if interval_minutes >= 60 { 60 } else { 30 };
    let path = config_path(&app)?;
    let mut cfg = load_config_from_path(&path);
    cfg.auto_sync = Some(enabled);
    cfg.sync_interval_minutes = Some(valid_interval);
    save_config_to_path(&path, &cfg)?;

    start_auto_sync_worker(app.clone());

    if enabled {
        let app_clone = app.clone();
        tauri::async_runtime::spawn(async move {
            if let Ok(res) = fetch_deepseek_balance(app_clone.clone(), None).await {
                let _ = app_clone.emit("deepseek-balance-updated", &res);
            }
        });
    }

    Ok(AutoSyncConfig {
        enabled,
        interval_minutes: valid_interval,
    })
}

#[tauri::command]
pub async fn fetch_deepseek_balance(
    app: tauri::AppHandle,
    api_key: Option<String>,
) -> Result<DeepSeekBalanceResult, String> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);

    let key = match api_key {
        Some(k) if !k.trim().is_empty() => k.trim().to_string(),
        _ => {
            let path = config_path(&app)?;
            let cfg = load_config_from_path(&path);
            match cfg.api_key {
                Some(k) if !k.trim().is_empty() => k,
                _ => match detect_env_api_key() {
                    Some(k) => k,
                    None => {
                        return Ok(DeepSeekBalanceResult {
                            success: false,
                            is_available: false,
                            balance_infos: Vec::new(),
                            updated_at: now,
                            error_message: Some(
                                "未检测到 API Key，请在界面输入或在环境变量中配置 DEEPSEEK_API_KEY".to_string(),
                            ),
                        });
                    }
                },
            }
        }
    };

    let client = match reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
    {
        Ok(c) => c,
        Err(e) => {
            return Ok(DeepSeekBalanceResult {
                success: false,
                is_available: false,
                balance_infos: Vec::new(),
                updated_at: now,
                error_message: Some(format!("创建网络客户端失败：{e}")),
            });
        }
    };

    let response = match client
        .get(API_URL)
        .header("Authorization", format!("Bearer {key}"))
        .header("Accept", "application/json")
        .send()
        .await
    {
        Ok(r) => r,
        Err(e) => {
            return Ok(DeepSeekBalanceResult {
                success: false,
                is_available: false,
                balance_infos: Vec::new(),
                updated_at: now,
                error_message: Some(format!("请求 DeepSeek 接口网络异常：{e}")),
            });
        }
    };

    let status = response.status();
    if !status.is_success() {
        let err_text = response
            .text()
            .await
            .unwrap_or_else(|_| "无法读取错误响应体".to_string());
        let msg = if status.as_u16() == 401 {
            "API Key 无效或未授权，请检查后重试 (HTTP 401)".to_string()
        } else if status.as_u16() == 402 {
            "账户余额不足或已冻结 (HTTP 402)".to_string()
        } else {
            format!("请求失败 (HTTP {status})：{err_text}")
        };

        return Ok(DeepSeekBalanceResult {
            success: false,
            is_available: false,
            balance_infos: Vec::new(),
            updated_at: now,
            error_message: Some(msg),
        });
    }

    match response.json::<DeepSeekBalanceResponse>().await {
        Ok(data) => {
            if let Some(first) = data.balance_infos.first() {
                let _ = update_tray_balance(&app, Some(&first.total_balance), data.is_available);
            }
            Ok(DeepSeekBalanceResult {
                success: true,
                is_available: data.is_available,
                balance_infos: data.balance_infos,
                updated_at: now,
                error_message: None,
            })
        }
        Err(e) => Ok(DeepSeekBalanceResult {
            success: false,
            is_available: false,
            balance_infos: Vec::new(),
            updated_at: now,
            error_message: Some(format!("解析 DeepSeek 响应失败：{e}")),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env;

    #[test]
    fn test_parse_balance_json() {
        let raw = r#"{
            "is_available": true,
            "balance_infos": [
                {
                    "currency": "CNY",
                    "total_balance": "110.00",
                    "granted_balance": "10.00",
                    "topped_up_balance": "100.00"
                }
            ]
        }"#;

        let parsed: DeepSeekBalanceResponse = serde_json::from_str(raw).unwrap();
        assert!(parsed.is_available);
        assert_eq!(parsed.balance_infos.len(), 1);
        let info = &parsed.balance_infos[0];
        assert_eq!(info.currency, "CNY");
        assert_eq!(info.total_balance, "110.00");
        assert_eq!(info.granted_balance, "10.00");
        assert_eq!(info.topped_up_balance, "100.00");
    }

    #[test]
    fn test_config_save_load() {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let temp_dir = env::temp_dir().join(format!("deepseek-test-{unique}"));
        let config_file = temp_dir.join("config.json");

        let default_cfg = load_config_from_path(&config_file);
        assert_eq!(default_cfg.api_key, None);

        let new_cfg = DeepSeekConfig {
            api_key: Some("sk-test-123456".to_string()),
            show_tray_balance: Some(true),
            auto_sync: Some(true),
            sync_interval_minutes: Some(60),
        };
        save_config_to_path(&config_file, &new_cfg).unwrap();

        let loaded = load_config_from_path(&config_file);
        assert_eq!(loaded.api_key, Some("sk-test-123456".to_string()));
        let _ = fs::remove_dir_all(temp_dir);
    }


    #[test]
    fn test_config_clear() {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let temp_dir = env::temp_dir().join(format!("deepseek-test-clear-{unique}"));
        let config_file = temp_dir.join("config.json");

        let cfg = DeepSeekConfig {
            api_key: Some("sk-valid-key".to_string()),
            ..Default::default()
        };
        save_config_to_path(&config_file, &cfg).unwrap();
        assert!(load_config_from_path(&config_file).api_key.is_some());

        let empty_cfg = DeepSeekConfig::default();
        save_config_to_path(&config_file, &empty_cfg).unwrap();
        assert!(load_config_from_path(&config_file).api_key.is_none());

        let _ = fs::remove_dir_all(temp_dir);
    }

    #[test]
    fn test_tray_switch_save_load() {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let temp_dir = env::temp_dir().join(format!("deepseek-tray-test-{unique}"));
        let config_file = temp_dir.join("config.json");

        let cfg = DeepSeekConfig {
            show_tray_balance: Some(true),
            ..Default::default()
        };
        save_config_to_path(&config_file, &cfg).unwrap();
        let loaded = load_config_from_path(&config_file);
        assert_eq!(loaded.show_tray_balance, Some(true));
        let _ = fs::remove_dir_all(temp_dir);
    }

    #[test]
    fn test_auto_sync_save_load() {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let temp_dir = env::temp_dir().join(format!("deepseek-autosync-test-{unique}"));
        let config_file = temp_dir.join("config.json");

        let cfg = DeepSeekConfig {
            auto_sync: Some(true),
            sync_interval_minutes: Some(60),
            ..Default::default()
        };
        save_config_to_path(&config_file, &cfg).unwrap();
        let loaded = load_config_from_path(&config_file);
        assert_eq!(loaded.auto_sync, Some(true));
        assert_eq!(loaded.sync_interval_minutes, Some(60));
        let _ = fs::remove_dir_all(temp_dir);
    }

    #[test]
    fn test_parse_multi_currency() {
        let raw = r#"{
            "is_available": false,
            "balance_infos": [
                {
                    "currency": "CNY",
                    "total_balance": "0.00",
                    "granted_balance": "0.00",
                    "topped_up_balance": "0.00"
                },
                {
                    "currency": "USD",
                    "total_balance": "5.50",
                    "granted_balance": "0.50",
                    "topped_up_balance": "5.00"
                }
            ]
        }"#;
        let parsed: DeepSeekBalanceResponse = serde_json::from_str(raw).unwrap();
        assert!(!parsed.is_available);
        assert_eq!(parsed.balance_infos.len(), 2);
        assert_eq!(parsed.balance_infos[1].currency, "USD");
        assert_eq!(parsed.balance_infos[1].total_balance, "5.50");
    }

    #[test]
    fn test_live_invalid_key_returns_401() {
        tauri::async_runtime::block_on(async {
            let client = reqwest::Client::builder()
                .timeout(Duration::from_secs(5))
                .build()
                .unwrap();
            let res = client
                .get(API_URL)
                .header("Authorization", "Bearer sk-invalid-key-for-testing-only")
                .header("Accept", "application/json")
                .send()
                .await;
            if let Ok(response) = res {
                assert_eq!(response.status().as_u16(), 401);
            }
        });
    }

    #[test]
    fn test_detect_env_api_key() {
        let detected = detect_env_api_key();
        // If DEEPSEEK_API_KEY exists in local environment, it should be Some and non-empty
        if env::var("DEEPSEEK_API_KEY").is_ok() {
            assert!(detected.is_some());
            assert!(!detected.unwrap().is_empty());
        }
    }
}
