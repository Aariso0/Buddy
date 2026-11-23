//! Linux PipeWire backend.
//!
//! PipeWire objects are `!Send`, so they live on one worker thread that owns
//! an event loop and a capture stream. The worker copies each graph buffer
//! into a preallocated single-producer/single-consumer ring; [`read`](AudioSource::read)
//! only drains that ring with atomics and never touches PipeWire itself.

use crate::ring::RingBuffer;
use crate::{AudioSource, CaptureError, DeviceInfo, TrackKind};
use pipewire as pw;
use pw::properties::properties;
use pw::spa::param::audio::{AudioFormat, AudioInfoRaw};
use pw::spa::param::format_utils;
use pw::spa::param::ParamType;
use pw::spa::pod::serialize::PodSerializer;
use pw::spa::pod::Pod;
use pw::spa::utils::Direction;
use std::io::Cursor;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU8, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender, TryRecvError};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

const DEFAULT_ID: &str = "default";
const MIC_PREFIX: &str = "mic:";
const SAMPLE_RATE: u32 = 48_000;
const CHANNELS: u32 = 2;
// About 170 ms of 48 kHz stereo, matching the engine's default ring: enough
// to absorb a scheduling hiccup without an unbounded allocation. Allocated
// once when the source opens.
const RING_FRAMES: usize = 8192;
const SAMPLE_BATCH: usize = 512;
const LOOP_TICK: Duration = Duration::from_millis(20);
const COMMAND_TIMEOUT: Duration = Duration::from_secs(3);
// A live PipeWire graph delivers buffers every few milliseconds. Waiting for
// this long lets the producer catch up before read reports an empty source.
const DATA_WAIT: Duration = Duration::from_secs(2);

const LIVE: u8 = 0;
const DEVICE_LOST: u8 = 1;
const CLOSED: u8 = 2;
const FAILED: u8 = 3;

enum Command {
    Pause(Sender<Result<(), CaptureError>>),
    Resume(Sender<Result<(), CaptureError>>),
    Shutdown(Sender<Result<(), CaptureError>>),
}

#[derive(Clone)]
struct StreamSpec {
    kind: TrackKind,
    // Checked against the registry at open; the worker reuses it so an
    // explicit id can never silently turn into an un-targeted stream.
    target: Option<u32>,
    ring: Arc<RingBuffer>,
    sample_rate: Arc<AtomicU32>,
    state: Arc<AtomicU8>,
}

struct StreamData {
    ring: Arc<RingBuffer>,
    sample_rate: Arc<AtomicU32>,
    state: Arc<AtomicU8>,
    format: AudioInfoRaw,
    streaming: bool,
}

/// PipeWire reports a removed node either as an outright stream error or as a
/// transition back to unconnected after the graph was running. The initial
/// unconnected state before the first link is not a device loss.
fn is_device_loss(new: &pw::stream::StreamState, was_streaming: bool) -> bool {
    matches!(new, pw::stream::StreamState::Error(_))
        || (was_streaming && matches!(new, pw::stream::StreamState::Unconnected))
}

fn backend_error(message: impl Into<String>) -> CaptureError {
    CaptureError::Backend(message.into())
}

/// The typed error for a live source whose ring stayed empty for the whole
/// wait. Prebuilt by `open` so `read` never allocates, even on this path.
fn stalled_error() -> CaptureError {
    CaptureError::Backend("PipeWire capture stalled: no samples before deadline".to_owned())
}

