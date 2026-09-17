import type {
  FailureCode,
  NotesRegeneration,
  PreflightView,
  RecordingStatus,
  SessionDetail,
  SessionNotes,
  SessionRepair,
  SessionRow,
  SessionRows,
  SettingsSummary,
  TranscriptWindow,
} from "../generated/bindings";
import type { Scrybe } from "../ipc/ScrybeProvider";
import {
  diagnostics,
  modelOfferFixture,
  modelOutcome,
  readiness,
  repairOutcome,
  settingsFormFixture,
} from "./setup";

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
    cancelQuery: () => Promise.resolve(false),
    getSession: () => Promise.resolve(detail()),
    readNotes: () => Promise.resolve(notes()),
    readTranscriptPage: (_id, offset, limit) =>
      Promise.resolve(transcriptWindow(offset, limit)),
    repairSession: () => Promise.resolve(repaired()),
    regenerateNotes: () => Promise.resolve(regenerated()),
    revealSession: () => Promise.resolve(),
    copyNotes: () => Promise.resolve(),
    copyTranscript: () => Promise.resolve(),
    settingsSummary: () => Promise.resolve(settings()),
    recordingStatus: () => Promise.resolve(idle()),
    recordingPreflight: () => Promise.resolve(clearedPreflight()),
    onRecordingTransition: () => Promise.resolve(() => undefined),
    settingsForm: () => Promise.resolve(settingsFormFixture()),
    applySettings: () => Promise.resolve(settingsFormFixture()),
    diagnosticsReport: () => Promise.resolve(diagnostics()),
    applyRecovery: () => Promise.resolve(repairOutcome()),
    readinessReport: () => Promise.resolve(readiness()),
    modelOffer: () => Promise.resolve(modelOfferFixture()),
    installModel: () => Promise.resolve(modelOutcome()),
    cancelModelInstall: () => Promise.resolve(false),
    onModelProgress: () => Promise.resolve(() => undefined),
    openSystemSettings: () => Promise.resolve(),
    openAdvancedConfiguration: () => Promise.resolve(),
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

export function detail(overrides: Partial<SessionDetail> = {}): SessionDetail {
  return {
    id: "2026-04-29-1430-quarterly-review-01HXYZ",
    progress: "complete",
    session_id: "01HXYZ",
    title: "Quarterly review",
    started_at: "2026-04-29T14:30:00Z",
    ended_at: "2026-04-29T15:12:00Z",
    duration_secs: 2520,
    artifacts: {
      notes: true,
      transcript: true,
      audio: true,
      playback: true,
      metadata: true,
      ...overrides.artifacts,
    },
    capture: {
      channels: 2,
      layout: "stereo:mic-l,system-r",
      sample_rate_hz: 48000,
      bitrate_bps: 32000,
      ...overrides.capture,
    },
    providers: {
      stt: "whisper-local",
      llm: "stub",
      diarizer: "binary-channel",
      ...overrides.providers,
    },
    actions: { repair: false, regenerate_notes: true, ...overrides.actions },
    ...overrides,
  };
}

export function notes(overrides: Partial<SessionNotes> = {}): SessionNotes {
  return {
    id: "2026-04-29-1430-quarterly-review-01HXYZ",
    progress: "complete",
    markdown: "## TL;DR\n- shipped the thing",
    ...overrides,
  };
}

/** A transcript whose lines say which line they are. */
export function transcriptWindow(
  offset = 0,
  limit = 50,
  total = 4,
): TranscriptWindow {
  const end = Math.min(offset + limit, total);
  return {
    id: "2026-04-29-1430-quarterly-review-01HXYZ",
    progress: "complete",
    cursor: Math.min(offset, total),
    lines: Array.from({ length: Math.max(end - offset, 0) }, (_unused, index) =>
      `line ${(offset + index + 1).toString()}`,
    ),
    total_lines: total,
    next: end < total ? end : null,
  };
}

export function repaired(overrides: Partial<SessionRepair> = {}): SessionRepair {
  return {
    id: "2026-04-29-1430-quarterly-review-01HXYZ",
    outcome: "recovered",
    progress: "complete",
    recovered_secs: 2520,
    channels: 2,
    wrote_metadata: true,
    ...overrides,
  };
}

export function regenerated(
  overrides: Partial<NotesRegeneration> = {},
): NotesRegeneration {
  return {
    id: "2026-04-29-1430-quarterly-review-01HXYZ",
    outcome: "replaced",
    bytes: 512,
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

/**
 * A preflight in which nothing blocks.
 *
 * The permission check is `unverified` rather than `passed`, because
 * that is what the service layer actually reports: nothing in this
 * release measures the capture grant, and a fixture that claimed
 * otherwise would let a view be written against an answer the
 * application never gives.
 */
export function clearedPreflight(
  overrides: Partial<PreflightView> = {},
): PreflightView {
  return {
    schema_version: 1,
    can_record: true,
    findings: [
      {
        check: "configuration",
        outcome: "passed",
        summary: "source synthetic, notes stub, consent quick",
      },
      {
        check: "permissions",
        outcome: "unverified",
        summary:
          "not checked — this release measures no permission grant; macOS raises its own Microphone prompt where the recording needs it",
      },
      {
        check: "device",
        outcome: "passed",
        summary: "the synthetic source opens no input device",
      },
      {
        check: "provider",
        outcome: "passed",
        summary: "notes come from the built-in stub; no provider is contacted",
      },
      {
        check: "model",
        outcome: "passed",
        summary: "transcription uses the built-in stub; no model is loaded",
      },
      {
        check: "storage",
        outcome: "passed",
        summary: "the storage root /tmp/scrybe is writable, under /tmp",
      },
      {
        check: "capture",
        outcome: "passed",
        summary: "this build can open source synthetic",
      },
    ],
    ...overrides,
  };
}
