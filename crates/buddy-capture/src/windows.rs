//! Windows WASAPI backend.
//!
//! `TrackKind::System` opens a shared-mode loopback stream on the default
//! render endpoint; `TrackKind::Mic` opens the default capture endpoint. WASAPI
//! COM objects stay on one worker thread (they are apartment-bound), which
//! fills a preallocated ring. [`AudioSource::read`] only drains that ring with
//! atomics: no allocation, lock, blocking call or network access.

use crate::ring::RingBuffer;
use crate::{AudioSource, CaptureError, DeviceInfo, TrackKind};
use std::sync::atomic::{AtomicU32, AtomicU8, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender, TryRecvError};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};
use wasapi::{
    deinitialize, initialize_mta, DeviceEnumerator, DeviceState, Direction, SampleType, StreamMode,
    WasapiError, WaveFormat,
};

const DEFAULT_ID: &str = "default";
const MIC_PREFIX: &str = "mic:";
const SAMPLE_RATE: u32 = 48_000;
const CHANNELS: usize = 2;
const RING_FRAMES: usize = 8192;
const SAMPLE_BATCH: usize = 512;
const EVENT_WAIT_MS: u32 = 100;
const COMMAND_TIMEOUT: Duration = Duration::from_secs(3);
const DATA_WAIT: Duration = Duration::from_millis(500);

const LIVE: u8 = 0;
const DEVICE_LOST: u8 = 1;
const CLOSED: u8 = 2;
const FAILED: u8 = 3;

// WASAPI/AUDCLNT HRESULTs, read from the windows 0.62.2 bindings that wasapi
// 0.27.0 is built against. Kept numeric so this crate needs no second Windows
// dependency just to classify device loss.
const AUDCLNT_E_DEVICE_INVALIDATED: i32 = 0x8889_0004u32 as i32;
const AUDCLNT_E_DEVICE_IN_USE: i32 = 0x8889_000Au32 as i32;
const AUDCLNT_E_EXCLUSIVE_MODE_NOT_ALLOWED: i32 = 0x8889_000Eu32 as i32;
const AUDCLNT_E_ENDPOINT_CREATE_FAILED: i32 = 0x8889_000Fu32 as i32;
const AUDCLNT_E_SERVICE_NOT_RUNNING: i32 = 0x8889_0010u32 as i32;
const AUDCLNT_E_RESOURCES_INVALIDATED: i32 = 0x8889_0026u32 as i32;
const E_NOTFOUND: i32 = 0x8007_0490u32 as i32;
const RPC_E_CHANGED_MODE: i32 = 0x8001_0106u32 as i32;

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

/// COM must be initialized on every thread that creates or uses WASAPI objects.
struct ComGuard {
    initialized: bool,
}

impl ComGuard {
    fn new() -> Result<Self, CaptureError> {
        let result = initialize_mta();
        // S_OK means this call owns the reference that `deinitialize` releases.
        // S_FALSE means COM was already initialised for MTA; that is usable but
        // must not be uninitialised here or the caller's balance is broken.
        if result.0 == 0 {
            return Ok(Self { initialized: true });
        }
        if result.0 == 1 || result.0 == RPC_E_CHANGED_MODE {
            return Ok(Self { initialized: false });
        }
        Err(backend_error(format!(
            "WASAPI COM initialization failed: HRESULT 0x{:08X}",
            result.0 as u32
        )))
    }
}

impl Drop for ComGuard {
    fn drop(&mut self) {
        if self.initialized {
            deinitialize();
        }
    }
}

fn backend_error(message: impl Into<String>) -> CaptureError {
    CaptureError::Backend(message.into())
}

fn direction_for(kind: &TrackKind) -> Direction {
    match kind {
        TrackKind::System => Direction::Render,
        TrackKind::Mic => Direction::Capture,
    }
}

fn decode_device_id(device_id: &str) -> Result<(TrackKind, &str), CaptureError> {
    if device_id.is_empty() {
        return Err(backend_error("empty WASAPI device id"));
    }
    if device_id == DEFAULT_ID {
        return Ok((TrackKind::System, DEFAULT_ID));
    }
    match device_id.strip_prefix(MIC_PREFIX) {
        Some(rest) if !rest.is_empty() => Ok((TrackKind::Mic, rest)),
        Some(_) => Err(backend_error("empty WASAPI microphone device id")),
        None => Ok((TrackKind::System, device_id)),
    }
}

