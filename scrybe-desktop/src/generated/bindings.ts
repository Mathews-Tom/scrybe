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

export type PreflightCheckView = "configuration" | "permissions" | "device" | "provider" | "model" | "storage" | "capture";

export type CheckOutcomeView = "passed" | "unverified" | "failed";

export type PreflightFindingView = { check: PreflightCheckView, outcome: CheckOutcomeView, summary: string, };

export type PreflightView = { 
/**
 * The service layer's preflight schema version, forwarded
 * unchanged so a frontend can refuse a payload it was not built
 * for.
 */
schema_version: number, 
/**
 * Whether every check that can block passed.
 */
can_record: boolean, findings: Array<PreflightFindingView>, };

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

export type SettingsField = "storage_root" | "storage_audio_bitrate_kbps" | "capture_mic_device" | "capture_hotkey" | "record_source" | "record_system_backend" | "record_llm" | "stt_provider" | "stt_model" | "stt_language" | "llm_provider" | "llm_base_url" | "llm_model" | "consent_default_mode" | "shell_indicators" | "agent_access_enabled";

export type SettingsFieldKind = "text" | "integer" | "boolean" | "text_list";

export type SettingsFieldSpec = { field: SettingsField, kind: SettingsFieldKind, };

export type SettingsValue = boolean | bigint | string | Array<string>;

export type SettingsChange = { field: SettingsField, value: SettingsValue, };

export type SettingsForm = { config_path: string, config_exists: boolean, schema_version: number, storage_root: string, storage_audio_bitrate_kbps: number, capture_mic_device: string, capture_hotkey: string | null, record_source: string, record_system_backend: string, record_llm: string, stt_provider: string, stt_model: string, stt_language: string, llm_provider: string, llm_base_url: string, llm_model: string, consent_default_mode: string, shell_indicators: Array<string>, agent_access_enabled: boolean, hosted_credential_required: boolean, 
/**
 * Every field this form may write, in the service layer's stable
 * order, with the control kind each one takes.
 */
editable: Array<SettingsFieldSpec>, warnings: Array<SettingsWarning>, };

export type ReadinessState = "ready" | "blocked" | "not_configured" | "unverified";

export type ReadinessFacet = { state: ReadinessState, summary: string, };

export type ReadinessReport = { capture: ReadinessFacet, transcription: ReadinessFacet, notes: ReadinessFacet, storage: ReadinessFacet, egress: ReadinessFacet, can_record: boolean, };

export type RecoveryActionView = { 
/**
 * The action's tagged discriminant.
 */
action: string, 
/**
 * The session the action names, when it names one.
 */
id?: string | null, 
/**
 * The file the action names, when it names one.
 */
name?: string | null, 
/**
 * The capability the action names, when it names one.
 */
capability?: string | null, 
/**
 * What the platform calls that capability, and where it is
 * granted. Present only for a permission recovery.
 */
capability_label?: string | null, settings_url?: string | null, };

export type DiagnosticRow = { code: string, severity: WarningSeverity, component: string, summary: string, 
/**
 * `None` when nothing can be done about it.
 */
recovery_action: RecoveryActionView | null, 
/**
 * Whether acting on it changes the system. Derived in the service
 * layer from the action's own shape.
 */
mutation_required: boolean, };

export type DiagnosticRows = { rows: Array<DiagnosticRow>, warning_count: number, };

export type RepairOutcome = { 
/**
 * `true` when the system was changed, `false` when the condition
 * had already been resolved.
 */
applied: boolean, summary: string, };

export type ModelOffer = { id: string, source_url: string, source_revision: string, license: string, size_bytes: string, sha256: string, runtime: string, destination: string, destination_path: string, required_bytes: string, 
/**
 * `None` when the platform will not report free space, which is
 * shown as unknown rather than as plenty or as none.
 */
available_bytes: string | null, sufficient_space: boolean, state: string, 
/**
 * Present when the state is a failure, describing which one in a
 * sentence a person reads.
 */
failure: string | null, 
/**
 * Present when the state is a failure: which refusal it was, as a
 * value a surface can branch on. `failure` is the prose beside it.
 */
reason: string | null, };

export type ModelProgress = { id: string, received_bytes: string, total_bytes: string, };

export type ModelOutcome = { id: string, state: string, 
/**
 * The line a person reads, present when the state is a failure.
 */
failure: string | null, 
/**
 * Which refusal it was, present when the state is a failure. A
 * full disk and a corrupted download are different problems with
 * different answers, and a surface cannot offer the right one
 * from prose.
 */
reason: string | null, 
/**
 * Whether this call put a verified artifact at the destination.
 */
promoted: boolean, };

export type SessionArtifacts = { notes: boolean, transcript: boolean, audio: boolean, playback: boolean, metadata: boolean, };

export type SessionCapture = { channels: number | null, 
/**
 * Canonical channel-attribution descriptor, e.g.
 * `stereo:mic-l,system-r`.
 */
layout: string | null, sample_rate_hz: number | null, bitrate_bps: number | null, };

export type SessionProviders = { stt: string | null, llm: string | null, diarizer: string | null, };

export type SessionActions = { repair: boolean, regenerate_notes: boolean, };

export type SessionDetail = { 
/**
 * The opaque identity every later command addresses this session
 * by. A folder name, never a path.
 */
id: string, progress: SessionProgress, 
/**
 * The session ULID recorded in `meta.toml`, when metadata exists.
 */
session_id: string | null, title: string | null, 
/**
 * RFC 3339. Rendered in the viewer's locale by the frontend, which
 * is the only side that knows the viewer's locale.
 */
started_at: string | null, 
/**
 * RFC 3339.
 */
ended_at: string | null, 
/**
 * `u64` on the wire is a JSON number, not a `bigint`.
 */
duration_secs: number | null, artifacts: SessionArtifacts, capture: SessionCapture, providers: SessionProviders, actions: SessionActions, };

export type SessionNotes = { id: string, progress: SessionProgress, 
/**
 * `null` when the session has no durable notes, which is not the
 * same as notes that are empty.
 */
markdown: string | null, };

export type TranscriptWindow = { id: string, progress: SessionProgress, 
/**
 * The line this window starts at.
 */
cursor: number, 
/**
 * Transcript lines, newline-stripped.
 */
lines: Array<string>, 
/**
 * Lines in the whole transcript.
 */
total_lines: number, 
/**
 * Where the next window starts, or `null` at the end.
 */
next: number | null, };

export type RepairKind = "recovered" | "metadata_reconstructed" | "nothing_to_repair";

export type SessionRepair = { id: string, outcome: RepairKind, 
/**
 * The session's state after the repair, so the view re-renders its
 * actions from what is true now rather than from what it assumed.
 */
progress: SessionProgress, recovered_secs: number | null, channels: number | null, wrote_metadata: boolean, };

export type NotesOutcome = "replaced" | "unchanged";

export type NotesRegeneration = { id: string, outcome: NotesOutcome, 
/**
 * Bytes of the durable `notes.md` after the operation.
 */
bytes: number, };

export const RECORDING_TRANSITION_EVENT = "recording-transition";
export const MODEL_PROGRESS_EVENT = "scrybe://model-progress";
