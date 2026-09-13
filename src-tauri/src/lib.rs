mod cleaner;
mod prompt_manager;
mod updater;
mod deepseek;

use cleaner::{clean_cache_target, clear_cleanup_history, get_cleanup_history, scan_cache_targets};
use prompt_manager::{
    get_prompt_manager_state, read_tool_prompt, save_global_prompt, save_tool_prompt,
    set_global_prompt_enabled, set_tool_global_prompt_enabled,
};
use updater::{scan_tool_updates, upgrade_tool, upgrade_tools};
use deepseek::{
    clear_deepseek_api_key, fetch_deepseek_balance, get_deepseek_api_key,
    get_deepseek_auto_sync_config, get_deepseek_tray_switch, save_deepseek_api_key,
    set_deepseek_auto_sync_config, set_deepseek_tray_switch,
};

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            let handle = app.handle();
            let _ = deepseek::update_tray_balance(handle, None, false);
            deepseek::start_auto_sync_worker(handle.clone());
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            scan_cache_targets,
            clean_cache_target,
            get_cleanup_history,
            clear_cleanup_history,
            get_prompt_manager_state,
            read_tool_prompt,
            save_global_prompt,
            save_tool_prompt,
            set_global_prompt_enabled,
            set_tool_global_prompt_enabled,
            scan_tool_updates,
            upgrade_tool,
            upgrade_tools,
            get_deepseek_api_key,
            save_deepseek_api_key,
            clear_deepseek_api_key,
            fetch_deepseek_balance,
            get_deepseek_tray_switch,
            set_deepseek_tray_switch,
            get_deepseek_auto_sync_config,
            set_deepseek_auto_sync_config,
        ])
        .run(tauri::generate_context!())
        .expect("failed to run developer cache cleaner");
}