/// Query the live PipeWire registry for this kind's sink/source nodes. The
/// `default` pseudo-device is added by `list_devices`, not here. `open` uses
/// this same query to reject ids that do not name a real node.
fn enumerate_nodes(kind: &TrackKind) -> Result<Vec<DeviceInfo>, CaptureError> {
    pw::init();
    let mainloop = pw::main_loop::MainLoopRc::new(None)
        .map_err(|error| backend_error(format!("PipeWire main loop: {error}")))?;
    let context = pw::context::ContextRc::new(&mainloop, None)
        .map_err(|error| backend_error(format!("PipeWire context: {error}")))?;
    let core = context
        .connect_rc(None)
        .map_err(|error| backend_error(format!("PipeWire core: {error}")))?;
    let registry = core
        .get_registry()
        .map_err(|error| backend_error(format!("PipeWire registry: {error}")))?;

    let found = Arc::new(Mutex::new(Vec::new()));
    let listener_kind = kind.clone();
    let done = Arc::new(AtomicBool::new(false));
    let done_listener = {
        let done = Arc::clone(&done);
        core.add_listener_local()
            .done(move |_, _| done.store(true, Ordering::Release))
            .register()
    };
    let registry_listener = {
        let found = Arc::clone(&found);
        registry
            .add_listener_local()
            .global(move |global| {
                if global.type_ != pw::types::ObjectType::Node {
                    return;
                }
                let Some(props) = global.props else { return };
                let class = props.get(*pw::keys::MEDIA_CLASS).unwrap_or("");
                // Sinks own the monitor ports used for system capture;
                // application playback streams (Stream/Output/Audio) do not.
                let wanted = match &listener_kind {
                    TrackKind::System => class.starts_with("Audio/Sink"),
                    TrackKind::Mic => class.starts_with("Audio/Source"),
                };
                if !wanted {
                    return;
                }
                let name = props
                    .get(*pw::keys::NODE_DESCRIPTION)
                    .or_else(|| props.get(*pw::keys::NODE_NAME))
                    .unwrap_or("PipeWire audio node");
                if let Ok(mut devices) = found.lock() {
                    devices.push(DeviceInfo {
                        id: encode_device_id(&listener_kind, &global.id.to_string()),
                        name: name.to_owned(),
                        kind: listener_kind.clone(),
                    });
                }
            })
            .register()
    };

    core.sync(0)
        .map_err(|error| backend_error(format!("PipeWire registry sync: {error}")))?;
    for _ in 0..200 {
        if done.load(Ordering::Acquire) {
            break;
        }
        mainloop
            .loop_()
            .iterate(pw::loop_::Timeout::Finite(Duration::from_millis(10)));
    }
    drop(registry_listener);
    drop(done_listener);

    // A sync that never completes leaves the registry only partly enumerated.
    // Returning those nodes would let `validated_target` reject a live device
    // as unknown, so report the timeout as a typed backend error instead.
    if !done.load(Ordering::Acquire) {
        return Err(backend_error(
            "PipeWire registry sync did not complete within the iteration budget",
        ));
    }

    let mut nodes = Vec::new();
    if let Ok(extra) = found.lock() {
        nodes.extend(extra.iter().cloned());
    }
    Ok(nodes)
}

/// Parse an explicit device id and verify it against the same registry that
/// `list_devices` reports. `default` is a pseudo-device and needs no entry;
/// any other id must name a live sink/source node of `kind`, otherwise this
/// is a typed error rather than a stream that never receives samples.
fn validated_target(kind: &TrackKind, device_id: &str) -> Result<Option<u32>, CaptureError> {
    if device_id == DEFAULT_ID {
        return Ok(None);
    }
    let id = device_id
        .parse::<u32>()
        .map_err(|_| backend_error(format!("unknown PipeWire device id: {device_id}")))?;
    let encoded = encode_device_id(kind, device_id);
    if enumerate_nodes(kind)?.iter().any(|node| node.id == encoded) {
        Ok(Some(id))
    } else {
        Err(backend_error(format!(
            "unknown PipeWire device id: {device_id}"
        )))
    }
}

/// The `mic:` prefix is how device ids from `list_devices(Mic)` encode the
/// track: `open` has no separate kind argument. Bare ids, including
/// `default`, select the system sink monitor.
fn decode_device_id(device_id: &str) -> Result<(TrackKind, &str), CaptureError> {
    if device_id.is_empty() {
        return Err(backend_error("empty PipeWire device id"));
    }
    match device_id.strip_prefix(MIC_PREFIX) {
        Some(rest) if !rest.is_empty() => Ok((TrackKind::Mic, rest)),
        Some(_) => Err(backend_error("empty PipeWire microphone device id")),
        None => Ok((TrackKind::System, device_id)),
    }
}

