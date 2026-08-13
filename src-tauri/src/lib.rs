mod cleaner;

use cleaner::{clean_cache_target, scan_cache_targets};

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .invoke_handler(tauri::generate_handler![
            scan_cache_targets,
            clean_cache_target
        ])
        .run(tauri::generate_context!())
        .expect("failed to run developer cache cleaner");
}
