use buddy_core::support::{SupportFloors, SUPPORT_FLOORS};

#[tauri::command]
fn support_matrix() -> SupportFloors {
    SUPPORT_FLOORS.clone()
}

pub fn run() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![support_matrix])
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

    #[test]
    fn configured_main_window_has_expected_label_and_title() {
        let context: tauri::Context<tauri::Wry> = tauri::generate_context!();
        let window = context
            .config()
            .app
            .windows
            .iter()
            .find(|window| window.label == "main")
            .expect("a window labeled main must be configured");
        assert_eq!(window.title, "Buddy");
    }
}
