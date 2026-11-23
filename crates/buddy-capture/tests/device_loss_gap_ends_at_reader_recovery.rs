mod common;

use buddy_capture::chunks::{read_gaps, GapReason};
use buddy_capture::engine::{CaptureConfig, CaptureEngine};
use buddy_capture::fake::FakeAudioSource;
use buddy_capture::{CaptureError, TrackKind};
use common::*;
use std::time::Duration;

#[test]
fn device_loss_gap_ends_at_reader_recovery() {
    let dir = tempfile::tempdir().unwrap();
    let session_dir = session_dir_under(dir.path());
    let (mic, handle) = ControlledSource::new(TrackKind::Mic, usize::MAX, SOURCE_PACE);
    handle.set_reconnect_delay(Duration::from_millis(300));
    let mut engine = CaptureEngine::new(
        SESSION_ID,
        &session_dir,
        buddy_core::clock::SessionClock::new(),
        Box::new(FakeAudioSource::from_samples(
            TrackKind::System,
            48_000,
            vec![0.5; 96_000],
        )),
        Box::new(mic),
        CaptureConfig::default(),
    )
    .unwrap();
    engine.start().unwrap();
    wait_for(|| handle.reads() >= 1);
    handle.inject(CaptureError::DeviceLost("slow-recovery".to_owned()));
    wait_for(|| handle.reconnects() >= 1);
    engine.stop().unwrap();

    let loss = read_gaps(&session_dir)
        .unwrap()
        .into_iter()
        .find(|gap| matches!(gap.reason, GapReason::DeviceLoss))
        .expect("a device-loss gap must be published");
    assert_eq!(loss.track, TrackKind::Mic);
    assert!(
        loss.end_ms.saturating_sub(loss.start_ms) >= 250,
        "the gap must span the recovery delay, not end when the writer drains it: {loss:?}"
    );
}
