import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

import { SETUP_COMMANDS } from "./setup";
import {
  RECORDING_PROGRESS_EVENT,
  RECORDING_TRANSITION_EVENT,
  type NotesRegeneration,
  type PreflightView,
  type RecordingProgressView,
  type RecordingStatus,
  type RecordingTransition,
  type RetentionOutcome,
  type SessionDetail,
  type SessionNotes,
  type SessionRepair,
  type SessionRows,
  type SettingsSummary,
  type TranscriptWindow,
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
  "cancel_query",
  "get_session",
  "read_notes",
  "read_transcript_page",
  "repair_session",
  "regenerate_notes",
  "reveal_session",
  "delete_session",
  "archive_session",
  "copy_notes",
  "copy_transcript",
  "settings_summary",
  "recording_status",
  "recording_preflight",
  "start_recording",
  "stop_recording",
  "acknowledge_recording",
  ...SETUP_COMMANDS,
] as const;

/** One page of the sessions under the configured storage root. */
export function listSessions(offset: number, limit: number): Promise<SessionRows> {
  return invoke<SessionRows>("list_sessions", { offset, limit });
}

/**
 * One page of the sessions matching `query`.
 *
 * `requestId` names this search so a later `cancelQuery` can reach it.
 * A cancellation token does not cross the IPC boundary and `invoke` has
 * no abort, so naming the call is the only way to abandon it.
 */
export function searchSessions(
  requestId: string,
  query: string,
  offset: number,
  limit: number,
): Promise<SessionRows> {
  return invoke<SessionRows>("search_sessions", {
    requestId,
    query,
    offset,
    limit,
  });
}

/**
 * Asks the query named `requestId` to stop, and resolves with whether
 * one was running under that name.
 */
export function cancelQuery(requestId: string): Promise<boolean> {
  return invoke<boolean>("cancel_query", { requestId });
}

/** Everything the detail view renders about one session. */
export function getSession(id: string): Promise<SessionDetail> {
  return invoke<SessionDetail>("get_session", { id });
}

/** A session's durable notes, or `null` when it has none. */
export function readNotes(id: string): Promise<SessionNotes> {
  return invoke<SessionNotes>("read_notes", { id });
}

/**
 * One window of a session's transcript, counted in lines.
 *
 * The whole document never crosses this boundary: a view asks for the
 * window it is showing and nothing else.
 */
export function readTranscriptPage(
  id: string,
  offset: number,
  limit: number,
): Promise<TranscriptWindow> {
  return invoke<TranscriptWindow>("read_transcript_page", { id, offset, limit });
}

/** Recovers an interrupted session. */
export function repairSession(id: string): Promise<SessionRepair> {
  return invoke<SessionRepair>("repair_session", { id });
}

/** Replaces a session's notes from its durable transcript. */
export function regenerateNotes(id: string): Promise<NotesRegeneration> {
  return invoke<NotesRegeneration>("regenerate_notes", { id });
}

/**
 * Shows the session's folder in the platform's file manager.
 *
 * A command that succeeds or fails and carries nothing back, so the
 * return is narrowed here rather than leaving a caller with a value to
 * read that says nothing.
 */
export async function revealSession(id: string): Promise<void> {
  await invoke<null>("reveal_session", { id });
}

/**
 * Moves a session to the trash, where the sweep at the next launch
 * removes it once the retention window has passed.
 *
 * The reader has already confirmed. The host checks the recording state
 * at this point rather than trusting the caller, because a confirmation
 * dialog cannot know what happened while it was open.
 */
export async function deleteSession(
  id: string,
  retentionDays: number,
): Promise<RetentionOutcome> {
  return invoke<RetentionOutcome>("delete_session", { id, retentionDays });
}

/** Moves a session to the archive, which the sweep never touches. */
export async function archiveSession(id: string): Promise<RetentionOutcome> {
  return invoke<RetentionOutcome>("archive_session", { id });
}

/** Puts a session's durable notes on the system clipboard. */
export async function copyNotes(id: string): Promise<void> {
  await invoke<null>("copy_notes", { id });
}

/**
 * Puts a session's durable transcript on the system clipboard.
 *
 * Read and copied in Rust, so the document the view deliberately never
 * holds is not assembled here either.
 */
export async function copyTranscript(id: string): Promise<void> {
  await invoke<null>("copy_transcript", { id });
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
 * Whether this installation could record right now, check by check.
 *
 * Reads and starts nothing, so a view may call it whenever it renders.
 */
export function recordingPreflight(): Promise<PreflightView> {
  return invoke<PreflightView>("recording_preflight");
}

/**
 * Starts a recording and returns as soon as it is under way.
 *
 * The host does not wait for the session: what happens next arrives on
 * the transition and progress subscriptions.
 */
export function startRecording(title: string | null): Promise<RecordingStatus> {
  return invoke<RecordingStatus>("start_recording", { title });
}

/**
 * Asks the recording in flight to stop and save.
 *
 * Idempotent: the host's controller decides under one lock whether a
 * request is the one that counts, so pressing twice is not two stops.
 */
export function stopRecording(): Promise<RecordingStatus> {
  return invoke<RecordingStatus>("stop_recording");
}

/**
 * Acknowledges a terminal recording once this surface has rendered its
 * outcome, returning the host to idle.
 *
 * Idempotent: called with nothing terminal, it changes nothing. A view
 * calls this after reading a `completed` or `failed` transition's
 * refreshed status, not before — acknowledging first would return the
 * host to idle before the read that is supposed to observe the outcome.
 */
export function acknowledgeRecording(): Promise<RecordingStatus> {
  return invoke<RecordingStatus>("acknowledge_recording");
}

/**
 * Calls `onProgress` each time saving moves a step, until the returned
 * function is called.
 */
export function onRecordingProgress(
  onProgress: (progress: RecordingProgressView) => void,
): Promise<() => void> {
  return listen<RecordingProgressView>(RECORDING_PROGRESS_EVENT, (event) => {
    onProgress(event.payload);
  });
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
