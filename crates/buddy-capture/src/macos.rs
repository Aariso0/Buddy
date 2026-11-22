//! macOS CoreAudio backend.
//!
//! `TrackKind::System` taps every CoreAudio process with a private
//! `ProcessTap` and reads the tap through a private aggregate device;
//! `TrackKind::Mic` reads the default input device. Both register an I/O proc
//! whose callback only pushes samples into a preallocated ring, so
//! [`AudioSource::read`] does no allocation, locking or blocking. A process tap
//! needs the user's System Audio Recording permission and the
//! `NSAudioCaptureUsageDescription` Info.plist key; without the grant the tap
//! is created but delivers silence. No virtual loopback device is used.
//!
//! Compilation and real capture are UNVERIFIED on the Linux host that builds
//! this repository. The macOS 14.2+ floor is enforced by the OS APIs and by
//! the pinned `coreaudio` 0.6.2 binding (see `NOTICE`).

use crate::ring::RingBuffer;
use crate::{AudioSource, CaptureError, DeviceInfo, TrackKind};
use coreaudio::{
    AppTapAggregateDevice, AudioObject, CoreAudioError, ErrorKind, ProcessTap, Scope, System,
    TapMute, DEVICE_NAME, DEVICE_NOMINAL_SAMPLE_RATE, DEVICE_UID, PROCESS_BUNDLE_ID,
};
use std::sync::atomic::{AtomicU32, AtomicU64, AtomicU8, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

const DEFAULT_ID: &str = "default";
const PROCESS_PREFIX: &str = "process:";
const MIC_PREFIX: &str = "mic:";
const SAMPLE_RATE: u32 = 48_000;
const CHANNELS: usize = 2;
const RING_FRAMES: usize = 8192;
const COMMAND_TIMEOUT: Duration = Duration::from_secs(3);
const DATA_WAIT: Duration = Duration::from_millis(500);

const LIVE: u8 = 0;
const DEVICE_LOST: u8 = 1;
const CLOSED: u8 = 2;
const FAILED: u8 = 3;

/// Aggregate UIDs must be unique among a process's private aggregates. A
/// counter avoids pulling in a UUID dependency just to name one.
static AGGREGATE_SEQUENCE: AtomicU64 = AtomicU64::new(0);

enum Command {
    Pause(Sender<Result<(), CaptureError>>),
    Resume(Sender<Result<(), CaptureError>>),
    Shutdown(Sender<Result<(), CaptureError>>),
}

#[derive(Clone)]
struct StreamSpec {
    kind: TrackKind,
    // Retained verbatim from `open` so reconnect never guesses another target.
    device_id: String,
    ring: Arc<RingBuffer>,
    sample_rate: Arc<AtomicU32>,
    state: Arc<AtomicU8>,
}

/// A private tap and the aggregate that makes it readable. Both fields are
/// held purely for their `Drop` side effects, and declaration order matters:
/// the aggregate must be destroyed before the tap it reads.
#[allow(dead_code)]
struct SystemTap {
    tap: ProcessTap,
    aggregate: AppTapAggregateDevice,
}

fn backend_error(message: impl Into<String>) -> CaptureError {
    CaptureError::Backend(message.into())
}

fn map_error(error: CoreAudioError) -> CaptureError {
    let detail = error.to_string();
    match error.kind() {
        ErrorKind::BadDevice
        | ErrorKind::BadObject
        | ErrorKind::BadStream
        | ErrorKind::NotRunning => CaptureError::DeviceLost(detail),
        ErrorKind::Permissions => CaptureError::DeviceBusy(detail),
        _ => backend_error(detail),
    }
}

fn decode_device_id(device_id: &str) -> Result<(TrackKind, &str), CaptureError> {
    if device_id.is_empty() {
        return Err(backend_error("empty CoreAudio device id"));
    }
    if device_id == DEFAULT_ID {
        return Ok((TrackKind::System, DEFAULT_ID));
    }
    if let Some(rest) = device_id.strip_prefix(MIC_PREFIX) {
        if rest.is_empty() {
            return Err(backend_error("empty CoreAudio microphone device id"));
        }
        return Ok((TrackKind::Mic, rest));
    }
    if let Some(rest) = device_id.strip_prefix(PROCESS_PREFIX) {
        if rest.is_empty() {
            return Err(backend_error("empty CoreAudio process id"));
        }
        return Ok((TrackKind::System, rest));
    }
    Err(backend_error(format!(
        "unrecognised CoreAudio device id: {device_id}"
    )))
}

fn encode_device_id(kind: &TrackKind, id: &str) -> String {
    match kind {
        TrackKind::System if id == DEFAULT_ID => DEFAULT_ID.to_owned(),
        TrackKind::System => format!("{PROCESS_PREFIX}{id}"),
        TrackKind::Mic => format!("{MIC_PREFIX}{id}"),
    }
}

fn process_name(process: &AudioObject<coreaudio::Process>) -> String {
    match process.get_property(PROCESS_BUNDLE_ID) {
        Ok(bundle_id) if !bundle_id.is_empty() => bundle_id,
        _ => format!("CoreAudio process {}", process.id()),
    }
}

fn enumerate_devices(kind: &TrackKind) -> Result<Vec<DeviceInfo>, CaptureError> {
    let system = AudioObject::<System>::default();
    let mut devices = vec![DeviceInfo {
        id: encode_device_id(kind, DEFAULT_ID),
        name: match kind {
            TrackKind::System => "Default system audio (CoreAudio process tap)".to_owned(),
            TrackKind::Mic => "Default microphone".to_owned(),
        },
        kind: kind.clone(),
    }];
    match kind {
        TrackKind::System => {
            for process in system.processes().map_err(map_error)? {
                devices.push(DeviceInfo {
                    id: encode_device_id(kind, &process.id().to_string()),
                    name: process_name(&process),
                    kind: kind.clone(),
                });
            }
        }
        TrackKind::Mic => {
            for device in system.devices_with_scope(Scope::Input).map_err(map_error)? {
                let uid = device.get_property(DEVICE_UID).map_err(map_error)?;
                let name = device
                    .get_property(DEVICE_NAME)
                    .unwrap_or_else(|_| uid.clone());
                devices.push(DeviceInfo {
                    id: encode_device_id(kind, &uid),
                    name,
                    kind: kind.clone(),
                });
            }
        }
    }
    Ok(devices)
}

/// Resolve the remembered id to a device we can attach an input I/O proc to.
/// The tap is returned alongside it so the caller keeps both alive until the
/// proc has been dropped.
fn resolve_device(
    spec: &StreamSpec,
) -> Result<(AudioObject<coreaudio::Device>, Option<SystemTap>), CaptureError> {
    let system = AudioObject::<System>::default();
    match spec.kind {
        TrackKind::Mic => {
            let device = if spec.device_id == DEFAULT_ID {
                system.current_device(Scope::Input).map_err(map_error)?
            } else {
                system
                    .devices_with_scope(Scope::Input)
                    .map_err(map_error)?
                    .into_iter()
                    .find(|device| {
                        device
                            .get_property(DEVICE_UID)
                            .is_ok_and(|uid: String| uid == spec.device_id)
                    })
                    .ok_or_else(|| {
                        CaptureError::DeviceLost(format!(
                            "CoreAudio input device not found: {}",
                            spec.device_id
                        ))
                    })?
            };
            Ok((device, None))
        }
        TrackKind::System => {
            let processes = if spec.device_id == DEFAULT_ID {
                system.processes().map_err(map_error)?
            } else {
                let id: u32 = spec.device_id.parse().map_err(|_| {
                    backend_error(format!("invalid CoreAudio process id: {}", spec.device_id))
                })?;
                system
                    .processes()
                    .map_err(map_error)?
                    .into_iter()
                    .filter(|process| process.id() == id)
                    .collect()
            };
            if processes.is_empty() {
                return Err(CaptureError::DeviceLost(format!(
                    "no CoreAudio process available to tap: {}",
                    spec.device_id
                )));
            }
            let name = "Buddy system audio tap";
            let tap = ProcessTap::stereo_mixdown(&processes, name, TapMute::Unmuted)
                .map_err(map_error)?;
            let sequence = AGGREGATE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
            let aggregate_uid = format!("buddy-system-{}-{sequence}", std::process::id());
            let aggregate = AppTapAggregateDevice::with_tap(name, &aggregate_uid, tap.uid())
                .map_err(map_error)?;
            let device = aggregate.device();
            Ok((device, Some(SystemTap { tap, aggregate })))
        }
    }
}

fn is_device_loss(error: &CaptureError) -> bool {
    matches!(error, CaptureError::DeviceLost(_))
}

fn run_worker(
    spec: StreamSpec,
    commands: Receiver<Command>,
    ready: Sender<Result<(), CaptureError>>,
) {
    let state = Arc::clone(&spec.state);
    if let Err(error) = build_and_run(spec, commands, &ready) {
        state.store(
            if is_device_loss(&error) {
                DEVICE_LOST
            } else {
                FAILED
            },
            Ordering::Release,
        );
        let _ = ready.send(Err(error));
    }
}

fn build_and_run(
    spec: StreamSpec,
    commands: Receiver<Command>,
    ready: &Sender<Result<(), CaptureError>>,
) -> Result<(), CaptureError> {
    let (device, tap) = resolve_device(&spec)?;
    let sample_rate = device
        .get_property(DEVICE_NOMINAL_SAMPLE_RATE)
        .map(|rate: f64| rate.round() as u32)
        .unwrap_or(SAMPLE_RATE);
    spec.sample_rate
        .store(sample_rate.max(1), Ordering::Release);

    let ring = Arc::clone(&spec.ring);
    let mut proc = device
        .add_io_proc(Scope::Input, move |buffers| {
            for buffer in buffers {
                // The ring push is allocation-free and never blocks; a full ring
                // drops this cycle's samples, which the engine records as a gap.
                let _ = ring.push(buffer.data);
            }
        })
        .map_err(map_error)?;
    proc.play().map_err(map_error)?;
    let _ = ready.send(Ok(()));

    let result = 'stream: loop {
        match commands.recv_timeout(Duration::from_millis(100)) {
            Ok(Command::Pause(reply)) => {
                let result = proc.pause().map_err(map_error);
                let _ = reply.send(result);
            }
            Ok(Command::Resume(reply)) => {
                let result = proc.play().map_err(map_error);
                let _ = reply.send(result);
            }
            Ok(Command::Shutdown(reply)) => {
                let _ = reply.send(Ok(()));
                break 'stream Ok(());
            }
            Err(RecvTimeoutError::Timeout) | Err(RecvTimeoutError::Disconnected) => {}
        }
    };

    let _ = proc.pause();
    // Teardown order: I/O proc, aggregate device, tap.
    drop(proc);
    drop(tap);
    result
}

