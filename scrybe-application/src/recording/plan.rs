// Copyright 2026 Mathews Tom
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     https://www.apache.org/licenses/LICENSE-2.0

//! What one recording will actually use, resolved once.
//!
//! Resolution is a policy — which capture source, which system-audio
//! adapter, which input device, which transcription model, which notes
//! backend, which consent mode — and it was previously written inline
//! in the command-line recorder, where no other surface could reach it.
//! It lives here so a desktop host and a terminal resolve the same
//! configuration to the same answers rather than each carrying its own
//! reading of the same file.
//!
//! Nothing here touches the filesystem, enumerates a device, or
//! constructs a provider. Resolution says what was *asked for*;
//! [`super::preflight`] is what says whether it can be had.

use std::path::{Path, PathBuf};

use scrybe_core::config::{
    Config, RecordConfig, RECORD_LLM_OPENAI_COMPAT, RECORD_LLM_STUB, RECORD_SOURCE_MIC,
    RECORD_SOURCE_MIC_SYSTEM, RECORD_SOURCE_SYNTHETIC, RECORD_SYSTEM_BACKEND_SCK,
    RECORD_SYSTEM_BACKEND_TAP,
};
use scrybe_core::record_defaults;
use scrybe_core::types::ConsentMode;
use serde::{Deserialize, Serialize};

use crate::error::{ApplicationError, ErrorCode};
use crate::Result;

/// Where captured audio comes from.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CaptureSource {
    /// A deterministic in-process generator. Records no hardware, so it
    /// is the only source that runs on a machine with no permission
    /// grant and no input device.
    #[default]
    Synthetic,
    /// The selected Core Audio input device.
    Mic,
    /// The selected input device and macOS system audio together, which
    /// is what attributes the far side of a meeting.
    #[serde(rename = "mic+system")]
    MicSystem,
}

impl CaptureSource {
    /// The `[record].source` spelling, so a surface and the file agree.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Synthetic => RECORD_SOURCE_SYNTHETIC,
            Self::Mic => RECORD_SOURCE_MIC,
            Self::MicSystem => RECORD_SOURCE_MIC_SYSTEM,
        }
    }

    /// Whether this source opens a Core Audio input device.
    #[must_use]
    pub const fn uses_input_device(self) -> bool {
        matches!(self, Self::Mic | Self::MicSystem)
    }

    /// Whether this source captures macOS system audio.
    #[must_use]
    pub const fn uses_system_audio(self) -> bool {
        matches!(self, Self::MicSystem)
    }
}

/// Which macOS system-audio adapter `mic+system` opens.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SystemBackend {
    /// `ScreenCaptureKit`, macOS 13+.
    #[default]
    Sck,
    /// Core Audio Taps, macOS 14.4+.
    Tap,
}

impl SystemBackend {
    /// The `[record].system_backend` spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Sck => RECORD_SYSTEM_BACKEND_SCK,
            Self::Tap => RECORD_SYSTEM_BACKEND_TAP,
        }
    }
}

/// Which provider writes `notes.md`.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum NotesBackend {
    /// A fixed templated body. Reaches no network and needs no model.
    #[default]
    Stub,
    /// Any OpenAI-compatible `/chat/completions` endpoint named under
    /// `[llm]`.
    OpenAiCompat,
}

impl NotesBackend {
    /// The `[record].llm` spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Stub => RECORD_LLM_STUB,
            Self::OpenAiCompat => RECORD_LLM_OPENAI_COMPAT,
        }
    }
}

/// Which transcription model the session will load.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind", content = "path")]
pub enum TranscriptionModel {
    /// The built-in deterministic line. Loads nothing.
    Stub,
    /// A `whisper.cpp` model file.
    Whisper(PathBuf),
    /// A directory holding a streaming Sherpa-ONNX model.
    Sherpa(PathBuf),
}

/// What a surface may say that overrides the configuration file.
///
/// Every field is an override rather than a value, so a surface that
/// says nothing gets exactly what the file says. A `~`-prefixed `root`
/// or model path is expanded during resolution, exactly as a path read
/// from the configuration file is.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct RecordingOverrides {
    pub title: Option<String>,
    pub root: Option<PathBuf>,
    pub source: Option<CaptureSource>,
    pub system_backend: Option<SystemBackend>,
    pub input_device: Option<String>,
    pub whisper_model: Option<PathBuf>,
    pub sherpa_model: Option<PathBuf>,
    pub notes: Option<NotesBackend>,
    pub consent: Option<ConsentMode>,
}

/// Everything one recording was asked to use.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RecordingPlan {
    /// The expanded storage root the session folder will be created
    /// under.
    pub root: PathBuf,
    pub title: Option<String>,
    pub source: CaptureSource,
    pub system_backend: SystemBackend,
    /// The requested Core Audio input-device UID. `None` means the
    /// platform default, resolved and pinned once at session start.
    pub input_device: Option<String>,
    pub transcription: TranscriptionModel,
    pub notes: NotesBackend,
    pub consent: ConsentMode,
    /// macOS `VoiceProcessingIO` echo cancellation, from
    /// `[record].aec`.
    pub aec: bool,
}