fn encode_device_id(kind: &TrackKind, id: &str) -> String {
    match kind {
        TrackKind::System => id.to_owned(),
        TrackKind::Mic => format!("{MIC_PREFIX}{id}"),
    }
}

fn format_params() -> Result<Vec<u8>, CaptureError> {
    let mut info = AudioInfoRaw::new();
    info.set_format(AudioFormat::F32LE);
    info.set_rate(SAMPLE_RATE);
    info.set_channels(CHANNELS);
    let object = pw::spa::pod::Object {
        type_: pw::spa::utils::SpaTypes::ObjectParamFormat.as_raw(),
        id: ParamType::EnumFormat.as_raw(),
        properties: info.into(),
    };
    let value = pw::spa::pod::Value::Object(object);
    PodSerializer::serialize(Cursor::new(Vec::new()), &value)
        .map(|(cursor, _)| cursor.into_inner())
        .map_err(|error| backend_error(format!("PipeWire audio format: {error}")))
}

/// Copy one graph buffer into the ring in fixed stack-sized batches: no
/// allocation, even in the PipeWire process callback. Dropping samples when
/// the ring is full is an overrun, preferable to blocking the graph thread.
fn copy_buffer(
    array: &mut [f32; SAMPLE_BATCH],
    buffer: &mut pw::buffer::Buffer<'_>,
    data: &StreamData,
) {
    let Some(datas) = buffer.datas_mut().first_mut() else {
        return;
    };
    let size = datas.chunk().size() as usize;
    let Some(bytes) = datas.data().and_then(|bytes| bytes.get(..size)) else {
        return;
    };
    let mut filled = 0;
    for chunk in bytes.chunks_exact(4) {
        array[filled] = f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
        filled += 1;
        if filled == array.len() {
            let _ = data.ring.push(&array[..]);
            filled = 0;
        }
    }
    if filled > 0 {
        let _ = data.ring.push(&array[..filled]);
    }
}

fn connect_and_run(
    spec: StreamSpec,
    commands: Receiver<Command>,
    ready: Sender<Result<(), CaptureError>>,
) {
    if let Err(error) = build_and_run(spec, commands, &ready) {
        let _ = ready.send(Err(error));
    }
}

