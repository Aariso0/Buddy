use buddy_capture::{
    engine::{CaptureConfig, CaptureEngine},
    AudioSource, DeviceInfo, TrackKind,
};
use buddy_core::{
    clock::SessionClock,
    session::{create_session, Preset},
    settings::{ensure_data_dir, Settings},
    support::{SupportFloors, SUPPORT_FLOORS},
};
use chrono::Utc;
use serde::Serialize;
use std::{
    fs,
    sync::{Arc, Mutex},
};
use tauri::{Emitter, Manager};
use tauri_plugin_global_shortcut::{Builder as ShortcutBuilder, ShortcutState};
use uuid::Uuid;

const HOTKEY_START_STOP: &str = "CommandOrControl+Shift+R";
// The display label must match CommandOrControl+Shift+R on every platform.
const HUD_HOTKEY: &str = if cfg!(target_os = "macos") {
    "Command+Shift+R"
} else {
    "Ctrl+Shift+R"
};
const TRAY_ID: &str = "buddy-tray";
const TRAY_TOGGLE_ID: &str = "tray-toggle-recording";
const TRAY_QUIT_ID: &str = "tray-quit";
const SYSTEM_DEVICE_ID: &str = "default";
const MIC_DEVICE_ID: &str = "mic:default";

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "lowercase")]
enum ViewPhase {
    Idle,
    Recording,
    Paused,
}

/// The single serializable snapshot the HUD, window title and tray all render.
#[derive(Debug, Clone, Serialize)]
struct RecordingView {
    phase: ViewPhase,
    session_id: Option<String>,
    title: String,
    hud: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    Recording,
    Paused,
}

struct ActiveRecording {
    id: String,
    preset: Preset,
    engine: CaptureEngine,
    phase: Phase,
}

fn preset_label(preset: &Preset) -> &'static str {
    match preset {
        Preset::Lecture => "lecture",
        Preset::Meeting => "meeting",
    }
}

fn view_for(active: Option<&ActiveRecording>) -> RecordingView {
    let Some(active) = active else {
        return RecordingView {
            phase: ViewPhase::Idle,
            session_id: None,
            title: "Buddy".to_owned(),
            hud: format!("system: idle · mic: idle · hotkey: {HUD_HOTKEY}"),
        };
    };

    let (phase, label, status) = match active.phase {
        Phase::Recording => (ViewPhase::Recording, "REC", "recording"),
        Phase::Paused => (ViewPhase::Paused, "PAUSED", "paused"),
    };
    let seconds = active.engine.elapsed_ms() / 1000;
    RecordingView {
        phase,
        session_id: Some(active.id.clone()),
        title: format!("Buddy — {label}"),
        hud: format!(
            "{label} · system: {status} · mic: {status} · preset: {} · {seconds}s",
            preset_label(&active.preset)
        ),
    }
}

/// Test seam for window-title, tray and event publication.
trait ViewSink: Send + Sync {
    fn publish(&self, view: &RecordingView);
}

struct TauriViewSink {
    app: tauri::AppHandle,
}

impl ViewSink for TauriViewSink {
    fn publish(&self, view: &RecordingView) {
        if let Some(window) = self.app.get_webview_window("main") {
            let _ = window.set_title(&view.title);
        }
        if let Some(tray) = self.app.tray_by_id(TRAY_ID) {
            let _ = tray.set_title(Some(&view.title));
        }
        let _ = self.app.emit("recording-state", view.clone());
    }
}

/// Test seam for platform audio devices. The production implementation uses
/// the already-licensed per-OS backends; tests use the capture fake.
trait SourceFactory: Send + Sync {
    fn list_devices(&self, kind: &TrackKind) -> Result<Vec<DeviceInfo>, String>;
    fn open(&self, kind: TrackKind, device_id: &str) -> Result<Box<dyn AudioSource>, String>;
}

struct PlatformSources;