fn spawn_worker(spec: StreamSpec) -> Result<(Sender<Command>, JoinHandle<()>), CaptureError> {
    let (ready_tx, ready_rx) = mpsc::channel();
    let (commands, command_rx) = mpsc::channel();
    let worker = thread::Builder::new()
        .name("buddy-coreaudio".to_owned())
        .spawn(move || run_worker(spec, command_rx, ready_tx))
        .map_err(|error| backend_error(format!("start CoreAudio worker: {error}")))?;

    match ready_rx.recv_timeout(COMMAND_TIMEOUT) {
        Ok(Ok(())) => Ok((commands, worker)),
        Ok(Err(error)) => {
            let _ = worker.join();
            Err(error)
        }
        Err(RecvTimeoutError::Timeout) | Err(RecvTimeoutError::Disconnected) => {
            Err(backend_error("CoreAudio worker did not connect"))
        }
    }
}

/// One track's live CoreAudio capture. `open` remembers the exact device id so
/// `reconnect` re-arms the same endpoint after a device loss.
pub struct CoreAudioSource {
    kind: TrackKind,
    device_id: String,
    ring: Arc<RingBuffer>,
    sample_rate: Arc<AtomicU32>,
    state: Arc<AtomicU8>,
    commands: Option<Sender<Command>>,
    worker: Option<JoinHandle<()>>,
    // Prebuilt so failure paths in `read` do not allocate.
    device_lost_error: Option<CaptureError>,
    failure_error: Option<CaptureError>,
    stalled_error: Option<CaptureError>,
}

