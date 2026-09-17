import type {
  DiagnosticRow,
  DiagnosticRows,
  ModelOffer,
  ModelOutcome,
  ReadinessFacet,
  ReadinessReport,
  RepairOutcome,
  SettingsForm,
} from "../generated/bindings";

/**
 * Stand-ins for the setup half of the Rust services.
 *
 * Kept beside `services.ts` rather than inside it so the setup surface
 * and the session surface do not share one builder that every test has
 * to read to understand either.
 *
 * The values are the shapes the generated contract describes, so a
 * contract change breaks these call sites at compile time.
 */
export function readinessFacet(overrides: Partial<ReadinessFacet> = {}): ReadinessFacet {
  return { state: "ready", summary: "nothing is wrong", ...overrides };
}

export function readiness(overrides: Partial<ReadinessReport> = {}): ReadinessReport {
  return {
    capture: readinessFacet(),
    transcription: readinessFacet(),
    notes: readinessFacet(),
    storage: readinessFacet(),
    egress: readinessFacet({ summary: "every configured provider runs on this device" }),
    can_record: true,
    ...overrides,
  };
}

export function settingsFormFixture(overrides: Partial<SettingsForm> = {}): SettingsForm {
  return {
    config_path: "/configured/config.toml",
    config_exists: true,
    schema_version: 1,
    storage_root: "/configured/sessions",
    storage_audio_bitrate_kbps: 32,
    capture_mic_device: "default",
    capture_hotkey: null,
    record_source: "mic+system",
    record_system_backend: "screencapturekit",
    record_llm: "stub",
    stt_provider: "whisper-local",
    stt_model: "small.en",
    stt_language: "auto",
    llm_provider: "openai-compat",
    llm_base_url: "http://127.0.0.1:11434/v1",
    llm_model: "qwen3:8b",
    consent_default_mode: "explicit",
    shell_indicators: ["menu-bar-waveform"],
    agent_access_enabled: false,
    hosted_credential_required: false,
    editable: [
      { field: "storage_root", kind: "text" },
      { field: "capture_mic_device", kind: "text" },
      { field: "capture_hotkey", kind: "text" },
      { field: "shell_indicators", kind: "text_list" },
      { field: "agent_access_enabled", kind: "boolean" },
      { field: "storage_audio_bitrate_kbps", kind: "integer" },
    ],
    warnings: [],
    ...overrides,
  };
}

export function diagnosticRow(overrides: Partial<DiagnosticRow> = {}): DiagnosticRow {
  return {
    code: "storage_root_present",
    severity: "info",
    component: "storage",
    summary: "storage root /configured/sessions holds 0 sessions",
    recovery_action: null,
    mutation_required: false,
    ...overrides,
  };
}

export function diagnostics(rows: DiagnosticRow[] = []): DiagnosticRows {
  return {
    rows,
    warning_count: rows.filter((row) => row.severity !== "info").length,
  };
}

export function modelOfferFixture(overrides: Partial<ModelOffer> = {}): ModelOffer {
  return {
    id: "whisper-small-en",
    source_url:
      "https://huggingface.co/ggerganov/whisper.cpp/resolve/5359861c739e955e79d9a303bcbc70fb988958b1/ggml-small.en.bin",
    source_revision: "5359861c739e955e79d9a303bcbc70fb988958b1",
    license: "MIT",
    size_bytes: "487614201",
    sha256: "c6138d6d58ecc8322097e0f987c32f1be8bb0a18532a3f88f734d1bbf9c41e5d",
    runtime: "whisper-rs 0.13 (GGML)",
    destination: "ggml-small.en.bin",
    destination_path: "/configured/models/ggml-small.en.bin",
    required_bytes: "756049465",
    available_bytes: "900000000000",
    sufficient_space: true,
    state: "available",
    failure: null,
    ...overrides,
  };
}

export function modelOutcome(overrides: Partial<ModelOutcome> = {}): ModelOutcome {
  return {
    id: "whisper-small-en",
    state: "ready",
    failure: null,
    promoted: true,
    ...overrides,
  };
}

export function repairOutcome(overrides: Partial<RepairOutcome> = {}): RepairOutcome {
  return { applied: true, summary: "done", ...overrides };
}
