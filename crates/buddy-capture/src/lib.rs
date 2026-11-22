//! Platform-independent audio capture primitives.
//!
//! Per-OS backends implement [`AudioSource`]; the engine drives any source
//! through that trait and stamps its chunks with the monotonic session clock.

pub mod chunks;
pub mod engine;
pub mod fake;
#[cfg(target_os = "linux")]
pub mod linux;
pub mod ring;

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// The two audio tracks recorded for a session, always kept separate.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TrackKind {
    System,
    Mic,
}

/// A device a backend can open. Serializable so Tauri commands can return it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DeviceInfo {
    pub id: String,
    pub name: String,
    pub kind: TrackKind,
}

#[derive(Debug, Error)]
pub enum CaptureError {
    #[error("audio device lost: {0}")]
    DeviceLost(String),
    #[error("audio device busy: {0}")]
    DeviceBusy(String),
    #[error("unsupported platform: {0}")]
    UnsupportedPlatform(String),
    #[error("audio backend error: {0}")]
    Backend(String),
    #[error("invalid audio chunk: {0}")]
    InvalidChunk(String),
    #[error("audio io error: {0}")]
    Io(#[from] std::io::Error),
}

/// One track's audio input. Implemented by each platform backend and by the
/// fake source used in CI contract tests.
pub trait AudioSource: Send {
    fn kind(&self) -> TrackKind;
    fn sample_rate(&self) -> u32;

    fn list_devices(kind: TrackKind) -> Result<Vec<DeviceInfo>, CaptureError>
    where
        Self: Sized;

    fn open(device_id: &str) -> Result<Self, CaptureError>
    where
        Self: Sized;

    /// Fill `out` with the next samples and return how many were written (zero
    /// once the source is exhausted). The caller owns the buffer: the read
    /// path must not allocate or lock, and it carries no timestamps because
    /// the engine applies the session clock when it writes chunks.
    fn read(&mut self, out: &mut [f32]) -> Result<usize, CaptureError>;

    fn pause(&mut self) -> Result<(), CaptureError>;
    fn resume(&mut self) -> Result<(), CaptureError>;

    /// Re-arm this source after a device loss. Backends remember their own
    /// device id; on success the source must be ready to read again, otherwise
    /// this returns a typed error. Object-safe so the engine can call it on a
    /// boxed source.
    fn reconnect(&mut self) -> Result<(), CaptureError>;

    fn close(&mut self) -> Result<(), CaptureError>;
}
