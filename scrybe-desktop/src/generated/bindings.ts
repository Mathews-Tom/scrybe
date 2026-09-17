// Generated from `src-tauri/src/contract` — do not edit by hand.
//
// Regenerate with `pnpm --dir scrybe-desktop run bindings`. Verify with
// `pnpm --dir scrybe-desktop run check:bindings`, which fails when this
// file and the Rust contract disagree.
//
// Declarations appear in dependency order, so the file reads top to
// bottom and the drift check compares a stable byte sequence rather
// than whatever order a hash map happened to produce.

export type FailureCode = "invalid_session_id" | "session_not_found" | "ambiguous_session_id" | "storage_root_missing" | "storage_unavailable" | "metadata_unreadable" | "cancelled" | "not_applicable" | "config_unreadable" | "config_invalid" | "config_write_failed" | "diagnostics_unavailable" | "repair_failed" | "notes_generation_failed" | "recording_state_conflict" | "preflight_failed" | "model_manifest_invalid" | "model_unknown" | "model_confirmation_required" | "model_storage_unavailable" | "model_download_unavailable";

export type CommandFailure = { code: FailureCode, message: string, };

export type SessionProgress = "complete" | "unfinished" | "repairable" | "failed";

export type SessionRow = { 
/**
 * The opaque identity later commands address this session by. It
 * is a folder name, never a path: the service layer refuses
 * separators, traversal, and drive- or home-relative forms at
 * construction, and resolves it beneath the configured root.
 */
id: string, progress: SessionProgress, title: string | null, 
/**
 * RFC 3339. Rendered in the viewer's locale by the frontend, which
 * is the only side that knows the viewer's locale.
 */
started_at: string | null, 
/**
 * `u64` on the wire is a JSON number, not a `bigint`.
 */
duration_secs: number | null, };

export type SessionRows = { rows: Array<SessionRow>, offset: number, total: number, has_more: boolean, };

export type RecordingState = "idle" | "preparing" | "recording" | "saving" | "completed" | "failed";

export type RecordingStatus = { 
/**
 * The service layer's event schema version, forwarded unchanged so
 * a frontend can refuse a payload it was not built for.
 */
schema_version: number, state: RecordingState, 
/**
 * `u64` on the wire is a JSON number, not a `bigint`.
 */
elapsed_ms: number, stop_requested: boolean, 
/**
 * The failure summary the service layer wrote. Present only in
 * `failed`.
 */
failure_summary: string | null, };

export type RecordingTransition = { schema_version: number, 
/**
 * Monotonic per process. A frontend that sees a gap knows it
 * missed an event rather than guessing.
 */
sequence: number, from: RecordingState, to: RecordingState, elapsed_ms: number, failure_summary: string | null, };

export type WarningSeverity = "info" | "warning" | "error";

export type SettingsWarning = { severity: WarningSeverity, message: string, };

export type SettingsSummary = { 
/**
 * Where the configuration file is, or would be written.
 */
config_path: string, 
/**
 * `false` when no file exists yet and the values below are the
 * built-in defaults.
 */
config_exists: boolean, storage_root: string, capture_source: string, transcription_provider: string, transcription_model: string, notes_provider: string, notes_model: string, 
/**
 * Whether the configured transcription or notes provider is a
 * hosted one that needs a credential. The frontend renders the
 * local/offline indicator from this; the credential itself never
 * leaves Rust.
 */
hosted_credential_required: boolean, warnings: Array<SettingsWarning>, };

export const RECORDING_TRANSITION_EVENT = "recording-transition";
