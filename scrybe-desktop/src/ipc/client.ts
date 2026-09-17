import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

import {
  RECORDING_TRANSITION_EVENT,
  type RecordingStatus,
  type RecordingTransition,
  type SessionRows,
  type SettingsSummary,
} from "../generated/bindings";

/**
 * Every command name the frontend may invoke.
 *
 * The host grants exactly these in its capability files, and
 * `client.test.ts` proves the two lists agree. A name that appears here
 * and not there is rejected at the IPC boundary at runtime, which
 * surfaces as a view that never loads rather than as a build failure.
 */
export const COMMANDS = [
  "list_sessions",
  "search_sessions",
  "settings_summary",
  "recording_status",
] as const;

/** One page of the sessions under the configured storage root. */
export function listSessions(offset: number, limit: number): Promise<SessionRows> {
  return invoke<SessionRows>("list_sessions", { offset, limit });
}

/** One page of the sessions matching `query`. */
export function searchSessions(
  query: string,
  offset: number,
  limit: number,
): Promise<SessionRows> {
  return invoke<SessionRows>("search_sessions", { query, offset, limit });
}

/** The configuration, narrowed to what the settings view renders. */
export function settingsSummary(): Promise<SettingsSummary> {
  return invoke<SettingsSummary>("settings_summary");
}

/** Where recording stands right now. */
export function recordingStatus(): Promise<RecordingStatus> {
  return invoke<RecordingStatus>("recording_status");
}

/**
 * Calls `onTransition` for every recording state change until the
 * returned function is called.
 */
export function onRecordingTransition(
  onTransition: (transition: RecordingTransition) => void,
): Promise<() => void> {
  return listen<RecordingTransition>(RECORDING_TRANSITION_EVENT, (event) => {
    onTransition(event.payload);
  });
}
