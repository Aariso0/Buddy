/**
 * Minimal typed bridge over the IPC globals Tauri v2 injects into the webview.
 *
 * `@tauri-apps/api` is deliberately not added: its `invoke` and `listen`
 * helpers are thin wrappers over these same globals, and Buddy only needs the
 * application commands plus one state event. Keeping the wrapper local leaves
 * the already-verified dependency set unchanged.
 */

export type Phase = "idle" | "recording" | "paused";

export interface RecordingView {
  phase: Phase;
  session_id: string | null;
  title: string;
  hud: string;
}

export interface DeviceInfo {
  id: string;
  name: string;
  kind: "system" | "mic";
}

type IpcCallback = (raw: unknown) => void;

interface TauriInternals {
  invoke: <T>(command: string, args?: Record<string, unknown>) => Promise<T>;
  transformCallback: (callback: IpcCallback, once?: boolean) => number;
}

interface EventInternals {
  unregisterListener: (event: string, eventId: number) => void;
}

declare global {
  interface Window {
    __TAURI_INTERNALS__?: TauriInternals;
    __TAURI_EVENT_PLUGIN_INTERNALS__?: EventInternals;
  }
}

function internals(): TauriInternals {
  const api = window.__TAURI_INTERNALS__;
  if (!api) {
    throw new Error("Tauri IPC bridge is unavailable");
  }
  return api;
}

/** Call a Rust command; Tauri rejects the promise with the command's error text. */
export function invoke<T>(
  command: string,
  args?: Record<string, unknown>,
): Promise<T> {
  return internals().invoke<T>(command, args);
}

/** Subscribe to backend `recording-state` events; returns a cleanup function. */
export async function listenRecordingState(
  onView: (view: RecordingView) => void,
): Promise<() => void> {
  const api = internals();
  const handler = api.transformCallback((raw) => {
    const event = raw as { payload: RecordingView };
    onView(event.payload);
  });
  const eventId = await invoke<number>("plugin:event|listen", {
    event: "recording-state",
    target: { kind: "Any" },
    handler,
  });

  return () => {
    window.__TAURI_EVENT_PLUGIN_INTERNALS__?.unregisterListener(
      "recording-state",
      eventId,
    );
    void invoke("plugin:event|unlisten", {
      event: "recording-state",
      eventId,
    });
  };
}