fn build_and_run(
    spec: StreamSpec,
    commands: Receiver<Command>,
    ready: &Sender<Result<(), CaptureError>>,
) -> Result<(), CaptureError> {
    pw::init();
    let mainloop = pw::main_loop::MainLoopRc::new(None)
        .map_err(|error| backend_error(format!("PipeWire main loop: {error}")))?;
    let context = pw::context::ContextRc::new(&mainloop, None)
        .map_err(|error| backend_error(format!("PipeWire context: {error}")))?;
    let core = context
        .connect_rc(None)
        .map_err(|error| backend_error(format!("PipeWire core: {error}")))?;

    // Already parsed and registry-checked at open; never re-guess it here.
    let target = spec.target;
    let mut props = properties! {
        *pw::keys::MEDIA_TYPE => "Audio",
        *pw::keys::MEDIA_CATEGORY => "Capture",
        *pw::keys::MEDIA_ROLE => "Music",
        *pw::keys::NODE_NAME => "buddy-capture",
    };
    if spec.kind == TrackKind::System {
        // Upstream's sink-monitor mode: autoconnect attaches an Input stream
        // to the default sink's monitor ports, never to a PulseAudio shim.
        props.insert(*pw::keys::STREAM_CAPTURE_SINK, "true");
    }
    if let Some(id) = target {
        // The key constant is feature-gated upstream; the wire property name
        // itself works on every supported PipeWire release.
        props.insert("target.object", id.to_string());
    }

    let stream = pw::stream::StreamBox::new(&core, "buddy-capture", props)
        .map_err(|error| backend_error(format!("PipeWire stream: {error}")))?;

    let encoded = format_params()?;
    let pod = Pod::from_bytes(&encoded)
        .ok_or_else(|| backend_error("PipeWire audio format pod is invalid"))?;
    let mut params = [pod];

    let data = StreamData {
        ring: Arc::clone(&spec.ring),
        sample_rate: Arc::clone(&spec.sample_rate),
        state: Arc::clone(&spec.state),
        format: AudioInfoRaw::new(),
        streaming: false,
    };
    let listener = stream
        .add_local_listener_with_user_data(data)
        .param_changed(|_, data, id, param| {
            let Some(param) = param else { return };
            let is_raw_audio = format_utils::parse_format(param)
                .map(|(media, subtype)| {
                    media == pw::spa::param::format::MediaType::Audio
                        && subtype == pw::spa::param::format::MediaSubtype::Raw
                })
                .unwrap_or(false);
            if id != ParamType::Format.as_raw() || !is_raw_audio {
                return;
            }
            if data.format.parse(param).is_ok() {
                if data.format.rate() > 0 {
                    data.sample_rate
                        .store(data.format.rate(), Ordering::Release);
                }
                data.streaming = true;
            }
        })
        .state_changed(|_, data, _, new| {
            if matches!(new, pw::stream::StreamState::Streaming) {
                data.streaming = true;
            } else if is_device_loss(&new, data.streaming) {
                data.state.store(DEVICE_LOST, Ordering::Release);
            }
        })
        .process(|stream, data| {
            let Some(mut buffer) = stream.dequeue_buffer() else {
                return;
            };
            let mut array = [0.0f32; SAMPLE_BATCH];
            copy_buffer(&mut array, &mut buffer, data);
        })
        .register();

    stream
        .connect(
            Direction::Input,
            target,
            pw::stream::StreamFlags::AUTOCONNECT | pw::stream::StreamFlags::MAP_BUFFERS,
            &mut params,
        )
        .map_err(|error| backend_error(format!("PipeWire stream connect: {error}")))?;

    // Announce a connected stream before entering the dispatch loop: `open`
    // waits for this and must not wait for the loop to stop.
    let _ = ready.send(Ok(()));

    loop {
        match commands.try_recv() {
            Ok(Command::Pause(reply)) => {
                let result = stream
                    .set_active(false)
                    .map_err(|error| backend_error(format!("PipeWire pause: {error}")));
                let _ = reply.send(result);
            }
            Ok(Command::Resume(reply)) => {
                let result = stream
                    .set_active(true)
                    .map_err(|error| backend_error(format!("PipeWire resume: {error}")));
                let _ = reply.send(result);
            }
            Ok(Command::Shutdown(reply)) => {
                let _ = reply.send(Ok(()));
                break;
            }
            Err(TryRecvError::Empty) => {}
            Err(TryRecvError::Disconnected) => break,
        }
        mainloop
            .loop_()
            .iterate(pw::loop_::Timeout::Finite(LOOP_TICK));
    }
    drop(listener);
    drop(stream);
    drop(core);
    drop(context);
    drop(mainloop);
    Ok(())
}

/// One track's live PipeWire capture. `open` remembers the exact device id so
/// `reconnect` can rebuild the stream against the same device after a loss.
pub struct PipeWireSource {
    kind: TrackKind,
    device_id: String,
    ring: Arc<RingBuffer>,
    sample_rate: Arc<AtomicU32>,
    state: Arc<AtomicU8>,
    commands: Option<Sender<Command>>,
    worker: Option<JoinHandle<()>>,
    // Prebuilt so `read` can hand out a typed error without allocating.
    pending_error: Option<CaptureError>,
    stalled_error: Option<CaptureError>,
    // Production uses DATA_WAIT; the backend's own unit test shortens it to
    // prove the deadline path without waiting two seconds.
    data_wait: Duration,
}