impl SourceFactory for PlatformSources {
    fn list_devices(&self, kind: &TrackKind) -> Result<Vec<DeviceInfo>, String> {
        #[cfg(target_os = "linux")]
        {
            buddy_capture::linux::PipeWireSource::list_devices(kind.clone())
                .map_err(|error| error.to_string())
        }
        #[cfg(target_os = "macos")]
        {
            buddy_capture::macos::CoreAudioSource::list_devices(kind.clone())
                .map_err(|error| error.to_string())
        }
        #[cfg(target_os = "windows")]
        {
            buddy_capture::windows::WasapiSource::list_devices(kind.clone())
                .map_err(|error| error.to_string())
        }
        #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
        {
            Err(format!(
                "unsupported audio capture platform: {}",
                std::env::consts::OS
            ))
        }
    }

    fn open(&self, kind: TrackKind, device_id: &str) -> Result<Box<dyn AudioSource>, String> {
        // Device ids follow the `list_devices` convention: the `mic:` prefix
        // carries the track. `AudioSource::open` decodes that prefix;
        // `open_for` takes an already-decoded raw id and validates it verbatim.
        #[cfg(target_os = "linux")]
        {
            let source = <buddy_capture::linux::PipeWireSource as AudioSource>::open(device_id)
                .map_err(|error| error.to_string())?;
            debug_assert_eq!(source.kind(), kind);
            Ok(Box::new(source))
        }
        #[cfg(target_os = "macos")]
        {
            let source = <buddy_capture::macos::CoreAudioSource as AudioSource>::open(device_id)
                .map_err(|error| error.to_string())?;
            debug_assert_eq!(source.kind(), kind);
            Ok(Box::new(source))
        }
        #[cfg(target_os = "windows")]
        {
            let source = <buddy_capture::windows::WasapiSource as AudioSource>::open(device_id)
                .map_err(|error| error.to_string())?;
            debug_assert_eq!(source.kind(), kind);
            Ok(Box::new(source))
        }
        #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
        {
            Err(format!(
                "unsupported audio capture platform: {}",
                std::env::consts::OS
            ))
        }
    }
}

#[derive(Default)]
struct Slot {
    // `busy` is a cheap guard so start/stop blocking work never runs while the
    // mutex is held. The engine itself remains owned by this one slot.
    active: Option<ActiveRecording>,
    busy: bool,
}

struct RecordingController {
    settings: Settings,
    factory: Box<dyn SourceFactory>,
    sink: Arc<dyn ViewSink>,
    slot: Mutex<Slot>,
    view: Mutex<RecordingView>,
}

impl RecordingController {
    fn new(settings: Settings, factory: Box<dyn SourceFactory>, sink: Arc<dyn ViewSink>) -> Self {
        let controller = Self {
            settings,
            factory,
            sink,
            slot: Mutex::new(Slot::default()),
            view: Mutex::new(view_for(None)),
        };
        let view = controller.view();
        controller.publish(view);
        controller
    }

