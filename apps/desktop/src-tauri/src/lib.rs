use buddy_core::support::{SupportFloors, SUPPORT_FLOORS};
use tauri::Manager;

#[tauri::command]
fn support_matrix() -> SupportFloors {
    SUPPORT_FLOORS.clone()
}

pub fn run() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![support_matrix])
        // `app` is empty in tauri.conf.json, so set the required window title here.
        .setup(|app| {
            if let Some(window) = app.get_webview_window("main") {
                window.set_title("Buddy")?;
            }
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running Buddy");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn support_matrix_command_matches_spec() {
        let m = support_matrix();
        assert_eq!(m.windows_min, "10");
        assert_eq!(m.macos_min, "14.2");
        assert!(m.linux_requires_pipewire);
    }
}
