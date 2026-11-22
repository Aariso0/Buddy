use buddy_core::settings::*;
use buddy_core::support::SUPPORT_FLOORS;

#[test]
fn support_floors_match_spec() {
    assert_eq!(SUPPORT_FLOORS.windows_min, "10");
    assert_eq!(SUPPORT_FLOORS.macos_min, "14.2");
    assert!(SUPPORT_FLOORS.linux_requires_pipewire);
}

#[test]
fn missing_settings_file_returns_defaults() {
    let root = tempfile::tempdir().unwrap();
    let settings = load_settings(&root.path().join("settings.json")).unwrap();
    assert_eq!(settings.log_level, "info");
    assert_eq!(
        settings.data_dir,
        buddy_core::settings::Settings::default().data_dir
    );
}

#[test]
fn settings_round_trip() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("settings.json");
    let settings = Settings {
        data_dir: root.path().join("data"),
        log_level: "debug".into(),
    };
    save_settings(&path, &settings).unwrap();
    assert_eq!(load_settings(&path).unwrap(), settings);
}

#[test]
fn save_settings_uses_a_unique_staging_file() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("settings.json");
    // The old fixed staging name would make `File::create` fail here.
    std::fs::create_dir(root.path().join("settings.json.tmp")).unwrap();
    let settings = Settings {
        data_dir: root.path().join("data"),
        log_level: "trace".into(),
    };
    save_settings(&path, &settings).unwrap();
    assert_eq!(load_settings(&path).unwrap(), settings);
}

#[test]
fn failed_save_settings_removes_staging_file() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("settings.json");
    // Rename cannot replace a directory with a file, so this save must fail.
    std::fs::create_dir(&path).unwrap();
    let settings = Settings {
        data_dir: root.path().join("data"),
        log_level: "debug".into(),
    };
    assert!(save_settings(&path, &settings).is_err());
    let leftovers: Vec<String> = std::fs::read_dir(root.path())
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|name| name.starts_with("settings.json.") && name.ends_with(".tmp"))
        .collect();
    assert!(leftovers.is_empty(), "stale staging files: {leftovers:?}");
}

#[test]
fn corrupt_settings_file_is_typed_error() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("settings.json");
    std::fs::write(&path, b"{not json").unwrap();
    assert!(matches!(
        load_settings(&path),
        Err(SettingsError::InvalidSettings(_))
    ));
}