    fn lock_slot(&self) -> std::sync::MutexGuard<'_, Slot> {
        self.slot
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn view(&self) -> RecordingView {
        self.view
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    fn lock_view(&self) -> std::sync::MutexGuard<'_, RecordingView> {
        self.view
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Store and emit under one guard: two concurrent publications must not
    /// reach the frontend in a different order from the stored snapshot.
    fn publish(&self, view: RecordingView) {
        let mut stored = self.lock_view();
        *stored = view.clone();
        self.sink.publish(&view);
    }

    fn list_devices(&self, kind: &TrackKind) -> Result<Vec<DeviceInfo>, String> {
        self.factory.list_devices(kind)
    }

    /// Check for an active recording and reserve the busy flag under a single
    /// lock. Two concurrent starts can no longer both pass the active check
    /// before either claims the slot.
    fn reserve_start(&self) -> Result<(), String> {
        let mut slot = self.lock_slot();
        if slot.active.is_some() {
            return Err("a recording is already running; stop it first".to_owned());
        }
        if slot.busy {
            return Err("another recording operation is in progress".to_owned());
        }
        slot.busy = true;
        Ok(())
    }

    fn publish_error(&self, cause: &str) {
        // Read, amend and emit under the same view guard as publish(), so a
        // concurrent state change cannot be overwritten by a stale error view.
        // Only the HUD changes: phase and session id stay as they are.
        let mut view = self.lock_view();
        view.hud = format!("{} · action failed: {cause}", view.hud);
        self.sink.publish(&view);
    }

    /// Tray and global-hotkey callbacks use this so a failed toggle is visible
    /// in the HUD instead of being silently discarded.
    fn toggle_from_ui(&self) {
        if let Err(error) = self.toggle() {
            self.publish_error(&error);
        }
    }

    fn start_inner(&self, preset: Preset) -> Result<(ActiveRecording, RecordingView), String> {
        // First-run bootstrap: the session write below must never race the
        // directory creation, so this happens before create_session.
        let sessions_root = ensure_data_dir(&self.settings).map_err(|error| error.to_string())?;
        let id = Uuid::new_v4().to_string();
        let mut session_created = false;

        let outcome = (|| -> Result<(ActiveRecording, RecordingView), String> {
            create_session(&sessions_root, &id, Utc::now(), preset.clone())
                .map_err(|error| error.to_string())?;
            session_created = true;

            let system = self.factory.open(TrackKind::System, SYSTEM_DEVICE_ID)?;
            let mic = self.factory.open(TrackKind::Mic, MIC_DEVICE_ID)?;
            let mut engine = CaptureEngine::new(
                &id,
                &sessions_root.join(&id),
                SessionClock::new(),
                system,
                mic,
                CaptureConfig::default(),
            )
            .map_err(|error| error.to_string())?;
            engine.start().map_err(|error| error.to_string())?;

            let active = ActiveRecording {
                id: id.clone(),
                preset,
                engine,
                phase: Phase::Recording,
            };
            let view = view_for(Some(&active));
            Ok((active, view))
        })();

        if outcome.is_err() && session_created {
            // A failed open/start must not leave a session stuck in recording
            // state. The typed error is still returned to the caller.
            let _ = fs::remove_dir_all(sessions_root.join(&id));
        }
        outcome
    }

    fn start(&self, preset: Preset) -> Result<String, String> {
        self.reserve_start()?;

        let outcome = self.start_inner(preset);
        let mut slot = self.lock_slot();
        slot.busy = false;
        match outcome {
            Ok((active, view)) => {
                let id = active.id.clone();
                slot.active = Some(active);
                drop(slot);
                self.publish(view);
                Ok(id)
            }
            Err(error) => Err(error),
        }
    }

    fn pause(&self, id: &str) -> Result<(), String> {
        let view = {
            let mut slot = self.lock_slot();
            if slot.busy {
                return Err("another recording operation is in progress".to_owned());
            }
            let active = slot
                .active
                .as_mut()
                .ok_or_else(|| "no active recording to pause".to_owned())?;
            if active.id != id {
                return Err(format!("unknown recording id: {id}"));
            }
            active.engine.pause().map_err(|error| error.to_string())?;
            active.phase = Phase::Paused;
            view_for(Some(active))
        };
        self.publish(view);
        Ok(())
    }

    fn resume(&self, id: &str) -> Result<(), String> {
        let view = {
            let mut slot = self.lock_slot();
            if slot.busy {
                return Err("another recording operation is in progress".to_owned());
            }
            let active = slot
                .active
                .as_mut()
                .ok_or_else(|| "no active recording to resume".to_owned())?;
            if active.id != id {
                return Err(format!("unknown recording id: {id}"));
            }
            active.engine.resume().map_err(|error| error.to_string())?;
            active.phase = Phase::Recording;
            view_for(Some(active))
        };
        self.publish(view);
        Ok(())
    }

    fn stop(&self, id: &str) -> Result<(), String> {
        let mut active = {
            let mut slot = self.lock_slot();
            if slot.busy {
                return Err("another recording operation is in progress".to_owned());
            }
            if slot.active.as_ref().map(|active| active.id.as_str()) != Some(id) {
                return Err(format!("unknown recording id: {id}"));
            }
            slot.busy = true;
            match slot.active.take() {
                Some(active) => active,
                None => {
                    slot.busy = false;
                    return Err(format!("unknown recording id: {id}"));
                }
            }
        };

        // Joining the readers and writer is blocking; the mutex is released.
        let result = active.engine.stop().map_err(|error| error.to_string());
        // `CaptureEngine::stop` clears its `started` flag before the fallible
        // join/finalize work, so an engine that fails to stop refuses every
        // later pause/resume/stop/start. Restoring it would wedge the app in a
        // permanent REC state. The active value is dropped below on both paths.
        let next_view = match &result {
            Ok(()) => view_for(None),
            Err(error) => {
                let mut view = view_for(None);
                view.hud = format!("{} · stop failed: {error}", view.hud);
                view
            }
        };

        {
            let mut slot = self.lock_slot();
            slot.busy = false;
            // The slot stays empty: a successful stop ends the recording and a
            // failed stop leaves an engine that must not be reused. A new start
            // can reclaim the slot immediately; the error text stays in the HUD.
            debug_assert!(slot.active.is_none());
        }
        self.publish(next_view);
        result
    }

    fn toggle(&self) -> Result<(), String> {
        let current = {
            let slot = self.lock_slot();
            slot.active
                .as_ref()
                .map(|active| (active.id.clone(), active.phase))
        };
        match current {
            Some((id, _)) => self.stop(&id),
            None => self.start(Preset::Lecture).map(|_| ()),
        }
    }
}

struct AppState {
    controller: RecordingController,
    // Holding the tray keeps it alive for the whole process.
    _tray: tauri::tray::TrayIcon,
}

fn toggle_from_ui(app: &tauri::AppHandle) {
    if let Some(state) = app.try_state::<AppState>() {
        state.controller.toggle_from_ui();
    }
}

#[tauri::command]
fn support_matrix() -> SupportFloors {
    SUPPORT_FLOORS.clone()
}

#[tauri::command]
fn recording_state(state: tauri::State<'_, AppState>) -> RecordingView {
    state.controller.view()
}

#[tauri::command]
fn list_audio_devices(
    kind: TrackKind,
    state: tauri::State<'_, AppState>,
) -> Result<Vec<DeviceInfo>, String> {
    state.controller.list_devices(&kind)
}

#[tauri::command]
fn start_recording(preset: Preset, state: tauri::State<'_, AppState>) -> Result<String, String> {
    state.controller.start(preset)
}

#[tauri::command]
fn pause_recording(id: String, state: tauri::State<'_, AppState>) -> Result<(), String> {
    state.controller.pause(&id)
}

#[tauri::command]
fn resume_recording(id: String, state: tauri::State<'_, AppState>) -> Result<(), String> {
    state.controller.resume(&id)
}

#[tauri::command]
fn stop_recording(id: String, state: tauri::State<'_, AppState>) -> Result<(), String> {
    state.controller.stop(&id)
}

// `generate_context!` embeds Info.plist on macOS. Keep it at one expansion
// site: a second call in the test would define `_EMBED_INFO_PLIST` twice and
// fail the macOS build.
fn app_context() -> tauri::Context<tauri::Wry> {
    tauri::generate_context!()
}

pub fn run() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![
            support_matrix,
            recording_state,
            list_audio_devices,
            start_recording,
            pause_recording,
            resume_recording,
            stop_recording,
        ])
        .setup(|app| {
            use tauri::menu::{Menu, MenuItem};
            use tauri::tray::TrayIconBuilder;

            let start_stop = MenuItem::with_id(
                app,
                TRAY_TOGGLE_ID,
                "Start / Stop recording",
                true,
                None::<&str>,
            )?;
            let quit = MenuItem::with_id(app, TRAY_QUIT_ID, "Quit Buddy", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&start_stop, &quit])?;
            let icon = app.default_window_icon().cloned().ok_or_else(|| {
                std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    "the configured default window icon is required for the tray",
                )
            })?;
            let tray = TrayIconBuilder::with_id(TRAY_ID)
                .icon(icon)
                .menu(&menu)
                .title("Buddy")
                .tooltip("Buddy")
                .on_menu_event(|app, event| {
                    let id = event.id().as_ref();
                    if id == TRAY_TOGGLE_ID {
                        toggle_from_ui(app);
                    } else if id == TRAY_QUIT_ID {
                        app.exit(0);
                    }
                })
                .build(app)?;

            let sink = Arc::new(TauriViewSink {
                app: app.handle().clone(),
            });
            let controller =
                RecordingController::new(Settings::default(), Box::new(PlatformSources), sink);
            app.manage(AppState {
                controller,
                _tray: tray,
            });

            // A hotkey that cannot be registered must not take down the whole
            // app: tray, window and HUD already work without it.
            match ShortcutBuilder::new().with_shortcut(HOTKEY_START_STOP) {
                Ok(builder) => {
                    if let Err(error) = app.handle().plugin(
                        builder
                            .with_handler(|app, _shortcut, event| {
                                if event.state == ShortcutState::Pressed {
                                    toggle_from_ui(app);
                                }
                            })
                            .build(),
                    ) {
                        eprintln!("global shortcut registration failed: {error}");
                    }
                }
                Err(error) => eprintln!("global shortcut is invalid: {error}"),
            }
            Ok(())
        })
        .run(app_context())
        .expect("error while running Buddy");
}