fn encode_device_id(kind: &TrackKind, id: &str) -> String {
    match kind {
        TrackKind::System => id.to_owned(),
        TrackKind::Mic => format!("{MIC_PREFIX}{id}"),
    }
}

fn map_error(error: WasapiError) -> CaptureError {
    match error {
        WasapiError::DeviceNotFound(id) => {
            CaptureError::DeviceLost(format!("WASAPI endpoint not found: {id}"))
        }
        WasapiError::IllegalDeviceState(code) => {
            CaptureError::DeviceLost(format!("WASAPI endpoint state is invalid: {code}"))
        }
        WasapiError::Windows(error) => match error.code().0 {
            code @ (AUDCLNT_E_DEVICE_INVALIDATED
            | AUDCLNT_E_ENDPOINT_CREATE_FAILED
            | AUDCLNT_E_SERVICE_NOT_RUNNING
            | AUDCLNT_E_RESOURCES_INVALIDATED
            | E_NOTFOUND) => CaptureError::DeviceLost(format!(
                "WASAPI endpoint was removed: HRESULT 0x{:08X}",
                code as u32
            )),
            code @ (AUDCLNT_E_DEVICE_IN_USE | AUDCLNT_E_EXCLUSIVE_MODE_NOT_ALLOWED) => {
                CaptureError::DeviceBusy(format!(
                    "WASAPI endpoint is unavailable: HRESULT 0x{:08X}",
                    code as u32
                ))
            }
            _ => backend_error(format!("WASAPI error: {error}")),
        },
        WasapiError::LoopbackWithExclusiveMode => backend_error(
            "WASAPI loopback requires shared mode; an exclusive-mode stream cannot be captured",
        ),
        _ => backend_error(format!("WASAPI error: {error}")),
    }
}

fn is_device_loss(error: &CaptureError) -> bool {
    matches!(error, CaptureError::DeviceLost(_))
}

fn resolve_device(spec: &StreamSpec) -> Result<wasapi::Device, CaptureError> {
    let enumerator = DeviceEnumerator::new().map_err(map_error)?;
    if spec.device_id == DEFAULT_ID {
        enumerator
            .get_default_device(direction_for(&spec.kind))
            .map_err(map_error)
    } else {
        enumerator.get_device(&spec.device_id).map_err(map_error)
    }
}

fn enumerate_devices(kind: &TrackKind) -> Result<Vec<DeviceInfo>, CaptureError> {
    let _com = ComGuard::new()?;
    let enumerator = DeviceEnumerator::new().map_err(map_error)?;
    let collection = enumerator
        .get_device_collection(direction_for(kind))
        .map_err(map_error)?;
    let mut devices = vec![DeviceInfo {
        id: encode_device_id(kind, DEFAULT_ID),
        name: match kind {
            TrackKind::System => "Default system audio (WASAPI loopback)".to_owned(),
            TrackKind::Mic => "Default microphone".to_owned(),
        },
        kind: kind.clone(),
    }];
    for index in 0..collection.get_nbr_devices().map_err(map_error)? {
        let device = collection.get_device_at_index(index).map_err(map_error)?;
        if device.get_state().map_err(map_error)? != DeviceState::Active {
            continue;
        }
        let id = device.get_id().map_err(map_error)?;
        let name = device.get_friendlyname().unwrap_or_else(|_| id.clone());
        devices.push(DeviceInfo {
            id: encode_device_id(kind, &id),
            name,
            kind: kind.clone(),
        });
    }
    Ok(devices)
}

