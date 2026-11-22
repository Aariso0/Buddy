//! Windows WASAPI backend contract. These tests only compile on Windows; this
//! Linux host can verify only that the cfg gate keeps the rest of the workspace
//! building.

#![cfg(target_os = "windows")]

use buddy_capture::windows::WasapiSource;
use buddy_capture::{AudioSource, CaptureError, TrackKind};

fn absent_runtime(error: &CaptureError) -> bool {
    matches!(
        error,
        CaptureError::Backend(_)
            | CaptureError::DeviceLost(_)
            | CaptureError::UnsupportedPlatform(_)
    )
}

#[test]
fn wasapi_source_implements_contract() {
    match WasapiSource::list_devices(TrackKind::System) {
        Ok(devices) => {
            assert!(
                devices.iter().any(|device| device.id == "default"),
                "system devices must include the default render loopback: {devices:?}"
            );
            assert!(devices
                .iter()
                .all(|device| device.kind == TrackKind::System));
        }
        Err(error) => assert!(absent_runtime(&error), "unexpected error: {error}"),
    }

    let mut source = match WasapiSource::open("default") {
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
    assert!(count > 0, "a live source must deliver samples");
    assert!(count <= buffer.len());

    // The remembered device id must survive a re-arm.
    source
        .reconnect()
        .expect("reconnect must re-arm the same endpoint");
    assert_eq!(source.kind(), TrackKind::System);
    assert!(source.sample_rate() > 0);
    source.close().expect("close must be typed and succeed");
}

#[test]
fn wasapi_missing_endpoint_is_typed_error() {
    match WasapiSource::open("default") {
        Ok(_) => {}
        Err(error) => assert!(absent_runtime(&error), "unexpected error: {error}"),
    }

    match WasapiSource::open("definitely-not-a-wasapi-endpoint") {
        Ok(_) => panic!("unknown device must not open successfully"),
        Err(CaptureError::DeviceLost(_)) | Err(CaptureError::Backend(_)) => {}
        Err(error) => panic!("unexpected error: {error}"),
    }
}
