//! Regression tests for PR review findings: invalid capture configs and
//! restarting a session that already holds published capture output.

use buddy_capture::engine::*;
use buddy_capture::fake::FakeAudioSource;
use buddy_capture::{CaptureError, TrackKind};
use std::path::{Path, PathBuf};

const SESSION_ID: &str = "22222222-2222-4222-8222-222222222222";

fn session_dir_under(root: &Path) -> PathBuf {
    let sessions_root = root.join("sessions");
    buddy_core::session::create_session(
        &sessions_root,
        SESSION_ID,
        chrono::Utc::now(),
        buddy_core::session::Preset::Lecture,
    )
    .unwrap();
    sessions_root.join(SESSION_ID)
}

fn engine(session_dir: &Path, config: CaptureConfig) -> Result<CaptureEngine, CaptureError> {
    CaptureEngine::new(
        SESSION_ID,
        session_dir,
        buddy_core::clock::SessionClock::new(),
        Box::new(FakeAudioSource::from_samples(
            TrackKind::System,
            48_000,
            Vec::new(),
        )),
        Box::new(FakeAudioSource::from_samples(
            TrackKind::Mic,
            48_000,
            Vec::new(),
        )),
        config,
    )
}

#[test]
fn zero_sized_capture_config_is_rejected_before_allocating_rings() {
    let dir = tempfile::tempdir().unwrap();
    let session_dir = session_dir_under(dir.path());
    for (config, field) in [
        (
            CaptureConfig {
                ring_frames: 0,
                ..CaptureConfig::default()
            },
            "ring_frames",
        ),
        (
            CaptureConfig {
                chunk_frames: 0,
                ..CaptureConfig::default()
            },
            "chunk_frames",
        ),
        (
            CaptureConfig {
                channels: 0,
                ..CaptureConfig::default()
            },
            "channels",
        ),
    ] {
        let error = engine(&session_dir, config)
            .err()
            .expect("a zero-sized config must be rejected");
        assert!(
            matches!(&error, CaptureError::Backend(message) if message.contains(field)),
            "unexpected error: {error}"
        );
    }
    assert!(
        !session_dir.join("audio").exists(),
        "a rejected config must not touch the session"
    );
}

#[test]
fn a_fresh_session_with_default_config_still_starts_and_stops() {
    let dir = tempfile::tempdir().unwrap();
    let session_dir = session_dir_under(dir.path());
    let mut engine = engine(&session_dir, CaptureConfig::default()).unwrap();
    engine.start().unwrap();
    engine.stop().unwrap();
    assert!(session_dir.join("audio").is_dir());
}

#[test]
fn a_session_with_a_published_chunk_cannot_be_restarted() {
    let dir = tempfile::tempdir().unwrap();
    let session_dir = session_dir_under(dir.path());
    let audio_dir = session_dir.join("audio");
    std::fs::create_dir_all(&audio_dir).unwrap();
    let chunk = audio_dir.join("system-0000.wav");
    std::fs::write(&chunk, b"earlier capture").unwrap();

    let mut engine = engine(&session_dir, CaptureConfig::default()).unwrap();
    let error = engine
        .start()
        .expect_err("a published chunk must block a fresh chunk index");
    assert!(
        matches!(&error, CaptureError::Backend(message) if message.contains(SESSION_ID) && message.contains("system-0000.wav")),
        "unexpected error: {error}"
    );
    assert_eq!(
        std::fs::read(&chunk).unwrap(),
        b"earlier capture".to_vec(),
        "a rejected start must not overwrite existing output"
    );
    assert!(
        matches!(engine.stop(), Err(CaptureError::Backend(message)) if message.contains("before start")),
        "a rejected start must not spawn workers or change engine state"
    );
}

#[test]
fn a_session_with_a_gap_log_cannot_be_restarted() {
    let dir = tempfile::tempdir().unwrap();
    let session_dir = session_dir_under(dir.path());
    let audio_dir = session_dir.join("audio");
    std::fs::create_dir_all(&audio_dir).unwrap();
    std::fs::write(audio_dir.join("gaps.jsonl"), b"earlier gap\n").unwrap();

    let mut engine = engine(&session_dir, CaptureConfig::default()).unwrap();
    let error = engine
        .start()
        .expect_err("an existing gap log must block a fresh writer");
    assert!(
        matches!(&error, CaptureError::Backend(message) if message.contains("gaps.jsonl")),
        "unexpected error: {error}"
    );
    assert!(
        matches!(engine.stop(), Err(CaptureError::Backend(message)) if message.contains("before start")),
        "a rejected start must not spawn workers or change engine state"
    );
}

#[test]
fn an_unpublished_staging_file_does_not_block_a_fresh_start() {
    let dir = tempfile::tempdir().unwrap();
    let session_dir = session_dir_under(dir.path());
    let audio_dir = session_dir.join("audio");
    std::fs::create_dir_all(&audio_dir).unwrap();
    let staging = audio_dir.join(".system-0000.wav.01234567-89ab-4cde-8fab-0123456789ab.tmp");
    std::fs::write(&staging, b"partial").unwrap();

    let mut engine = engine(&session_dir, CaptureConfig::default()).unwrap();
    engine.start().unwrap();
    engine.stop().unwrap();
    assert!(staging.exists(), "a staging file is not published output");
}
