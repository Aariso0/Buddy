//! Windows WASAPI backend contract. These tests only compile on Windows; this
//! Linux host can verify only that the cfg gate keeps the rest of the workspace
//! building.
//!
//! `BUDDY_AUDIO_SOAK=1` turns a missing render endpoint into a hard failure.
//! Task 8's Windows soak must set it so a device-less runner cannot pass by
//! skipping both contract tests.

#![cfg(target_os = "windows")]

use buddy_capture::windows::WasapiSource;
use buddy_capture::{AudioSource, CaptureError, DeviceInfo, TrackKind};

const DEFAULT_ID: &str = "default";
const SOAK_ENV: &str = "BUDDY_AUDIO_SOAK";

fn soak_enabled() -> bool {
    std::env::var(SOAK_ENV)
        .map(|value| value == "1")
        .unwrap_or(false)
}

fn skip_or_fail(reason: &str) {
    if soak_enabled() {
        panic!("{SOAK_ENV}=1 requires {reason}");
    }
    eprintln!("SKIP: {reason}; set {SOAK_ENV}=1 to make this a hard failure");
}

/// Device presence comes only from `list_devices`. A `Backend` error from
/// `open` is never treated as absence: once a real System endpoint is listed,
/// the caller must assert the full contract and let any `open` failure fail.
fn system_devices_or_skip() -> Option<Vec<DeviceInfo>> {
    let devices = match WasapiSource::list_devices(TrackKind::System) {
        Ok(devices) => devices,
        Err(error) => {
            skip_or_fail(&format!(
                "WASAPI device enumeration to succeed (got {error})"
            ));
            return None;
        }
    };
    // `default` is a pseudo-device that list_devices always injects, so a
    // non-default id is the evidence that a real render endpoint exists.
    if !devices.iter().any(|device| device.id != DEFAULT_ID) {
        skip_or_fail("a real WASAPI render endpoint to be listed");
        return None;
    }
    Some(devices)
}

#[test]
fn wasapi_source_implements_contract() {
    let Some(devices) = system_devices_or_skip() else {
        return;
    };
    assert!(
        devices.iter().any(|device| device.id == DEFAULT_ID),
        "system devices must include the default render loopback: {devices:?}"
    );
    assert!(devices
        .iter()
        .all(|device| device.kind == TrackKind::System));

    // A listed System endpoint makes every assertion below mandatory. A
    // Backend error here is a broken backend, not a no-device skip.
    let mut source = WasapiSource::open(DEFAULT_ID)
        .unwrap_or_else(|error| panic!("listed System endpoint but open(default) failed: {error}"));
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
    let Some(devices) = system_devices_or_skip() else {
        return;
    };

    // Prove the backend can open a listed id before testing an unknown one;
    // otherwise a global Backend failure could satisfy both assertions.
    let valid_id = devices
        .iter()
        .find(|device| device.id != DEFAULT_ID)
        .map(|device| device.id.as_str())
        .expect("system_devices_or_skip guarantees a non-default id");
    let mut valid = WasapiSource::open(valid_id).unwrap_or_else(|error| {
        panic!("listed System endpoint {valid_id} failed to open: {error}")
    });
    assert_eq!(valid.kind(), TrackKind::System);
    assert!(
        valid.sample_rate() > 0,
        "listed endpoint must report a positive sample rate"
    );
    valid.close().expect("close must be typed and succeed");

    match WasapiSource::open("definitely-not-a-wasapi-endpoint") {
        Ok(_) => panic!("unknown device must not open successfully"),
        Err(CaptureError::DeviceLost(_)) | Err(CaptureError::Backend(_)) => {}
        Err(error) => panic!("unexpected error for unknown id: {error}"),
    }
}