impl CoreAudioSource {
    /// Open a known track directly. The trait's `open` infers the track from
    /// the device id; callers that already know it can use this instead.
    pub fn open_for(kind: TrackKind, device_id: &str) -> Result<Self, CaptureError> {
        if device_id.is_empty() {
            return Err(backend_error("empty CoreAudio device id"));
        }
        let ring = Arc::new(RingBuffer::new(RING_FRAMES, CHANNELS));
        let sample_rate = Arc::new(AtomicU32::new(SAMPLE_RATE));
        let state = Arc::new(AtomicU8::new(LIVE));
        let spec = StreamSpec {
            kind: kind.clone(),
            device_id: device_id.to_owned(),
            ring: Arc::clone(&ring),
            sample_rate: Arc::clone(&sample_rate),
            state: Arc::clone(&state),
        };
        let (commands, worker) = spawn_worker(spec)?;
        Ok(Self {
            kind,
            device_id: device_id.to_owned(),
            ring,
            sample_rate,
            state,
            commands: Some(commands),
            worker: Some(worker),
            device_lost_error: Some(CaptureError::DeviceLost(
                "CoreAudio endpoint was removed".to_owned(),
            )),
            failure_error: Some(backend_error("CoreAudio capture is no longer live")),
            stalled_error: Some(backend_error(
                "CoreAudio capture stalled: no samples before deadline",
            )),
        })
    }

