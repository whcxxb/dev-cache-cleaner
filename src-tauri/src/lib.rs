mod cleaner;
mod prompt_manager;
mod updater;

use cleaner::{clean_cache_target, clear_cleanup_history, get_cleanup_history, scan_cache_targets};
use prompt_manager::{
    get_prompt_manager_state, read_tool_prompt, save_global_prompt, save_tool_prompt,
    set_global_prompt_enabled, set_tool_global_prompt_enabled,
};
use updater::{scan_tool_updates, upgrade_tool, upgrade_tools};

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
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
            upgrade_tools
        ])
        .run(tauri::generate_context!())
        .expect("failed to run developer cache cleaner");
}