#[cfg(test)]
mod tests {
    use super::*;
    use buddy_capture::{AudioSource, DeviceInfo, TrackKind};
    use std::path::Path;
    use std::sync::{Arc, Mutex};

    struct FakeSourceFactory;

    impl SourceFactory for FakeSourceFactory {
        fn list_devices(&self, kind: &TrackKind) -> Result<Vec<DeviceInfo>, String> {
            buddy_capture::fake::FakeAudioSource::list_devices(kind.clone())
                .map_err(|error| error.to_string())
        }

        fn open(&self, kind: TrackKind, _device_id: &str) -> Result<Box<dyn AudioSource>, String> {
            Ok(Box::new(
                buddy_capture::fake::FakeAudioSource::from_samples(kind, 48_000, vec![0.0; 4_096]),
            ))
        }
    }

    struct FailingSourceFactory;

    impl SourceFactory for FailingSourceFactory {
        fn list_devices(&self, _kind: &TrackKind) -> Result<Vec<DeviceInfo>, String> {
            Err("device enumeration unavailable".to_owned())
        }

        fn open(&self, _kind: TrackKind, _device_id: &str) -> Result<Box<dyn AudioSource>, String> {
            Err("audio device busy: fake failure".to_owned())
        }
    }

    struct MemoryViewSink {
        title: Mutex<String>,
        hud: Mutex<String>,
    }