    fn command(
        &self,
        make: impl FnOnce(Sender<Result<(), CaptureError>>) -> Command,
    ) -> Result<(), CaptureError> {
        let commands = self
            .commands
            .as_ref()
            .ok_or_else(|| backend_error("CoreAudio source is closed"))?;
        let (reply_tx, reply_rx) = mpsc::channel();
        commands
            .send(make(reply_tx))
            .map_err(|_| backend_error("CoreAudio worker is gone"))?;
        match reply_rx.recv_timeout(COMMAND_TIMEOUT) {
            Ok(result) => result,
            Err(RecvTimeoutError::Timeout) => Err(backend_error("CoreAudio command timed out")),
            Err(RecvTimeoutError::Disconnected) => Err(backend_error("CoreAudio worker stopped")),
        }
    }

    fn shutdown_worker(&mut self) -> Result<(), CaptureError> {
        let result = self.command(Command::Shutdown);
        if let Some(worker) = self.worker.take() {
            worker
                .join()
                .map_err(|_| backend_error("CoreAudio worker panicked"))?;
        }
        self.commands = None;
        result
    }

    fn failure(&mut self, lost: bool) -> CaptureError {
        let slot = if lost {
            &mut self.device_lost_error
        } else {
            &mut self.failure_error
        };
        slot.take().unwrap_or_else(|| backend_error(String::new()))
    }
}

impl AudioSource for CoreAudioSource {
    fn kind(&self) -> TrackKind {
        self.kind.clone()
    }

    fn sample_rate(&self) -> u32 {
        self.sample_rate.load(Ordering::Acquire)
    }

