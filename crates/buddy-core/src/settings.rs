use serde::{Deserialize, Serialize};
use std::ffi::OsString;
use std::fs;
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};
use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Settings {
    pub data_dir: PathBuf,
    pub log_level: String,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            // `data_local_dir` is absent only on platforms without a defined
            // local-data location; fall back to a relative path rather than
            // panicking, since `Default` cannot return an error.
            data_dir: dirs::data_local_dir()
                .map(|dir| dir.join("Buddy"))
                .unwrap_or_else(|| PathBuf::from("Buddy")),
            log_level: "info".to_owned(),
        }
    }
}

#[derive(Debug, Error)]
pub enum SettingsError {
    #[error("invalid settings: {0}")]
    InvalidSettings(String),
    #[error("settings io error: {0}")]
    Io(#[from] std::io::Error),
}

/// Load settings from `path`. A missing file means "first run" and yields the
/// defaults; a file that exists but cannot be parsed is a typed error.
pub fn load_settings(path: &Path) -> Result<Settings, SettingsError> {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(err) if err.kind() == ErrorKind::NotFound => return Ok(Settings::default()),
        Err(err) => return Err(SettingsError::Io(err)),
    };
    serde_json::from_slice(&bytes).map_err(|err| SettingsError::InvalidSettings(err.to_string()))
}

/// Write settings to `path` atomically using the session module's
/// temp-file → fsync → rename pattern.
pub fn save_settings(path: &Path, settings: &Settings) -> Result<(), SettingsError> {
    let bytes = serde_json::to_vec_pretty(settings)
        .map_err(|err| SettingsError::InvalidSettings(err.to_string()))?;

    let mut tmp_name = OsString::from(path.as_os_str());
    tmp_name.push(".tmp");
    let tmp_path = PathBuf::from(tmp_name);

    let mut tmp = fs::File::create(&tmp_path)?;
    tmp.write_all(&bytes)?;
    tmp.sync_all()?;
    drop(tmp);

    fs::rename(&tmp_path, path)?;

    // The rename has already committed the file; directory fsync is only a
    // durability hint. A plain File::open on a directory fails on Windows, so
    // keep this Unix-only and best-effort (same ruling as Task 2).
    #[cfg(unix)]
    {
        if let Some(parent) = path.parent() {
            if let Ok(dir) = fs::File::open(parent) {
                let _ = dir.sync_all();
            }
        }
    }

    Ok(())
}