    impl MemoryViewSink {
        fn new() -> Self {
            Self {
                title: Mutex::new(String::new()),
                hud: Mutex::new(String::new()),
            }
        }
    }

    impl ViewSink for MemoryViewSink {
        fn publish(&self, view: &RecordingView) {
            *self.title.lock().unwrap_or_else(|e| e.into_inner()) = view.title.clone();
            *self.hud.lock().unwrap_or_else(|e| e.into_inner()) = view.hud.clone();
        }
    }

    struct TestApp {
        controller: RecordingController,
        sink: Arc<MemoryViewSink>,
    }

    impl TestApp {
        fn new(data_dir: &Path, factory: Box<dyn SourceFactory>) -> Self {
            let sink = Arc::new(MemoryViewSink::new());
            let settings = Settings {
                data_dir: data_dir.to_path_buf(),
                ..Settings::default()
            };
            let controller = RecordingController::new(settings, factory, sink.clone());
            Self { controller, sink }
        }

        fn call(&self, command: &str, arg: &str) -> Result<String, String> {
            match command {
                "list_audio_devices" => {
                    let kind = match arg {
                        "system" => TrackKind::System,
                        "mic" => TrackKind::Mic,
                        other => return Err(format!("unknown track kind: {other}")),
                    };
                    let devices = self.controller.list_devices(&kind)?;
                    serde_json::to_string(&devices).map_err(|error| error.to_string())
                }
                "start_recording" => self.controller.start(preset_from(arg)),
                "pause_recording" => self.controller.pause(arg).map(|_| String::new()),
                "resume_recording" => self.controller.resume(arg).map(|_| String::new()),
                "stop_recording" => self.controller.stop(arg).map(|_| String::new()),
                other => Err(format!("unknown command: {other}")),
            }
        }