    fn list_devices(kind: TrackKind) -> Result<Vec<DeviceInfo>, CaptureError> {
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            let _ = tx.send(enumerate_devices(&kind));
        });
        match rx.recv_timeout(COMMAND_TIMEOUT) {
            Ok(result) => result,
            Err(_) => Err(backend_error("CoreAudio device enumeration timed out")),
        }
    }

    fn open(device_id: &str) -> Result<Self, CaptureError> {
        let (kind, id) = decode_device_id(device_id)?;
        Self::open_for(kind, id)
    }

    fn read(&mut self, out: &mut [f32]) -> Result<usize, CaptureError> {
        if out.is_empty() {
            return Ok(0);
        }
        let deadline = Instant::now() + DATA_WAIT;
        loop {
            match self.state.load(Ordering::Acquire) {
                DEVICE_LOST => return Err(self.failure(true)),
                CLOSED | FAILED => return Err(self.failure(false)),
                _ => {}
            }
            if let Some(count) = self.ring.pop(out) {
                return Ok(count);
            }
            if Instant::now() >= deadline {
                return Err(self
                    .stalled_error
                    .take()
                    .unwrap_or_else(|| backend_error(String::new())));
            }
            // Never report Ok(0) as exhaustion for a live source, and never
            // block in `read`: the worker is the only thread that waits.
            std::thread::yield_now();
        }
    }

    fn pause(&mut self) -> Result<(), CaptureError> {
        self.command(Command::Pause)
    }

    fn resume(&mut self) -> Result<(), CaptureError> {
        self.command(Command::Resume)
    }

    fn reconnect(&mut self) -> Result<(), CaptureError> {
        self.shutdown_worker()?;
        let mut scratch = [0.0f32; 512];
        while self.ring.pop(&mut scratch).is_some() {}

        // The remembered id is used unchanged: `default` re-resolves the
        // current CoreAudio process set or input device, an explicit id names
        // the same process or device.
        let spec = StreamSpec {
            kind: self.kind.clone(),
            device_id: self.device_id.clone(),
            ring: Arc::clone(&self.ring),
            sample_rate: Arc::clone(&self.sample_rate),
            state: Arc::clone(&self.state),
        };
        self.state.store(LIVE, Ordering::Release);
        self.device_lost_error = Some(CaptureError::DeviceLost(
            "CoreAudio endpoint was removed".to_owned(),
        ));
        self.failure_error = Some(backend_error("CoreAudio capture is no longer live"));
        self.stalled_error = Some(backend_error(
            "CoreAudio capture stalled: no samples before deadline",
        ));
        match spawn_worker(spec) {
            Ok((commands, worker)) => {
                self.commands = Some(commands);
                self.worker = Some(worker);
                Ok(())
            }
            Err(error) => {
                self.state.store(FAILED, Ordering::Release);
                Err(error)
            }
        }
    }

    fn close(&mut self) -> Result<(), CaptureError> {
        let result = self.shutdown_worker();
        self.state.store(CLOSED, Ordering::Release);
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn device_ids_encode_the_track_without_a_separate_argument() {
        assert_eq!(
            decode_device_id(DEFAULT_ID).unwrap(),
            (TrackKind::System, DEFAULT_ID)
        );
        assert_eq!(
            decode_device_id("mic:default").unwrap(),
            (TrackKind::Mic, DEFAULT_ID)
        );
        assert_eq!(
            decode_device_id("process:42").unwrap(),
            (TrackKind::System, "42")
        );
        assert!(decode_device_id("").is_err());
        assert!(decode_device_id("mic:").is_err());
        assert!(decode_device_id("process:").is_err());
        assert!(decode_device_id("unknown:42").is_err());
        assert_eq!(encode_device_id(&TrackKind::Mic, "42"), "mic:42");
        assert_eq!(encode_device_id(&TrackKind::System, "42"), "process:42");
        assert_eq!(encode_device_id(&TrackKind::System, DEFAULT_ID), DEFAULT_ID);
    }

    #[test]
    fn device_loss_errors_map_to_the_typed_variant() {
        assert!(matches!(
            map_error(CoreAudioError::from(-1)),
            CaptureError::Backend(_)
        ));
    }
}
