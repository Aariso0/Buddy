mod common;

use buddy_capture::chunks::{read_chunk_index, read_gaps, GapReason};
use buddy_capture::engine::{CaptureConfig, CaptureEngine};
use buddy_capture::fake::FakeAudioSource;
use buddy_capture::{CaptureError, TrackKind};
use common::*;

#[test]
fn pause_resume_keeps_monotonic_timeline() {
    let dir = tempfile::tempdir().unwrap();
    let session_dir = session_dir_under(dir.path());
    let (mic, handle) = ControlledSource::new(TrackKind::Mic, usize::MAX, SOURCE_PACE);
    let mut engine = CaptureEngine::new(
        SESSION_ID,
        &session_dir,
        buddy_core::clock::SessionClock::new(),
        Box::new(FakeAudioSource::from_samples(
            TrackKind::System,
            48_000,
            vec![0.5; 144_000],
        )),
        Box::new(mic),
        CaptureConfig::default(),
    )
    .unwrap();
    engine.start().unwrap();
    engine.pause().unwrap();
    // Wait until the reader has actually applied pause, then capture the
    // boundary on the same clock that stamps gap markers.
    wait_for(|| handle.pauses() >= 1);
    let pause_boundary_ms = engine.elapsed_ms();
    handle.inject(CaptureError::DeviceLost("mic-after-resume".into()));
    engine.resume().unwrap();
    // Only assert the boundary once the reader has really re-armed after the
    // injected loss; stopping earlier can race with the gap being stamped.
    wait_for(|| handle.reconnects() >= 1);
    engine.stop().unwrap();

    let gaps = read_gaps(&session_dir).unwrap();
    let loss_starts: Vec<u64> = gaps
        .iter()
        .filter(|gap| matches!(gap.reason, GapReason::DeviceLoss))
        .map(|gap| gap.start_ms)
        .collect();
    assert!(
        !loss_starts.is_empty(),
        "the injected device-loss gap must actually be recorded: {gaps:?}"
    );
    assert!(
        loss_starts
            .iter()
            .any(|&start| start >= pause_boundary_ms),
        "a DeviceLoss gap injected after resume must not be stamped before the pause boundary ({pause_boundary_ms}): {loss_starts:?}"
    );
    let all_starts: Vec<u64> = gaps.iter().map(|gap| gap.start_ms).collect();
    assert!(all_starts.windows(2).all(|w| w[0] <= w[1]));
    assert!(read_chunk_index(&session_dir)
        .unwrap()
        .iter()
        .all(|c| c.frames > 0));
}
