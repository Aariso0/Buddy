mod common;

use buddy_capture::chunks::{read_chunk_index, read_gaps, GapReason};
use buddy_capture::engine::{CaptureConfig, CaptureEngine};
use buddy_capture::fake::FakeAudioSource;
use buddy_capture::TrackKind;
use common::*;
use std::time::Duration;

#[test]
fn overrun_records_gap_and_new_samples_reach_disk() {
    let dir = tempfile::tempdir().unwrap();
    let session_dir = session_dir_under(dir.path());
    let config = CaptureConfig {
        chunk_frames: 64,
        ring_frames: 8,
        ..CaptureConfig::default()
    };
    // Two back-to-back reads are enough to overrun the tiny ring. Slow reads
    // afterwards let the writer drain; an unbounded fast source floods
    // gaps.jsonl on slow CI filesystems and starves later samples.
    let (mic, handle) = ControlledSource::new(TrackKind::Mic, usize::MAX, Duration::from_millis(5));
    handle.set_burst(2);
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
        config,
    )
    .unwrap();
    engine.start().unwrap();
    // The third read cannot start until the second has already overrun. Waiting
    // on reads avoids polling gaps.jsonl, which the writer fsyncs per marker.
    wait_for(|| handle.reads() >= 3);
    let reads_at_switch = handle.reads();
    handle.set_value(0.75);
    wait_for(|| handle.reads() >= reads_at_switch + 8);
    // stop() joins the readers, then drains the rings and flushes pending
    // samples, so the distinguishable batch is on disk without polling chunks.
    engine.stop().unwrap();

    assert!(read_gaps(&session_dir)
        .unwrap()
        .iter()
        .any(|g| matches!(g.reason, GapReason::Overrun)));
    let mic_samples: Vec<f32> = read_chunk_index(&session_dir)
        .unwrap()
        .into_iter()
        .filter(|chunk| chunk.track == TrackKind::Mic)
        .flat_map(|chunk| read_chunk_samples(&session_dir, &chunk).unwrap())
        .collect();
    assert!(
        mic_samples.iter().any(|&sample| sample == 0.75),
        "samples captured after the overrun must reach disk: {mic_samples:?}"
    );
}