        fn window_title(&self) -> String {
            self.sink
                .title
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .clone()
        }

        fn hud_text(&self) -> String {
            self.sink
                .hud
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .clone()
        }

        fn trigger_hotkey(&self, name: &str) {
            assert_eq!(name, "start_stop");
            self.controller
                .toggle()
                .expect("the start/stop hotkey must toggle recording");
        }
    }

    fn preset_from(value: &str) -> Preset {
        match value {
            "lecture" => Preset::Lecture,
            "meeting" => Preset::Meeting,
            other => panic!("unknown preset: {other}"),
        }
    }

    #[test]
    fn recording_state_is_visible_and_hotkey_toggles_it() {
        let dir = tempfile::tempdir().expect("a temporary data directory");
        let app = TestApp::new(dir.path(), Box::new(FakeSourceFactory));
        app.call("start_recording", "lecture").unwrap();
        assert!(app.window_title().contains("REC"));
        assert!(app.hud_text().contains("system"));
        assert!(app.hud_text().contains("mic"));
        app.trigger_hotkey("start_stop");
        assert!(!app.window_title().contains("REC"));
    }

    #[test]
    fn commands_surface_typed_errors_as_non_zero_results() {
        let dir = tempfile::tempdir().expect("a temporary data directory");
        let app = TestApp::new(dir.path(), Box::new(FailingSourceFactory));
        let error = app
            .call("start_recording", "lecture")
            .expect_err("a busy device must fail the command");
        assert!(
            error.contains("audio device busy: fake failure"),
            "unexpected error: {error}"
        );
        assert!(!app.window_title().contains("REC"));
    }

    #[test]
    fn idle_hud_matches_the_configured_hotkey_label() {
        let expected = if cfg!(target_os = "macos") {
            "Command+Shift+R"
        } else {
            "Ctrl+Shift+R"
        };
        assert_eq!(HOTKEY_START_STOP, "CommandOrControl+Shift+R");
        let hud = view_for(None).hud;
        assert!(
            hud.contains(expected),
            "idle HUD must show the platform hotkey: {hud}"
        );
    }

    #[test]
    fn ui_toggle_failure_is_published_to_the_hud() {
        let dir = tempfile::tempdir().expect("a temporary data directory");
        let app = TestApp::new(dir.path(), Box::new(FailingSourceFactory));
        app.controller.toggle_from_ui();
        let hud = app.hud_text();
        assert!(
            hud.contains("action failed") && hud.contains("audio device busy: fake failure"),
            "a failed UI toggle must be visible in the HUD: {hud}"
        );
    }

    #[test]
    fn error_publication_preserves_the_active_recording() {
        let dir = tempfile::tempdir().expect("a temporary data directory");
        let app = TestApp::new(dir.path(), Box::new(FakeSourceFactory));
        let id = app.call("start_recording", "lecture").unwrap();

        app.controller.publish_error("probe failure");

        let view = app.controller.view();
        assert_eq!(view.session_id.as_deref(), Some(id.as_str()));
        assert!(matches!(view.phase, ViewPhase::Recording));
        assert!(view.hud.contains("probe failure"));
        assert!(app.hud_text().contains("probe failure"));
    }

