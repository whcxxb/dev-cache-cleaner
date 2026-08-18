mod cleaner;

use cleaner::{clean_cache_target, clear_cleanup_history, get_cleanup_history, scan_cache_targets};

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .invoke_handler(tauri::generate_handler![
            scan_cache_targets,
            clean_cache_target,
            get_cleanup_history,
            clear_cleanup_history
        ])
        .run(tauri::generate_context!())
        .expect("failed to run developer cache cleaner");
}