impl PipeWireSource {
    /// Open a known track directly. The trait's `open` infers the track from
    /// the device id; callers that already know the kind can use this instead.
    pub fn open_for(kind: TrackKind, device_id: &str) -> Result<Self, CaptureError> {
        if device_id.is_empty() {
            return Err(backend_error("empty PipeWire device id"));
        }
        // Reject a nonexistent explicit id before a stream is created: a
        // successful open must never be an empty success for a dead target.
        let target = validated_target(&kind, device_id)?;
        let ring = Arc::new(RingBuffer::new(RING_FRAMES, CHANNELS as usize));
        let sample_rate = Arc::new(AtomicU32::new(SAMPLE_RATE));
        let state = Arc::new(AtomicU8::new(LIVE));
        let spec = StreamSpec {
            kind: kind.clone(),
            target,
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
            pending_error: Some(CaptureError::DeviceLost(
                "PipeWire device was removed".to_owned(),
            )),
            stalled_error: Some(stalled_error()),
            data_wait: DATA_WAIT,
        })
    }

    fn command(
        &self,
        make: impl FnOnce(Sender<Result<(), CaptureError>>) -> Command,
    ) -> Result<(), CaptureError> {
        let commands = self
            .commands
            .as_ref()
            .ok_or_else(|| backend_error("PipeWire source is closed"))?;
        let (reply_tx, reply_rx) = mpsc::channel();
        commands
            .send(make(reply_tx))
            .map_err(|_| backend_error("PipeWire worker is gone"))?;
        match reply_rx.recv_timeout(COMMAND_TIMEOUT) {
            Ok(result) => result,
            Err(RecvTimeoutError::Timeout) => Err(backend_error("PipeWire command timed out")),
            Err(RecvTimeoutError::Disconnected) => Err(backend_error("PipeWire worker stopped")),
        }
    }

    fn shutdown_worker(&mut self) -> Result<(), CaptureError> {
        let result = if self.worker.is_some() {
            self.command(Command::Shutdown)
        } else {
            Ok(())
        };
        if let Some(worker) = self.worker.take() {
            worker
                .join()
                .map_err(|_| backend_error("PipeWire worker panicked"))?;
        }
        self.commands = None;
        result
    }

    fn failure(&mut self, fallback: &str) -> CaptureError {
        self.pending_error
            .take()
            .unwrap_or_else(|| backend_error(fallback))
    }
}

fn spawn_worker(spec: StreamSpec) -> Result<(Sender<Command>, JoinHandle<()>), CaptureError> {
    let (ready_tx, ready_rx) = mpsc::channel();
    let (commands, command_rx) = mpsc::channel();
    let worker = thread::Builder::new()
        .name("buddy-pipewire".to_owned())
        .spawn(move || connect_and_run(spec, command_rx, ready_tx))
        .map_err(|error| backend_error(format!("start PipeWire worker: {error}")))?;

    match ready_rx.recv_timeout(COMMAND_TIMEOUT) {
        Ok(Ok(())) => Ok((commands, worker)),
        Ok(Err(error)) => {
            // The worker has already returned after reporting its failure.
            let _ = worker.join();
            Err(error)
        }
        Err(RecvTimeoutError::Timeout) | Err(RecvTimeoutError::Disconnected) => {
            // Never block the caller joining a worker that may still be
            // waiting on the PipeWire daemon; it will exit on its own.
            Err(backend_error("PipeWire worker did not connect"))
        }
    }
}

impl AudioSource for PipeWireSource {
    fn kind(&self) -> TrackKind {
        self.kind.clone()
    }

    fn sample_rate(&self) -> u32 {
        self.sample_rate.load(Ordering::Acquire)
    }

    fn list_devices(kind: TrackKind) -> Result<Vec<DeviceInfo>, CaptureError> {
        let nodes = enumerate_nodes(&kind)?;
        let mut devices = vec![DeviceInfo {
            id: encode_device_id(&kind, DEFAULT_ID),
            name: match &kind {
                TrackKind::System => "Default system audio (sink monitor)".to_owned(),
                TrackKind::Mic => "Default microphone".to_owned(),
            },
            kind: kind.clone(),
        }];
        devices.extend(nodes);
        Ok(devices)
    }

    fn open(device_id: &str) -> Result<Self, CaptureError> {
        let (kind, id) = decode_device_id(device_id)?;
        Self::open_for(kind, id)
    }