impl RecordingPlan {
    /// Resolves `config` under `overrides`, expanding `~` against
    /// `home`.
    ///
    /// `home` is passed rather than probed so resolution stays a pure
    /// function of its inputs: a test names a disposable directory and
    /// gets the same answers the process gets against a real home,
    /// without setting an environment variable another test can see.
    /// A `None` home leaves every `~` path alone, which is what a
    /// process with no resolvable home directory should do — silently
    /// reinterpreting `~/scrybe` as a relative directory would write a
    /// reader's recordings next to the working directory.
    ///
    /// # Errors
    ///
    /// [`ErrorCode::ConfigInvalid`] when `[record].source`,
    /// `[record].system_backend`, or `[record].llm` holds a value the
    /// configuration does not define. An unreadable value is refused
    /// rather than replaced with a default: a recording that silently
    /// captured the wrong source because a typo fell back to
    /// `synthetic` would produce a file the reader believes is their
    /// meeting.
    pub fn resolve(
        config: &Config,
        home: Option<&Path>,
        overrides: &RecordingOverrides,
    ) -> Result<Self> {
        let source = resolve_source(overrides.source, &config.record)?;
        let transcription = resolve_transcription(config, home, overrides, source);
        let root = overrides
            .root
            .clone()
            .unwrap_or_else(|| PathBuf::from(&config.storage.root));
        Ok(Self {
            root: expand_tilde(&root, home),
            title: overrides.title.clone(),
            source,
            system_backend: resolve_system_backend(overrides.system_backend, &config.record)?,
            input_device: overrides
                .input_device
                .clone()
                .or_else(|| config.record.input_device.clone()),
            transcription,
            notes: resolve_notes(overrides.notes, &config.record)?,
            consent: overrides.consent.unwrap_or(config.consent.default_mode),
            aec: config.record.aec,
        })
    }
}

fn resolve_source(explicit: Option<CaptureSource>, record: &RecordConfig) -> Result<CaptureSource> {
    if let Some(source) = explicit {
        return Ok(source);
    }
    match record.validated_source() {
        Some(RECORD_SOURCE_SYNTHETIC) => Ok(CaptureSource::Synthetic),
        Some(RECORD_SOURCE_MIC) => Ok(CaptureSource::Mic),
        Some(RECORD_SOURCE_MIC_SYSTEM) => Ok(CaptureSource::MicSystem),
        Some(_) | None => Err(invalid(
            "[record].source",
            &record.source,
            "synthetic, mic, mic+system",
        )),
    }
}

fn resolve_system_backend(
    explicit: Option<SystemBackend>,
    record: &RecordConfig,
) -> Result<SystemBackend> {
    if let Some(backend) = explicit {
        return Ok(backend);
    }
    match record.validated_system_backend() {
        Some(RECORD_SYSTEM_BACKEND_SCK) => Ok(SystemBackend::Sck),
        Some(RECORD_SYSTEM_BACKEND_TAP) => Ok(SystemBackend::Tap),
        Some(_) | None => Err(invalid(
            "[record].system_backend",
            &record.system_backend,
            "sck, tap",
        )),
    }
}

fn resolve_notes(explicit: Option<NotesBackend>, record: &RecordConfig) -> Result<NotesBackend> {
    if let Some(backend) = explicit {
        return Ok(backend);
    }
    match record.validated_llm() {
        Some(RECORD_LLM_STUB) => Ok(NotesBackend::Stub),
        Some(RECORD_LLM_OPENAI_COMPAT) => Ok(NotesBackend::OpenAiCompat),
        Some(_) | None => Err(invalid("[record].llm", &record.llm, "stub, openai-compat")),
    }
}

/// Which model the session loads, in the order the command-line
/// recorder established: an explicit Sherpa directory, then an explicit
/// whisper file, then the legacy `[record].whisper_model` path, then the
/// `[stt].model` name — and only when the source is real hardware.
///
/// The synthetic source deliberately stays on the stub even with a
/// model configured. It generates a fixed tone, so transcribing it with
/// a real model would spend a model load and produce nothing a reader
/// wants.
fn resolve_transcription(
    config: &Config,
    home: Option<&Path>,
    overrides: &RecordingOverrides,
    source: CaptureSource,
) -> TranscriptionModel {
    if let Some(path) = &overrides.sherpa_model {
        return TranscriptionModel::Sherpa(expand_tilde(path, home));
    }
    if let Some(path) = &overrides.whisper_model {
        return TranscriptionModel::Whisper(expand_tilde(path, home));
    }
    if let Some(path) = &config.record.whisper_model {
        return TranscriptionModel::Whisper(expand_tilde(path, home));
    }
    if source != CaptureSource::Synthetic && config.stt.provider == "whisper-local" {
        return record_defaults::whisper_model_path(&config.stt.model)
            .map_or(TranscriptionModel::Stub, TranscriptionModel::Whisper);
    }
    TranscriptionModel::Stub
}

/// Resolves a leading `~` against `home`.
///
/// `~user` is deliberately not resolved: it names someone else's home,
/// and rewriting it to this user's would point the recorder at the
/// wrong person's directory.
#[must_use]
pub fn expand_tilde(path: &Path, home: Option<&Path>) -> PathBuf {
    let Some(home) = home else {
        return path.to_path_buf();
    };
    let text = path.to_string_lossy();
    if let Some(rest) = text.strip_prefix("~/") {
        return home.join(rest);
    }
    if text == "~" {
        return home.to_path_buf();
    }
    path.to_path_buf()
}

fn invalid(key: &str, found: &str, expected: &str) -> ApplicationError {
    ApplicationError::new(
        ErrorCode::ConfigInvalid,
        format!("invalid {key} `{found}`; expected one of: {expected}"),
    )
}
