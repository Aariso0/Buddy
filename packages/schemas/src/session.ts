export const SESSION_FORMAT_VERSION = 1;

export type SessionState = "recording" | "finalizing" | "complete";

export type Preset = "lecture" | "meeting";

export interface SessionManifest {
  id: string;
  format_version: number;
  state: SessionState;
  started_at: string;
  ended_at: string | null;
  preset: Preset;
}
