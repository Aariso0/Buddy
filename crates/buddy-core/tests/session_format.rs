// Imports below are kept verbatim from the task brief; `Path` and `Uuid` are
// vestigial there, so silence the otherwise warning-generating unused imports.
#![allow(unused_imports)]

use buddy_core::session::*;
use chrono::{TimeZone, Utc};
use std::path::Path;
use uuid::Uuid;

fn id() -> &'static str {
    "11111111-1111-4111-8111-111111111111"
}
fn now() -> chrono::DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 11, 22, 10, 0, 0).unwrap()
}

#[test]
fn create_read_recover_round_trip() {
    let root = tempfile::tempdir().unwrap();
    let manifest = create_session(root.path(), id(), now()).unwrap();
    assert_eq!(manifest.format_version, 1);
    assert_eq!(manifest.id, id());
    assert!(matches!(manifest.state, SessionState::Recording));
    assert_eq!(read_session(root.path(), id()).unwrap(), manifest);
    let outcome = recover_session(root.path(), id()).unwrap();
    assert!(outcome.needs_repair);
    assert_eq!(outcome.manifest.state, SessionState::Recording);
}

#[test]
fn create_session_rejects_existing_id_without_changing_it() {
    let root = tempfile::tempdir().unwrap();
    let manifest = create_session(root.path(), id(), now()).unwrap();
    assert!(matches!(
        create_session(root.path(), id(), now()),
        Err(SessionError::AlreadyExists(_))
    ));
    assert_eq!(read_session(root.path(), id()).unwrap(), manifest);
}

#[test]
fn manifest_id_must_match_directory_id() {
    let root = tempfile::tempdir().unwrap();
    let other_id = "22222222-2222-4222-8222-222222222222";
    create_session(root.path(), other_id, now()).unwrap();
    let dir = root.path().join(id());
    std::fs::create_dir(&dir).unwrap();
    std::fs::copy(
        root.path().join(other_id).join("session.json"),
        dir.join("session.json"),
    )
    .unwrap();
    assert!(matches!(
        read_session(root.path(), id()),
        Err(SessionError::IdMismatch { .. })
    ));
    assert!(matches!(
        recover_session(root.path(), id()),
        Err(SessionError::IdMismatch { .. })
    ));
}

#[test]
fn corrupt_manifest_is_typed_error() {
    let root = tempfile::tempdir().unwrap();
    let dir = root.path().join(id());
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("session.json"),
        b"{\"format_version\":1,\"state\":",
    )
    .unwrap();
    assert!(matches!(
        read_session(root.path(), id()),
        Err(SessionError::InvalidManifest(_))
    ));
}

#[test]
fn traversal_session_id_is_rejected() {
    let root = tempfile::tempdir().unwrap();
    assert!(matches!(
        read_session(root.path(), "../etc/passwd"),
        Err(SessionError::InvalidSessionId(_))
    ));
    assert!(matches!(
        read_session(root.path(), "/etc/passwd"),
        Err(SessionError::InvalidSessionId(_))
    ));
}

#[test]
fn unknown_version_is_typed_error() {
    let root = tempfile::tempdir().unwrap();
    let dir = root.path().join(id());
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("session.json"), b"{\"id\":\"11111111-1111-4111-8111-111111111111\",\"format_version\":2,\"state\":\"recording\",\"started_at\":\"2026-11-22T10:00:00Z\",\"ended_at\":null,\"preset\":\"lecture\"}").unwrap();
    assert!(matches!(
        read_session(root.path(), id()),
        Err(SessionError::UnsupportedVersion(2))
    ));
}

#[test]
fn stale_tmp_file_does_not_affect_read() {
    let root = tempfile::tempdir().unwrap();
    create_session(root.path(), id(), now()).unwrap();
    std::fs::write(root.path().join(id()).join("session.json.tmp"), b"garbage").unwrap();
    assert_eq!(
        read_session(root.path(), id()).unwrap().state,
        SessionState::Recording
    );
}

#[test]
fn manifest_matches_shared_schema_fixture() {
    let root = tempfile::tempdir().unwrap();
    let manifest = create_session(root.path(), id(), now()).unwrap();
    let fixture: serde_json::Value = serde_json::from_str(include_str!(
        "../../../packages/schemas/fixtures/session-v1.json"
    ))
    .unwrap();
    assert_eq!(serde_json::to_value(&manifest).unwrap(), fixture);
}