/// Copy one WASAPI packet into the ring in fixed stack-sized batches. The byte
/// scratch buffer is allocated once by the worker, never in this loop.
fn copy_packet(bytes: &[u8], samples: &mut [f32; SAMPLE_BATCH], ring: &RingBuffer) {
    let mut filled = 0;
    for chunk in bytes.chunks_exact(4) {
        samples[filled] = f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
        filled += 1;
        if filled == samples.len() {
            let _ = ring.push(&samples[..]);
            filled = 0;
        }
    }
    if filled > 0 {
        let _ = ring.push(&samples[..filled]);
    }
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
    let _com = ComGuard::new()?;
    let device = resolve_device(&spec)?;
    let mut client = device.get_iaudioclient().map_err(map_error)?;

    // Shared mode is mandatory for loopback and is also the only mode that
    // coexists with other applications. `Direction::Render` on the client plus
    // `Direction::Capture` here makes wasapi set AUDCLNT_STREAMFLAGS_LOOPBACK.
    let format = WaveFormat::new(
        32,
        32,
        SampleType::Float,
        SAMPLE_RATE as usize,
        CHANNELS,
        None,
    );
    let (_, min_period) = client.get_device_period().map_err(map_error)?;
    client
        .initialize_client(
            &format,
            Direction::Capture,
            StreamMode::EventsShared {
                autoconvert: true,
                buffer_duration_hns: min_period,
            },
            None,
        )
        .map_err(map_error)?;
    spec.sample_rate
        .store(format.get_samplespersec(), Ordering::Release);

    let event = client.set_get_eventhandle().map_err(map_error)?;
    let buffer_frames = client.get_buffer_size().map_err(map_error)?;
    let capture = client.get_audiocaptureclient().map_err(map_error)?;
    let bytes_per_frame = format.get_blockalign() as usize;
    let mut scratch = vec![0u8; (buffer_frames as usize).max(1) * bytes_per_frame];
    let mut samples = [0.0f32; SAMPLE_BATCH];
    let mut paused = false;

    client.start_stream().map_err(map_error)?;
    let _ = ready.send(Ok(()));

    let result = 'stream: loop {
        match commands.try_recv() {
            Ok(Command::Pause(reply)) => {
                let result = client.stop_stream().map_err(map_error);
                if result.is_ok() {
                    paused = true;
                }
                let _ = reply.send(result);
            }
            Ok(Command::Resume(reply)) => {
                let result = client.start_stream().map_err(map_error);
                if result.is_ok() {
                    paused = false;
                }
                let _ = reply.send(result);
            }
            Ok(Command::Shutdown(reply)) => {
                let _ = reply.send(Ok(()));
                break Ok(());
            }
            Err(TryRecvError::Empty) => {}
            Err(TryRecvError::Disconnected) => break Ok(()),
        }

        if let Err(error) = event.wait_for_event(EVENT_WAIT_MS) {
            // A timeout just means no audio period has elapsed yet; any other
            // error is a real device or COM failure.
            if !matches!(&error, WasapiError::EventTimeout) {
                break Err(map_error(error));
            }
            continue;
        }
        if paused {
            continue;
        }

        loop {
            let Some(packet) = capture.get_next_packet_size().map_err(map_error)? else {
                break;
            };
            if packet == 0 {
                break;
            }
            let needed = packet as usize * bytes_per_frame;
            if needed > scratch.len() {
                break 'stream Err(backend_error("WASAPI packet exceeds the capture buffer"));
            }
            let (frames, _) = capture
                .read_from_device(&mut scratch[..needed])
                .map_err(map_error)?;
            if frames == 0 {
                break;
            }
            copy_packet(
                &scratch[..frames as usize * bytes_per_frame],
                &mut samples,
                &spec.ring,
            );
        }
    };

    let _ = client.stop_stream();
    result
}

fn spawn_worker(spec: StreamSpec) -> Result<(Sender<Command>, JoinHandle<()>), CaptureError> {
    let (ready_tx, ready_rx) = mpsc::channel();
    let (commands, command_rx) = mpsc::channel();
    let worker = thread::Builder::new()
        .name("buddy-wasapi".to_owned())
        .spawn(move || run_worker(spec, command_rx, ready_tx))
        .map_err(|error| backend_error(format!("start WASAPI worker: {error}")))?;

    match ready_rx.recv_timeout(COMMAND_TIMEOUT) {
        Ok(Ok(())) => Ok((commands, worker)),
        Ok(Err(error)) => {
            let _ = worker.join();
            Err(error)
        }
        Err(RecvTimeoutError::Timeout) | Err(RecvTimeoutError::Disconnected) => {
            // Do not join a worker that may still be waiting inside COM setup.
            Err(backend_error("WASAPI worker did not connect"))
        }
    }
}

