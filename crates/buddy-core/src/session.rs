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
    #[error("session already exists: {0}")]
    AlreadyExists(String),
    #[error("session id mismatch: manifest declares {manifest} but directory is {directory}")]
    IdMismatch { manifest: String, directory: String },
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

    // Uuid parses uppercase and hyphenless spellings, but the on-disk id must
    // be exactly the canonical lowercase hyphenated form. Rejecting rather than
    // normalizing keeps one directory per session with no hidden aliases.
    let noncanonical = Uuid::parse_str(id)
        .map(|parsed| parsed.to_string() != id)
        .unwrap_or(true);
    if unsafe_id || noncanonical {
        return Err(SessionError::InvalidSessionId(id.to_owned()));
    }

    Ok(root.join(id_path))
}

// Directory fsync is durability hardening only: it must never turn a successful
// write into an error. Plain File::open on a directory fails on Windows (it
// needs FILE_FLAG_BACKUP_SEMANTICS), so keep this Unix-only and best-effort.
#[cfg(unix)]
fn sync_dir(path: &Path) {
    if let Ok(dir) = fs::File::open(path) {
        let _ = dir.sync_all();
    }
}

#[cfg(unix)]
fn sync_dir_chain(path: &Path) {
    let mut current = Some(path);
    while let Some(dir) = current {
        // A one-component relative path has an empty parent, which means `.`.
        let target = if dir.as_os_str().is_empty() {
            Path::new(".")
        } else {
            dir
        };
        sync_dir(target);
        current = dir.parent();
    }
}

/// Create a session in `recording` state, writing `session.json` atomically.
/// The session directory is reserved exclusively, so an existing id is an
/// error rather than an overwrite of its manifest.
pub fn create_session(
    root: &Path,
    id: &str,
    now: DateTime<Utc>,
    preset: Preset,
) -> Result<SessionManifest, SessionError> {
    let dir = session_dir(root, id)?;
    let parent = dir.parent().unwrap_or_else(|| Path::new("."));
    if !parent.as_os_str().is_empty() {
        fs::create_dir_all(parent)?;
    }
    match fs::create_dir(&dir) {
        Ok(()) => {}
        Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => {
            return Err(SessionError::AlreadyExists(id.to_owned()));
        }
        Err(err) => return Err(SessionError::Io(err)),
    }

    // Persist the reserved entry in its parent, plus any ancestors created
    // above, before write_manifest publishes the manifest inside it.
    #[cfg(unix)]
    sync_dir_chain(&parent);

    let manifest = SessionManifest {
        id: id.to_owned(),
        format_version: FORMAT_VERSION,
        state: SessionState::Recording,
        started_at: now,
        ended_at: None,
        preset,
    };

    // Cleanup only ever touches the directory reserved by this attempt. An
    // existing session was rejected above and cannot reach this point.
    if let Err(err) = write_manifest(&dir, &manifest) {
        let _ = fs::remove_dir_all(&dir);
        return Err(err);
    }
    Ok(manifest)
}

fn write_manifest(dir: &Path, manifest: &SessionManifest) -> Result<(), SessionError> {
    let bytes = serde_json::to_vec_pretty(manifest)
        .map_err(|e| SessionError::InvalidManifest(e.to_string()))?;
    // A unique, exclusively created staging name keeps overlapping writes from
    // truncating or renaming each other's in-progress manifest.
    let tmp_path = dir.join(format!("session.json.{}.tmp", Uuid::new_v4()));

    let mut tmp = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&tmp_path)?;
    tmp.write_all(&bytes)?;
    tmp.sync_all()?;
    drop(tmp);

    fs::rename(&tmp_path, dir.join("session.json"))?;

    // The rename has already committed `session.json`; fsync only hardens
    // durability and must not turn a successful write into an error.
    #[cfg(unix)]
    sync_dir(dir);

    Ok(())
}

/// Mark an existing session complete through the same atomic manifest write
/// used at creation. Everything except `state` and `ended_at` is left intact.
pub fn finalize_session(
    root: &Path,
    id: &str,
    now: DateTime<Utc>,
) -> Result<SessionManifest, SessionError> {
    let dir = session_dir(root, id)?;
    let mut manifest = read_session(root, id)?;
    manifest.state = SessionState::Complete;
    manifest.ended_at = Some(now);
    write_manifest(&dir, &manifest)?;
    Ok(manifest)
}

/// Read a session manifest. Malformed JSON and schema errors are typed errors,
/// and an unsupported `format_version` is reported before schema validation.
pub fn read_session(root: &Path, id: &str) -> Result<SessionManifest, SessionError> {
    let dir = session_dir(root, id)?;
    let bytes = fs::read(dir.join("session.json"))?;
    let manifest = parse_manifest(&bytes)?;
    if manifest.id != id {
        return Err(SessionError::IdMismatch {
            manifest: manifest.id,
            directory: id.to_owned(),
        });
    }
    Ok(manifest)
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
