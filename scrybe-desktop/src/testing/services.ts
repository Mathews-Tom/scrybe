import type {
  FailureCode,
  RecordingStatus,
  SessionRow,
  SessionRows,
  SettingsSummary,
} from "../generated/bindings";
import type { Scrybe } from "../ipc/ScrybeProvider";

/**
 * A stand-in for the Rust services.
 *
 * The IPC bridge exists only inside the host's WebView, so a view wired
 * straight to it could only be exercised by launching the application.
 * The values here are the shapes the generated contract describes, so a
 * contract change breaks these call sites at compile time.
 */
export function servicesReturning(overrides: Partial<Scrybe> = {}): Scrybe {
  return {
    listSessions: () => Promise.resolve(page([])),
    searchSessions: () => Promise.resolve(page([])),
    settingsSummary: () => Promise.resolve(settings()),
    recordingStatus: () => Promise.resolve(idle()),
    ...overrides,
  };
}

export function page(rows: SessionRow[], overrides: Partial<SessionRows> = {}): SessionRows {
  return { rows, offset: 0, total: rows.length, has_more: false, ...overrides };
}

export function session(overrides: Partial<SessionRow> = {}): SessionRow {
  return {
    id: "2026-04-29-1430-quarterly-review-01HXYZ",
    progress: "complete",
    title: "Quarterly review",
    started_at: "2026-04-29T14:30:00Z",
    duration_secs: 2520,
    ...overrides,
  };
}

export function settings(overrides: Partial<SettingsSummary> = {}): SettingsSummary {
  return {
    config_path: "/configured/config.toml",
    config_exists: true,
    storage_root: "/configured/sessions",
    capture_source: "system",
    transcription_provider: "whisper-local",
    transcription_model: "base.en",
    notes_provider: "stub",
    notes_model: "none",
    hosted_credential_required: false,
    warnings: [],
    ...overrides,
  };
}

export function idle(overrides: Partial<RecordingStatus> = {}): RecordingStatus {
  return {
    schema_version: 1,
    state: "idle",
    elapsed_ms: 0,
    stop_requested: false,
    failure_summary: null,
    ...overrides,
  };
}

/**
 * A rejected command.
 *
 * Tauri rejects with the payload Rust serialized — a stable code and a
 * message — not with an `Error`, and a test that rejected with an
 * `Error` would be exercising a shape the application never sees.
 */
export function commandFailure(code: FailureCode, message: string): Promise<never> {
  // eslint-disable-next-line @typescript-eslint/prefer-promise-reject-errors -- see above
  return Promise.reject({ code, message });
}
