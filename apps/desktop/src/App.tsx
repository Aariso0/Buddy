import { useCallback, useEffect, useState } from "react";
import {
  invoke,
  listenRecordingState,
  type DeviceInfo,
  type RecordingView,
} from "./tauri";

const INITIAL_VIEW: RecordingView = {
  phase: "idle",
  session_id: null,
  title: "Buddy",
  // The authoritative HUD text (including the platform hotkey) comes from the
  // Rust snapshot on mount; keep this placeholder free of duplicated shortcut
  // labels so macOS and other platforms never show a stale Ctrl hint.
  hud: "system: idle · mic: idle",
};

const PHASE_LABEL = {
  idle: "Ready",
  recording: "Recording",
  paused: "Paused",
} as const;

export default function App() {
  const [view, setView] = useState<RecordingView>(INITIAL_VIEW);
  const [devices, setDevices] = useState<{ system: DeviceInfo[]; mic: DeviceInfo[] }>({
    system: [],
    mic: [],
  });
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    let active = true;
    let stopListening: (() => void) | undefined;
    let sawStateEvent = false;

    // Subscribe first: a state change between the snapshot read and listener
    // registration would otherwise be lost, and a late snapshot could overwrite
    // a newer event.
    void listenRecordingState((next) => {
      if (active) {
        sawStateEvent = true;
        setView(next);
      }
    })
      .then((stop) => {
        if (!active) {
          stop();
          return;
        }
        stopListening = stop;
        void invoke<RecordingView>("recording_state")
          .then((state) => {
            if (active && !sawStateEvent) setView(state);
          })
          .catch((cause: unknown) => {
            if (active) setError(String(cause));
          });
      })
      .catch((cause: unknown) => {
        if (active) setError(String(cause));
      });

    for (const kind of ["system", "mic"] as const) {
      void invoke<DeviceInfo[]>("list_audio_devices", { kind })
        .then((found) => {
          if (active) {
            setDevices((current) => ({ ...current, [kind]: found }));
          }
        })
        .catch((cause: unknown) => {
          if (active) setError(String(cause));
        });
    }

    return () => {
      active = false;
      stopListening?.();
    };
  }, []);

  const run = useCallback(async (command: string, args?: Record<string, unknown>) => {
    setBusy(true);
    setError(null);
    try {
      await invoke(command, args);
      setView(await invoke<RecordingView>("recording_state"));
    } catch (cause) {
      // Keep the last known view: a failed refresh must not make a live
      // recording look idle just because the follow-up snapshot failed.
      setError(String(cause));
    } finally {
      setBusy(false);
    }
  }, []);

  const active = view.phase !== "idle";
  const sessionId = view.session_id ?? "—";

  return (
    <main>
      <h1>Buddy</h1>

      <section aria-label="Recording status">
        <p data-testid="phase">{PHASE_LABEL[view.phase]}</p>
        <p data-testid="hud" role="status" aria-live="polite">
          {view.hud}
        </p>
        <p>
          Session: <code data-testid="session-id">{sessionId}</code>
        </p>
      </section>

      <section aria-label="Recording controls">
        <button
          type="button"
          disabled={busy || active}
          onClick={() => void run("start_recording", { preset: "lecture" })}
        >
          Start lecture
        </button>
        <button
          type="button"
          disabled={busy || active}
          onClick={() => void run("start_recording", { preset: "meeting" })}
        >
          Start meeting
        </button>
        <button
          type="button"
          disabled={busy || view.phase !== "recording"}
          onClick={() => void run("pause_recording", { id: sessionId })}
        >
          Pause
        </button>
        <button
          type="button"
          disabled={busy || view.phase !== "paused"}
          onClick={() => void run("resume_recording", { id: sessionId })}
        >
          Resume
        </button>
        <button
          type="button"
          disabled={busy || !active}
          onClick={() => void run("stop_recording", { id: sessionId })}
        >
          Stop
        </button>
      </section>

      <section aria-label="Audio devices">
        <p>System: {devices.system.map((device) => device.name).join(", ") || "none"}</p>
        <p>Microphone: {devices.mic.map((device) => device.name).join(", ") || "none"}</p>
      </section>

      {error ? (
        <p role="alert" data-testid="error">
          {error}
        </p>
      ) : null}
    </main>
  );
}