/// One track's live WASAPI capture. `open` remembers the exact device id so
/// `reconnect` re-arms the same endpoint after a device loss.
pub struct WasapiSource {
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

impl WasapiSource {
    /// Open a known track directly. The trait's `open` infers the track from
    /// the device id; callers that already know it can use this instead.
    pub fn open_for(kind: TrackKind, device_id: &str) -> Result<Self, CaptureError> {
        if device_id.is_empty() {
            return Err(backend_error("empty WASAPI device id"));
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
                "WASAPI endpoint was removed".to_owned(),
            )),
            failure_error: Some(backend_error("WASAPI capture is no longer live")),
            stalled_error: Some(backend_error(
                "WASAPI capture stalled: no samples before deadline",
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
            .ok_or_else(|| backend_error("WASAPI source is closed"))?;
        let (reply_tx, reply_rx) = mpsc::channel();
        commands
            .send(make(reply_tx))
            .map_err(|_| backend_error("WASAPI worker is gone"))?;
        match reply_rx.recv_timeout(COMMAND_TIMEOUT) {
            Ok(result) => result,
            Err(RecvTimeoutError::Timeout) => Err(backend_error("WASAPI command timed out")),
            Err(RecvTimeoutError::Disconnected) => Err(backend_error("WASAPI worker stopped")),
        }
    }

    fn shutdown_worker(&mut self) -> Result<(), CaptureError> {
        // A failed worker has already dropped its command receiver; that is not
        // an error when shutting down or re-arming after a loss.
        let result = match self.command(Command::Shutdown) {
            Ok(()) | Err(CaptureError::Backend(_)) => Ok(()),
            Err(error) => Err(error),
        };
        if let Some(worker) = self.worker.take() {
            worker
                .join()
                .map_err(|_| backend_error("WASAPI worker panicked"))?;
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
        // String::new() does not allocate; a second failure after the prebuilt
        // error was returned must still stay allocation-free in `read`.
        slot.take().unwrap_or_else(|| backend_error(String::new()))
    }
}

impl AudioSource for WasapiSource {
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
            Err(_) => Err(backend_error("WASAPI device enumeration timed out")),
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
        let mut scratch = [0.0f32; SAMPLE_BATCH];
        while self.ring.pop(&mut scratch).is_some() {}

        // The remembered id is used unchanged: `default` re-resolves the
        // current default endpoint, an explicit id names the same endpoint.
        // Each worker generation owns its status cell. A timed-out setup
        // leaves the old worker detached, so its later terminal store must not
        // be observed as the replacement worker's status.
        let state = Arc::new(AtomicU8::new(LIVE));
        let spec = StreamSpec {
            kind: self.kind.clone(),
            device_id: self.device_id.clone(),
            ring: Arc::clone(&self.ring),
            sample_rate: Arc::clone(&self.sample_rate),
            state: Arc::clone(&state),
        };
        self.state = state;
        self.device_lost_error = Some(CaptureError::DeviceLost(
            "WASAPI endpoint was removed".to_owned(),
        ));
        self.failure_error = Some(backend_error("WASAPI capture is no longer live"));
        self.stalled_error = Some(backend_error(
            "WASAPI capture stalled: no samples before deadline",
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
            decode_device_id("{render-guid}").unwrap(),
            (TrackKind::System, "{render-guid}")
        );
        assert!(decode_device_id("").is_err());
        assert!(decode_device_id("mic:").is_err());
        assert_eq!(encode_device_id(&TrackKind::Mic, "42"), "mic:42");
        assert_eq!(encode_device_id(&TrackKind::System, "42"), "42");
    }

    #[test]
    fn loopback_with_exclusive_mode_is_a_typed_backend_error() {
        assert!(matches!(
            map_error(WasapiError::LoopbackWithExclusiveMode),
            CaptureError::Backend(_)
        ));
        assert!(is_device_loss(&CaptureError::DeviceLost("gone".to_owned())));
        assert!(!is_device_loss(&backend_error("other")));
    }
}