    fn read(&mut self, out: &mut [f32]) -> Result<usize, CaptureError> {
        match self.state.load(Ordering::Acquire) {
            DEVICE_LOST => return Err(self.failure("PipeWire device was removed")),
            CLOSED | FAILED => return Err(self.failure("PipeWire source is closed")),
            _ => {}
        }
        if let Some(count) = self.ring.pop(out) {
            return Ok(count);
        }
        // The engine reads as fast as it can and treats Ok(0) as exhaustion.
        // A short sleep lets the producer catch up without a lock, allocation
        // or a CPU-burning spin. A live source must never report exhaustion: a
        // stalled graph is a typed Backend error so the engine records a gap
        // instead of silently ending the track.
        let deadline = Instant::now() + self.data_wait;
        while Instant::now() < deadline {
            thread::sleep(Duration::from_millis(1));
            if self.state.load(Ordering::Acquire) != LIVE {
                return Err(self.failure("PipeWire capture is no longer live"));
            }
            if let Some(count) = self.ring.pop(out) {
                return Ok(count);
            }
        }
        Err(self.stalled_error.take().unwrap_or_else(stalled_error))
    }

    fn pause(&mut self) -> Result<(), CaptureError> {
        self.command(Command::Pause)
    }

    fn resume(&mut self) -> Result<(), CaptureError> {
        self.command(Command::Resume)
    }

    fn reconnect(&mut self) -> Result<(), CaptureError> {
        // Keep the original device id and all shared handles. Only the
        // PipeWire-side objects (which are !Send) are rebuilt on the worker.
        self.shutdown_worker()?;
        let mut scratch = [0.0f32; SAMPLE_BATCH];
        while self.ring.pop(&mut scratch).is_some() {}
        // Re-check the remembered id against the live registry: a device that
        // was removed must fail reconnect with a typed error, not re-arm a
        // stale stream.
        let target = validated_target(&self.kind, &self.device_id)?;
        let spec = StreamSpec {
            kind: self.kind.clone(),
            target,
            ring: Arc::clone(&self.ring),
            sample_rate: Arc::clone(&self.sample_rate),
            state: Arc::clone(&self.state),
        };
        self.state.store(LIVE, Ordering::Release);
        self.pending_error = Some(CaptureError::DeviceLost(
            "PipeWire device was removed".to_owned(),
        ));
        self.stalled_error = Some(stalled_error());
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
        self.state.store(CLOSED, Ordering::Release);
        self.shutdown_worker()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stream_error_and_post_stream_unconnected_are_device_loss() {
        assert!(is_device_loss(
            &pw::stream::StreamState::Error("node removed".to_owned()),
            true
        ));
        assert!(is_device_loss(&pw::stream::StreamState::Unconnected, true));
        assert!(!is_device_loss(
            &pw::stream::StreamState::Unconnected,
            false
        ));
        assert!(!is_device_loss(&pw::stream::StreamState::Streaming, true));
    }

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
        assert!(decode_device_id("").is_err());
        assert!(decode_device_id("mic:").is_err());
        assert_eq!(encode_device_id(&TrackKind::Mic, "42"), "mic:42");
        assert_eq!(encode_device_id(&TrackKind::System, "42"), "42");
    }

    #[test]
    fn empty_ring_deadline_is_a_typed_error_not_exhaustion() {
        // A live source with an empty ring and a short test-only deadline must
        // fail as Backend, never report Ok(0) as end-of-stream.
        let ring = Arc::new(RingBuffer::new(8, 2));
        let mut source = PipeWireSource {
            kind: TrackKind::System,
            device_id: DEFAULT_ID.to_owned(),
            ring: Arc::clone(&ring),
            sample_rate: Arc::new(AtomicU32::new(SAMPLE_RATE)),
            state: Arc::new(AtomicU8::new(LIVE)),
            commands: None,
            worker: None,
            pending_error: Some(CaptureError::DeviceLost("unused".to_owned())),
            stalled_error: Some(stalled_error()),
            data_wait: Duration::from_millis(20),
        };
        let mut buffer = [0.0f32; 8];
        let error = source
            .read(&mut buffer)
            .expect_err("empty live source must never report Ok(0)");
        assert!(
            matches!(error, CaptureError::Backend(_)),
            "deadline must be typed Backend, got {error}"
        );

        // The same source still reads normally once the producer catches up.
        ring.push(&[1.0, 2.0]).expect("ring has room");
        let count = source
            .read(&mut buffer)
            .expect("samples must read normally after a transient stall");
        assert_eq!(count, 2);
        assert_eq!(&buffer[..2], &[1.0, 2.0]);
    }
}
