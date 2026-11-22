//! Linux PipeWire backend contract. PipeWire is optional on the test host, so
//! an absent server is a valid typed outcome rather than a failed assertion.

#![cfg(target_os = "linux")]

use buddy_capture::linux::PipeWireSource;
use buddy_capture::{AudioSource, CaptureError, TrackKind};

fn absent_runtime(error: &CaptureError) -> bool {
    matches!(
        error,
        CaptureError::Backend(_) | CaptureError::UnsupportedPlatform(_)
    )
}

#[test]
fn pipewire_source_implements_contract() {
    match PipeWireSource::list_devices(TrackKind::System) {
        Ok(devices) => {
            assert!(
                devices.iter().any(|device| device.id == "default"),
                "system devices must include the default sink monitor: {devices:?}"
            );
            assert!(devices
                .iter()
                .all(|device| device.kind == TrackKind::System));
        }
        Err(error) => assert!(absent_runtime(&error), "unexpected error: {error}"),
    }

    let mut source = match PipeWireSource::open("default") {
        Ok(source) => source,
        Err(error) => {
            assert!(absent_runtime(&error), "unexpected error: {error}");
            return;
        }
    };
    assert_eq!(source.kind(), TrackKind::System);
    assert!(source.sample_rate() > 0, "sample rate must be positive");

    let mut buffer = vec![0.0f32; 64];
    let count = source.read(&mut buffer).expect("live read must not fail");
    assert!(count <= buffer.len());

    // The remembered device id must survive a re-arm: the reconnected source
    // still reports the same track and a usable format.
    source
        .reconnect()
        .expect("reconnect must re-arm the source");
    assert_eq!(source.kind(), TrackKind::System);
    assert!(source.sample_rate() > 0);
    source.close().expect("close must be typed and succeed");
}

#[test]
fn pipewire_reports_missing_pipewire_as_typed_error() {
    // No runtime means a typed absence; a running runtime means a real source.
    // Both are allowed here. Real capture is proven by the M1 Linux soak.
    match PipeWireSource::open("default") {
        Ok(_) => {}
        Err(error) => assert!(absent_runtime(&error), "unexpected error: {error}"),
    }

    // An unresolvable device is never an empty Ok and never a panic.
    match PipeWireSource::open("definitely-not-a-pipewire-node") {
        Ok(_) => panic!("unknown device must not open successfully"),
        Err(CaptureError::Backend(_))
        | Err(CaptureError::DeviceLost(_))
        | Err(CaptureError::UnsupportedPlatform(_)) => {}
        Err(error) => panic!("unexpected error: {error}"),
    }
}

#[test]
fn pipewire_rejects_unknown_numeric_device_id() {
    let devices = match PipeWireSource::list_devices(TrackKind::System) {
        Ok(devices) => devices,
        Err(error) => {
            assert!(absent_runtime(&error), "unexpected error: {error}");
            // Headless hosts still reject an explicit id as a typed error.
            match PipeWireSource::open("999999") {
                Ok(_) => panic!("unknown device id must not open"),
                Err(error) => assert!(absent_runtime(&error), "unexpected error: {error}"),
            }
            return;
        }
    };

    // Live daemon: an id absent from the registry must fail before a stream
    // is created, rather than reporting success on a device that never links.
    for id in ["999999", "mic:999999"] {
        match PipeWireSource::open(id) {
            Ok(_) => panic!("unknown device id {id} must not open"),
            Err(CaptureError::Backend(_)) | Err(CaptureError::DeviceLost(_)) => {}
            Err(error) => panic!("unexpected error for {id}: {error}"),
        }
    }

    // A registry-backed id still opens and reports the right track.
    let valid = devices
        .iter()
        .find(|device| device.id != "default")
        .map(|device| device.id.clone())
        .unwrap_or_else(|| "default".to_owned());
    let mut source = PipeWireSource::open(&valid)
        .unwrap_or_else(|error| panic!("valid id {valid} must open: {error}"));
    assert_eq!(source.kind(), TrackKind::System);
    source.close().expect("close must succeed");
}
