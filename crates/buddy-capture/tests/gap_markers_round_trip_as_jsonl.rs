mod common;

use buddy_capture::chunks::{append_gap, read_gaps, GapMarker, GapReason};
use buddy_capture::TrackKind;

#[test]
fn gap_markers_round_trip_as_jsonl() {
    let dir = tempfile::tempdir().unwrap();
    append_gap(
        dir.path(),
        &GapMarker {
            track: TrackKind::Mic,
            start_ms: 1200,
            end_ms: 1350,
            reason: GapReason::DeviceLoss,
        },
    )
    .unwrap();
    let gaps = read_gaps(dir.path()).unwrap();
    assert_eq!(gaps.len(), 1);
    assert_eq!(gaps[0].track, TrackKind::Mic);
    assert_eq!(gaps[0].start_ms, 1200);
    assert_eq!(gaps[0].end_ms, 1350);
    assert!(matches!(gaps[0].reason, GapReason::DeviceLoss));
}
