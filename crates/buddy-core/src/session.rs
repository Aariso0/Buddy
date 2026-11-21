use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::Write;
use std::path::{Component, Path, PathBuf};
use thiserror::Error;
use uuid::Uuid;

/// Current on-disk `session.json` format version.
pub const FORMAT_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SessionState {
    Recording,
    Finalizing,
    Complete,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Preset {
    Lecture,
    Meeting,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionManifest {
    pub id: String,
    pub format_version: u32,
    pub state: SessionState,
    pub started_at: DateTime<Utc>,
    pub ended_at: Option<DateTime<Utc>>,
    pub preset: Preset,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RecoveryOutcome {
    pub manifest: SessionManifest,
    pub needs_repair: bool,
}

#[derive(Debug, Error)]
pub enum SessionError {
    #[error("invalid session id: {0}")]
    InvalidSessionId(String),
    #[error("unsupported session format version: {0}")]
    UnsupportedVersion(u32),
    #[error("invalid session manifest: {0}")]
    InvalidManifest(String),
    #[error("session io error: {0}")]
    Io(#[from] std::io::Error),
}

/// Resolve a session id to a directory directly beneath `root`, rejecting
/// anything that could escape `root` before any filesystem access occurs.
fn session_dir(root: &Path, id: &str) -> Result<PathBuf, SessionError> {
    let id_path = Path::new(id);
    let unsafe_id = id.is_empty()
        || id.contains('/')
        || id.contains('\\')
        || id_path.is_absolute()
        || id_path.components().any(|c| {
            matches!(
                c,
                Component::CurDir
                    | Component::ParentDir
                    | Component::RootDir
                    | Component::Prefix(_)
            )
        });

    if unsafe_id || Uuid::parse_str(id).is_err() {
        return Err(SessionError::InvalidSessionId(id.to_owned()));
    }

    Ok(root.join(id_path))
}

/// Create a session in `recording` state, writing `session.json` atomically.
pub fn create_session(
    root: &Path,
    id: &str,
    now: DateTime<Utc>,
) -> Result<SessionManifest, SessionError> {
    let dir = session_dir(root, id)?;
    fs::create_dir_all(&dir)?;

    let manifest = SessionManifest {
        id: id.to_owned(),
        format_version: FORMAT_VERSION,
        state: SessionState::Recording,
        started_at: now,
        ended_at: None,
        preset: Preset::Lecture,
    };

    write_manifest(&dir, &manifest)?;
    Ok(manifest)
}

fn write_manifest(dir: &Path, manifest: &SessionManifest) -> Result<(), SessionError> {
    let bytes = serde_json::to_vec_pretty(manifest)
        .map_err(|e| SessionError::InvalidManifest(e.to_string()))?;
    let tmp_path = dir.join("session.json.tmp");

    let mut tmp = fs::File::create(&tmp_path)?;
    tmp.write_all(&bytes)?;
    tmp.sync_all()?;
    drop(tmp);

    fs::rename(&tmp_path, dir.join("session.json"))?;

    // The rename has already committed `session.json`. Directory fsync is
    // durability hardening only, so it must never turn a successful write into
    // a reported failure. A plain File::open on a directory fails on Windows
    // (it needs FILE_FLAG_BACKUP_SEMANTICS), so keep this step Unix-only and
    // best-effort; Windows CI still exercises create_session via the existing
    // round-trip test.
    #[cfg(unix)]
    {
        if let Ok(parent) = fs::File::open(dir) {
            let _ = parent.sync_all();
        }
    }

    Ok(())
}

/// Read a session manifest. Malformed JSON and schema errors are typed errors,
/// and an unsupported `format_version` is reported before schema validation.
pub fn read_session(root: &Path, id: &str) -> Result<SessionManifest, SessionError> {
    let dir = session_dir(root, id)?;
    let bytes = fs::read(dir.join("session.json"))?;
    parse_manifest(&bytes)
}

fn parse_manifest(bytes: &[u8]) -> Result<SessionManifest, SessionError> {
    let value: serde_json::Value =
        serde_json::from_slice(bytes).map_err(|e| SessionError::InvalidManifest(e.to_string()))?;

    let raw_version = value
        .get("format_version")
        .and_then(serde_json::Value::as_u64)
        .ok_or_else(|| {
            SessionError::InvalidManifest("missing or non-numeric format_version".to_owned())
        })?;
    let version = u32::try_from(raw_version).map_err(|_| {
        SessionError::InvalidManifest(format!("format_version out of range: {raw_version}"))
    })?;

    if version != FORMAT_VERSION {
        return Err(SessionError::UnsupportedVersion(version));
    }

    serde_json::from_value(value).map_err(|e| SessionError::InvalidManifest(e.to_string()))
}

/// Read a manifest and report whether it describes an interrupted session.
pub fn recover_session(root: &Path, id: &str) -> Result<RecoveryOutcome, SessionError> {
    let manifest = read_session(root, id)?;
    let needs_repair = !matches!(manifest.state, SessionState::Complete);
    Ok(RecoveryOutcome {
        manifest,
        needs_repair,
    })
}