    #[test]
    fn stop_error_discards_the_dead_engine_and_allows_a_new_start() {
        let dir = tempfile::tempdir().expect("a temporary data directory");
        let app = TestApp::new(dir.path(), Box::new(FakeSourceFactory));
        let id = app.call("start_recording", "lecture").unwrap();
        assert!(app.window_title().contains("REC"));

        // Remove the reserved session so finalize_session returns a typed error
        // from engine.stop() after the engine has already marked itself stopped.
        let session_dir = dir.path().join("sessions").join(&id);
        fs::remove_dir_all(&session_dir).expect("the session directory must exist");

        let error = app
            .call("stop_recording", &id)
            .expect_err("a missing manifest must fail the stop command");
        assert!(
            error.contains("finalize session"),
            "unexpected error: {error}"
        );
        assert!(!app.window_title().contains("REC"));
        assert!(app.hud_text().contains("stop failed"));

        // The wedged state is gone: a new recording can start and show REC.
        let next_id = app.call("start_recording", "meeting").unwrap();
        assert_ne!(next_id, id);
        assert!(app.window_title().contains("REC"));
    }

    #[test]
    fn start_recording_persists_the_requested_preset() {
        let dir = tempfile::tempdir().expect("a temporary data directory");
        let app = TestApp::new(dir.path(), Box::new(FakeSourceFactory));
        let id = app.call("start_recording", "meeting").unwrap();
        let manifest = buddy_core::session::read_session(&dir.path().join("sessions"), &id)
            .expect("the session manifest must be readable");
        assert_eq!(manifest.preset, Preset::Meeting);
        assert_eq!(manifest.state, buddy_core::session::SessionState::Recording);
    }

    #[test]
    fn listing_devices_reports_both_tracks() {
        let dir = tempfile::tempdir().expect("a temporary data directory");
        let app = TestApp::new(dir.path(), Box::new(FakeSourceFactory));
        let system = app.call("list_audio_devices", "system").unwrap();
        let mic = app.call("list_audio_devices", "mic").unwrap();
        assert!(system.contains("Fake system audio"));
        assert!(mic.contains("Fake microphone"));
    }

    /// `MIC_DEVICE_ID` mirrors the encoded ids returned by `list_devices(Mic)`.
    /// It must reach the backend's decoding `AudioSource::open`, never the
    /// `open_for(kind, raw_id)` entry point, which validates the id verbatim.
    /// The fake factory ignores ids, so only the real factory can pin this.
    #[test]
    fn platform_sources_decodes_the_encoded_mic_id_before_opening() {
        // A live device is not required. A decoded `default` either opens or
        // fails later in the backend's device layer; an encoded id forwarded
        // to `open_for` is rejected by id validation with the literal
        // `mic:default` in the error. That is the pre-fix failure this catches.
        match PlatformSources.open(TrackKind::Mic, MIC_DEVICE_ID) {
            Ok(source) => assert_eq!(source.kind(), TrackKind::Mic),
            Err(error) => assert!(
                !error.contains(MIC_DEVICE_ID),
                "encoded device id reached open_for undecoded: {error}"
            ),
        }
    }

    #[test]
    fn support_matrix_command_matches_spec() {
        let m = support_matrix();
        assert_eq!(m.windows_min, "10");
        assert_eq!(m.macos_min, "14.2");
        assert!(m.linux_requires_pipewire);
    }

    #[test]
    fn configured_main_window_has_expected_label_and_title() {
        let context = app_context();
        let window = context
            .config()
            .app
            .windows
            .iter()
            .find(|window| window.label == "main")
            .expect("a window labeled main must be configured");
        assert_eq!(window.title, "Buddy");
    }
}
