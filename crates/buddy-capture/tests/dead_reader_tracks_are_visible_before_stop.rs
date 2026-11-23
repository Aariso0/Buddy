mod common;

use buddy_capture::engine::{CaptureConfig, CaptureEngine};
use buddy_capture::TrackKind;
use common::*;
use std::time::Duration;

#[test]
fn dead_reader_tracks_are_visible_before_stop() {
    let dir = tempfile::tempdir().unwrap();
    let session_dir = session_dir_under(dir.path());
    let (system, _system_handle) =
        ControlledSource::new(TrackKind::System, usize::MAX, SOURCE_PACE);
    let (mic, _mic_handle) = ControlledSource::new(TrackKind::Mic, 8, Duration::ZERO);
    let mut engine = CaptureEngine::new(
        SESSION_ID,
        &session_dir,
        buddy_core::clock::SessionClock::new(),
        Box::new(system),
        Box::new(mic),
        CaptureConfig::default(),
    )
    .unwrap();
    engine.start().unwrap();
    wait_for(|| !engine.track_is_alive(&TrackKind::Mic));
    assert!(engine.track_is_alive(&TrackKind::System));
    engine.stop().unwrap();
    assert!(!engine.track_is_alive(&TrackKind::Mic));
}
