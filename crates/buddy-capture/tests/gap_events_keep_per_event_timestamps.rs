mod common;

use buddy_capture::chunks::{read_gaps, GapReason};
use buddy_capture::engine::{CaptureConfig, CaptureEngine};
use buddy_capture::fake::FakeAudioSource;
use buddy_capture::{CaptureError, TrackKind};
use common::*;
use std::time::Duration;

#[test]
fn gap_events_keep_per_event_timestamps() {
    let dir = tempfile::tempdir().unwrap();
    let session_dir = session_dir_under(dir.path());
    let (mic, handle) = ControlledSource::new(TrackKind::Mic, usize::MAX, Duration::from_millis(6));
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
    for i in 0..3 {
        let before = handle.reconnects();
        handle.inject(CaptureError::DeviceLost(format!("mic-{i}")));
        wait_for(|| handle.reconnects() > before);
    }
    engine.stop().unwrap();

    let losses: Vec<_> = read_gaps(&session_dir)
        .unwrap()
        .into_iter()
        .filter(|gap| matches!(gap.reason, GapReason::DeviceLoss))
        .collect();
    assert!(
        losses.len() >= 3,
        "expected one marker per loss event, got {}",
        losses.len()
    );
    let starts: Vec<u64> = losses.iter().map(|gap| gap.start_ms).collect();
    assert!(
        starts.windows(2).all(|pair| pair[0] <= pair[1]),
        "gap timestamps must be monotonic: {starts:?}"
    );
    assert!(
        starts.iter().any(|&start| start > starts[0]),
        "each gap must keep its own detection time, not a shared overwritten slot: {starts:?}"
    );
}
