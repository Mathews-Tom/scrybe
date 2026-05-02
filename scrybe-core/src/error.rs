// Copyright 2026 Mathews Tom
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     https://www.apache.org/licenses/LICENSE-2.0

//! Error type hierarchy.
//!
//! Mirrors `docs/system-design.md` §4.6. Each adapter trait owns a narrow
//! variant set and composes into [`CoreError`] via `#[from]`. `LifecycleEvent`
//! carries `Arc<dyn std::error::Error + Send + Sync + 'static>` (Tier 1);
//! the variants below are Tier 2.

use std::path::PathBuf;

/// Opaque error payload used by `Platform`, `Transport`, and similar variants.
///
/// `Send + Sync + 'static` is required so library callers can carry these
/// across `tokio` task boundaries.
pub type BoxError = Box<dyn std::error::Error + Send + Sync + 'static>;

/// Top-level error returned by `scrybe-core`'s public surface.
#[derive(thiserror::Error, Debug)]
pub enum CoreError {
    #[error("capture: {0}")]
    Capture(#[from] CaptureError),

    #[error("speech-to-text: {0}")]
    Stt(#[from] SttError),

    #[error("language model: {0}")]
    Llm(#[from] LlmError),

    #[error("storage: {0}")]
    Storage(#[from] StorageError),

    #[error("config: {0}")]
    Config(#[from] ConfigError),

    #[error("consent: {0}")]
    Consent(#[from] ConsentError),

    #[error("pipeline: {0}")]
    Pipeline(#[from] PipelineError),
}

#[derive(thiserror::Error, Debug)]
pub enum CaptureError {
    #[error("permission denied: {0}")]
    PermissionDenied(String),

    #[error("device not available: {0}")]
    DeviceUnavailable(String),

    #[error("default input device changed mid-session: was {was}, now {now}")]
    DeviceChanged { was: String, now: String },

    #[error("system entered sleep state mid-session at {at_secs}s")]
    SystemSlept { at_secs: u64 },

    #[error("unsupported sample rate: {0} Hz")]
    UnsupportedSampleRate(u32),

    #[error("platform API error: {0}")]
    Platform(#[source] BoxError),

    #[error("capture stream closed unexpectedly")]
    StreamClosed,
}

#[derive(thiserror::Error, Debug)]
pub enum SttError {
    #[error("model not loaded: {0}")]
    ModelNotLoaded(String),

    #[error("model file corrupt or wrong checksum: {}", path.display())]
    ModelCorrupt { path: PathBuf },

    #[error("provider returned non-success status: {status}")]
    ProviderStatus { status: u16 },

    #[error("retry budget exhausted after {attempts} attempts")]
    RetriesExhausted { attempts: u32 },

    #[error("transport: {0}")]
    Transport(#[source] BoxError),

    #[error("decoding: {0}")]
    Decoding(#[source] BoxError),
}

#[derive(thiserror::Error, Debug)]
pub enum LlmError {
    #[error("provider returned non-success status: {status}")]
    ProviderStatus { status: u16 },

    #[error("retry budget exhausted after {attempts} attempts")]
    RetriesExhausted { attempts: u32 },

    #[error("prompt rendering: {0}")]
    PromptRendering(String),

    #[error("transport: {0}")]
    Transport(#[source] BoxError),
}

#[derive(thiserror::Error, Debug)]
pub enum HookError {
    #[error("hook timed out after {timeout_ms} ms")]
    Timeout { timeout_ms: u32 },

    #[error("hook returned error: {0}")]
    Hook(#[source] BoxError),
}

// `Io` is the canonical promotion target for `std::io::Error` via `?`.
// `AtomicRename` and `Persist` carry `source: std::io::Error` and must
// always be constructed manually; do not collapse them into `Io`. The
// extra context (which path, which step) is load-bearing for crash
// triage.
#[derive(thiserror::Error, Debug)]
pub enum StorageError {
    #[error("disk full or quota exceeded at {}", path.display())]
    DiskFull { path: PathBuf },

    #[error("session lock held by pid {pid} at {}", path.display())]
    SessionLocked { pid: u32, path: PathBuf },

    #[error("atomic rename failed: {}", path.display())]
    AtomicRename {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("invalid target path (no parent): {}", path.display())]
    InvalidTargetPath { path: PathBuf },

    #[error("named-temp-file persist failed: {}", path.display())]
    Persist {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("io: {0}")]
    Io(#[from] std::io::Error),
}

#[derive(thiserror::Error, Debug)]
pub enum ConfigError {
    #[error("config file not found at {}", path.display())]
    NotFound { path: PathBuf },

    #[error("config parse error in {}: {message}", path.display())]
    Parse { path: PathBuf, message: String },

    #[error("unknown config key {key} (line {line})")]
    UnknownKey { key: String, line: usize },

    #[error("missing required value for {key}")]
    Missing { key: String },

    #[error("schema version {found} cannot be auto-migrated to {target}")]
    UnsupportedSchemaVersion { found: u32, target: u32 },
}

#[derive(thiserror::Error, Debug)]
pub enum ConsentError {
    #[error("user aborted at consent prompt")]
    UserAborted,

    #[error("attestation could not be written to meta.toml: {0}")]
    AttestationWriteFailed(#[source] StorageError),

    #[error("notify mode requested but no chat target detected")]
    ChatTargetMissing,

    #[error("announce mode requested but TTS engine unavailable: {0}")]
    TtsUnavailable(String),
}

#[derive(thiserror::Error, Debug)]
pub enum PipelineError {
    #[error("vad initialization failed: {0}")]
    VadInit(#[source] BoxError),

    #[error("resample failed: source rate {source_rate} Hz")]
    Resample { source_rate: u32 },

    #[error("opus encoder failed: {0}")]
    OpusEncode(#[source] BoxError),

    #[error("metadata serialization failed: {0}")]
    MetaSerialize(#[source] BoxError),

    #[error("empty chunk emitted; dropped without sending to stt")]
    EmptyChunk,

    #[error("diarizer unavailable: {reason}")]
    DiarizerUnavailable { reason: String },
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;
    use std::path::PathBuf;

    fn boxed(message: &'static str) -> BoxError {
        let err: std::io::Error = std::io::Error::other(message);
        Box::new(err)
    }

    #[test]
    fn test_core_error_capture_variant_renders_via_display_chain() {
        let inner = CaptureError::PermissionDenied("Screen Recording".into());
        let core: CoreError = inner.into();

        assert_eq!(
            core.to_string(),
            "capture: permission denied: Screen Recording"
        );
    }

    #[test]
    fn test_core_error_stt_variant_includes_attempt_count() {
        let inner = SttError::RetriesExhausted { attempts: 3 };
        let core: CoreError = inner.into();

        assert_eq!(
            core.to_string(),
            "speech-to-text: retry budget exhausted after 3 attempts"
        );
    }

    #[test]
    fn test_core_error_storage_io_promotion_carries_source() {
        let io_err = std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "truncated");
        let storage: StorageError = io_err.into();
        let core: CoreError = storage.into();

        assert!(core.to_string().starts_with("storage: io:"));
        let mut chain = std::error::Error::source(&core);
        let mut depth = 0;
        while let Some(s) = chain {
            depth += 1;
            chain = s.source();
        }
        assert!(depth >= 2, "expected source chain depth ≥ 2, got {depth}");
    }

    #[test]
    fn test_capture_error_device_changed_includes_both_device_names() {
        let err = CaptureError::DeviceChanged {
            was: "MacBook Pro Microphone".into(),
            now: "AirPods Pro".into(),
        };

        let rendered = err.to_string();

        assert!(rendered.contains("MacBook Pro Microphone"));
        assert!(rendered.contains("AirPods Pro"));
    }

    #[test]
    fn test_capture_error_platform_preserves_inner_source() {
        let err = CaptureError::Platform(boxed("kAudioHardwareUnknownPropertyError"));

        assert!(err.to_string().contains("platform API error"));
        assert!(std::error::Error::source(&err).is_some());
    }

    #[test]
    fn test_storage_error_atomic_rename_displays_path_and_chains_source() {
        let path = PathBuf::from("/var/scrybe/notes.md");
        let io_err = std::io::Error::new(std::io::ErrorKind::PermissionDenied, "EACCES");
        let err = StorageError::AtomicRename {
            path,
            source: io_err,
        };

        assert!(err.to_string().contains("/var/scrybe/notes.md"));
        assert!(std::error::Error::source(&err).is_some());
    }

    #[test]
    fn test_config_error_unknown_key_includes_line_number() {
        let err = ConfigError::UnknownKey {
            key: "stt.weight".into(),
            line: 42,
        };

        assert_eq!(err.to_string(), "unknown config key stt.weight (line 42)");
    }

    #[test]
    fn test_consent_error_attestation_write_failed_chains_storage_error() {
        let storage_err = StorageError::DiskFull {
            path: PathBuf::from("/var/scrybe/sess/meta.toml"),
        };
        let err = ConsentError::AttestationWriteFailed(storage_err);

        assert!(err.to_string().contains("attestation could not be written"));
        assert!(std::error::Error::source(&err).is_some());
    }

    #[test]
    fn test_hook_error_timeout_renders_milliseconds() {
        let err = HookError::Timeout { timeout_ms: 30_000 };

        assert_eq!(err.to_string(), "hook timed out after 30000 ms");
    }

    #[test]
    fn test_pipeline_error_resample_includes_source_rate() {
        let err = PipelineError::Resample {
            source_rate: 44_100,
        };

        assert_eq!(err.to_string(), "resample failed: source rate 44100 Hz");
    }

    #[test]
    fn test_pipeline_error_diarizer_unavailable_renders_reason_in_display() {
        let err = PipelineError::DiarizerUnavailable {
            reason: "live binding pending".into(),
        };

        assert_eq!(
            err.to_string(),
            "diarizer unavailable: live binding pending"
        );
    }

    #[test]
    fn test_pipeline_error_diarizer_unavailable_promotes_through_core_error() {
        let core: CoreError = PipelineError::DiarizerUnavailable {
            reason: "feature off".into(),
        }
        .into();

        assert!(core
            .to_string()
            .starts_with("pipeline: diarizer unavailable"));
    }
}
