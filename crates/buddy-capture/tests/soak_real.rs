//! Real-device 60-minute capture soak.
//!
//! Ignored by the normal test suite because it needs live audio endpoints and
//! an hour of wall time. The Task 8 CI soak jobs opt in with
//! `BUDDY_AUDIO_SOAK=1` and `--ignored`; a device-less runner fails instead of
//! skipping. The CI job plays the generated signal into the default sink
//! (PipeWire) or default render endpoint (WASAPI loopback); this test drives
//! the real backend and validates the published WAV sequences and gap markers
//! through the public API.

#![cfg(any(target_os = "linux", target_os = "windows"))]

use buddy_capture::chunks::{read_chunk_index, read_gaps};
use buddy_capture::engine::{CaptureConfig, CaptureEngine};
use buddy_capture::{AudioSource, TrackKind};
use buddy_core::clock::SessionClock;
use buddy_core::session::{create_session, Preset};
use chrono::Utc;
use std::time::Duration;

#[cfg(target_os = "linux")]
use buddy_capture::linux::PipeWireSource as RealSource;
#[cfg(target_os = "windows")]
use buddy_capture::windows::WasapiSource as RealSource;

const SESSION_ID: &str = "11111111-1111-4111-8111-111111111111";
const SOAK_SECONDS: u64 = 60 * 60;
const SAMPLE_RATE: u32 = 48_000;
const CHANNELS: u16 = 2;
// `ChunkInfo::frames` counts interleaved f32 values, so a full one-second
// stereo chunk at 48 kHz is 48_000 * 2 = 96_000 values.
const FULL_CHUNK_VALUES: u32 = SAMPLE_RATE * CHANNELS as u32;
// 95% of the 3600 s target. Tolerates normal start/stop drift, but an early
// stop or systematically short chunks cannot pass as a full-hour soak.
const MIN_COVERAGE_SECONDS: u64 = (SOAK_SECONDS * 95) / 100;

// Bare ids select the System track on both backends; Mic ids are prefixed.
const SYSTEM_ID: &str = "default";
const MIC_ID: &str = "mic:default";

fn default_id(kind: &TrackKind) -> &'static str {
    match kind {
        TrackKind::System => SYSTEM_ID,
        TrackKind::Mic => MIC_ID,
    }
}

/// The System track must actually contain the signal the CI job played, not
/// just silence. Read the 44-byte WAV header plus f32 data of one chunk.
fn assert_captured_signal(path: &std::path::Path, track: &TrackKind) {
    let bytes =
        std::fs::read(path).unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
    assert!(
        bytes.len() > 44,
        "{track:?} published an empty chunk at {}",
        path.display()
    );
    let peak = bytes[44..]
        .chunks_exact(4)
        .map(|sample| f32::from_le_bytes([sample[0], sample[1], sample[2], sample[3]]).abs())
        .fold(0.0f32, f32::max);
    assert!(
        peak > 0.01,
        "{track:?} captured only silence (peak {peak}); the generated signal did not reach the default endpoint"
    );
    println!("{track:?} signal peak in first chunk: {peak:.3}");
}

#[test]
#[ignore = "real-device soak: requires BUDDY_AUDIO_SOAK=1 and live endpoints"]
fn sixty_minute_real_capture_has_two_gap_free_chunk_sequences() {
    assert_eq!(
        std::env::var("BUDDY_AUDIO_SOAK").as_deref(),
        Ok("1"),
        "BUDDY_AUDIO_SOAK=1 is required; no device-less runner may pass"
    );

    for kind in [TrackKind::System, TrackKind::Mic] {
        let devices = RealSource::list_devices(kind.clone())
            .unwrap_or_else(|error| panic!("{kind:?} device enumeration failed: {error}"));
        let default = default_id(&kind);
        assert!(
            devices.iter().any(|device| device.id == default),
            "{kind:?} has no default endpoint: {devices:?}"
        );
        assert!(
            devices.iter().any(|device| device.id != default),
            "{kind:?} has no real endpoint on this runner: {devices:?}"
        );
    }

    let root = tempfile::tempdir().expect("soak session temp dir");
    create_session(root.path(), SESSION_ID, Utc::now(), Preset::Lecture)
        .expect("reserve soak session");
    let session_dir = root.path().join(SESSION_ID);

    let mut engine = CaptureEngine::new(
        SESSION_ID,
        &session_dir,
        SessionClock::new(),
        Box::new(RealSource::open(SYSTEM_ID).expect("open real System endpoint")),
        Box::new(RealSource::open(MIC_ID).expect("open real Mic endpoint")),
        CaptureConfig::default(),
    )
    .expect("construct real capture engine");
    engine.start().expect("start real capture");
    std::thread::sleep(Duration::from_secs(SOAK_SECONDS));
    engine.stop().expect("stop real capture cleanly");

    let gaps = read_gaps(&session_dir).expect("read gap markers");
    let chunks = read_chunk_index(&session_dir).expect("validate published chunks");
    for kind in [TrackKind::System, TrackKind::Mic] {
        let sequence: Vec<_> = chunks.iter().filter(|chunk| chunk.track == kind).collect();
        assert!(
            !sequence.is_empty(),
            "{kind:?} published no chunks; chunk index: {chunks:?}"
        );
        for (expected_index, chunk) in (0u32..).zip(sequence.iter()) {
            assert_eq!(
                chunk.index, expected_index,
                "{kind:?} chunk sequence is not gap-free: {sequence:?}"
            );
            assert!(
                chunk.frames > 0 && chunk.sample_rate == SAMPLE_RATE && chunk.channels == CHANNELS,
                "{kind:?} published an invalid chunk: {chunk:?}"
            );
            // The writer publishes only full chunks while capturing and
            // flushes at most one short chunk on stop. A short chunk before
            // the last one is lost audio, even with no gap marker.
            assert!(
                chunk.frames >= FULL_CHUNK_VALUES || expected_index + 1 == sequence.len() as u32,
                "{kind:?} chunk {expected_index} is short: {} values, expected >= {FULL_CHUNK_VALUES}; only the final chunk may be partial: {chunk:?}",
                chunk.frames
            );
        }
        let total_values: u64 = sequence.iter().map(|chunk| u64::from(chunk.frames)).sum();
        let values_per_second = u64::from(SAMPLE_RATE) * u64::from(CHANNELS);
        let covered_seconds = total_values / values_per_second;
        assert!(
            covered_seconds >= MIN_COVERAGE_SECONDS,
            "{kind:?} covered only {covered_seconds}s of the {SOAK_SECONDS}s target (minimum {MIN_COVERAGE_SECONDS}s): {total_values} values in {} chunks",
            sequence.len()
        );
        println!(
            "{kind:?}: {} chunks, {covered_seconds}s, indices 0..{}",
            sequence.len(),
            sequence.len() - 1
        );
        if kind == TrackKind::System {
            assert_captured_signal(&sequence[0].path, &kind);
        }
    }
    println!(
        "gaps: {} ({:?})",
        gaps.len(),
        gaps.iter().map(|gap| gap.reason).collect::<Vec<_>>()
    );
    assert!(
        gaps.is_empty(),
        "soak must have zero lost chunks; gap markers: {gaps:?}"
    );
}
