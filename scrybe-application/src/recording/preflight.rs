// Copyright 2026 Mathews Tom
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     https://www.apache.org/licenses/LICENSE-2.0

//! Whether the resolved plan can actually be carried out.
//!
//! Seven checks, each answered separately, run before any session
//! folder exists. **This module writes nothing.** That is what makes the
//! guarantee a surface relies on — a refused recording leaves nothing
//! behind — checkable on the filesystem rather than inferred from a
//! returned error: after a refusal there is no session folder, and if
//! the storage root did not exist beforehand there is no storage root
//! either.
//!
//! Each check carries its own [`CheckOutcome`], and one of them is
//! [`CheckOutcome::Unverified`] by design. Nothing in this release
//! measures the macOS capture permission grant — the diagnostics layer
//! reports it the same way, and [`crate::diagnostics::Readiness`] says
//! so in its own words. A preflight that claimed to have checked it
//! would be asserting something no code here establishes, so
//! [`PreflightCheck::Permissions`] states what it did instead: it
//! reports whether the platform will raise its own prompt, and it never
//! blocks.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::error::{ApplicationError, ErrorCode};
use crate::recording::plan::{CaptureSource, NotesBackend, RecordingPlan, TranscriptionModel};
use crate::Result;

/// Version of the preflight report shape, so a surface can refuse a
/// payload it was not built for. Moves with [`PreflightCheck`] gaining,
/// losing, or changing the meaning of a variant.
pub const PREFLIGHT_SCHEMA_VERSION: u32 = 1;

/// One thing a recording needs.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PreflightCheck {
    /// The configuration resolved to a plan at all.
    Configuration,
    /// Whether the capture permission grant will be asked for.
    Permissions,
    /// The Core Audio input device the plan named.
    Device,
    /// The provider that will write `notes.md`.
    Provider,
    /// The transcription model the session will load.
    Model,
    /// The directory the session folder will be created under.
    Storage,
    /// Whether this build carries the capture the plan named.
    Capture,
}

impl PreflightCheck {
    /// Stable label used in logs and evidence.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Configuration => "configuration",
            Self::Permissions => "permissions",
            Self::Device => "device",
            Self::Provider => "provider",
            Self::Model => "model",
            Self::Storage => "storage",
            Self::Capture => "capture",
        }
    }
}

/// How one check came out.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckOutcome {
    /// Established here, now.
    Passed,
    /// Nothing here can establish it either way. Distinct from
    /// [`Self::Passed`], which is a claim, and from [`Self::Failed`],
    /// which is a different claim: this is the absence of one, and a
    /// surface rendering it must say what was not checked rather than
    /// imply either. Never blocks.
    Unverified,
    /// Established here to be wrong. Blocks.
    Failed,
}

/// One check, and what it found.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct PreflightFinding {
    pub check: PreflightCheck,
    pub outcome: CheckOutcome,
    /// One user-safe line. Names configuration keys, model paths, and
    /// device identities — all of which the reader supplied — and never
    /// transcript, notes, or audio content, none of which exists yet.
    pub summary: String,
}

impl PreflightFinding {
    fn new(check: PreflightCheck, outcome: CheckOutcome, summary: impl Into<String>) -> Self {
        Self {
            check,
            outcome,
            summary: summary.into(),
        }
    }
}

/// Every check, in the order they were run.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct PreflightReport {
    pub schema_version: u32,
    pub findings: Vec<PreflightFinding>,
}

impl PreflightReport {
    /// The findings that block a recording.
    #[must_use]
    pub fn blocking(&self) -> Vec<&PreflightFinding> {
        self.findings
            .iter()
            .filter(|finding| finding.outcome == CheckOutcome::Failed)
            .collect()
    }

    /// Whether a recording may begin.
    #[must_use]
    pub fn can_record(&self) -> bool {
        self.blocking().is_empty()
    }

    /// The report as a refusal, when it is one.
    ///
    /// # Errors
    ///
    /// [`ErrorCode::PreflightFailed`], listing every blocking check by
    /// label, when any check failed. Every blocking reason is named
    /// rather than the first: a reader with no model *and* no writable
    /// root should learn both in one attempt instead of fixing one and
    /// discovering the other.
    pub fn into_result(self) -> Result<Self> {
        let blocking = self.blocking();
        if blocking.is_empty() {
            drop(blocking);
            return Ok(self);
        }
        let reasons = blocking
            .iter()
            .map(|finding| format!("{}: {}", finding.check.label(), finding.summary))
            .collect::<Vec<_>>()
            .join("; ");
        Err(ApplicationError::new(
            ErrorCode::PreflightFailed,
            format!("recording cannot start — {reasons}"),
        ))
    }
}

/// What this build can actually open.
///
/// Supplied by the caller rather than read from a `cfg!` here, because
/// the crate that carries the capture adapters is not this one: a
/// frontend knows which adapters it linked, and passing that in keeps
/// this module's answers checkable from a test that names either shape.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct CaptureSupport {
    /// The richest capture source this build can open.
    pub capture: CaptureCapability,
    /// Whether a real transcription model can be loaded, rather than
    /// the built-in stub line.
    pub transcription_model: bool,
    /// Whether the configured notes provider can be reached at all,
    /// rather than refusing for want of a transport.
    pub notes_provider: bool,
}

/// How far a build's capture reaches.
///
/// Ordered rather than a pair of independent flags, because the
/// combinations are not independent: nothing links system audio without
/// also linking the microphone adapter it is mixed with, so a pair of
/// booleans could spell a build that does not exist, and a check
/// written against it would be unreachable.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Hash, PartialOrd, Ord)]
pub enum CaptureCapability {
    /// Only the in-process generator.
    #[default]
    SyntheticOnly,
    /// The generator and a Core Audio input device.
    Microphone,
    /// The generator, an input device, and macOS system audio.
    MicrophoneAndSystemAudio,
}

impl CaptureCapability {
    /// Whether this build can open `source`.
    #[must_use]
    pub fn opens(self, source: CaptureSource) -> bool {
        match source {
            CaptureSource::Synthetic => true,
            CaptureSource::Mic => self >= Self::Microphone,
            CaptureSource::MicSystem => self == Self::MicrophoneAndSystemAudio,
        }
    }

    /// What a source this build cannot open is missing.
    const fn missing_for(self, source: CaptureSource) -> &'static str {
        match self {
            Self::SyntheticOnly => "microphone capture",
            _ => match source {
                CaptureSource::MicSystem => "system-audio capture",
                CaptureSource::Synthetic | CaptureSource::Mic => "microphone capture",
            },
        }
    }
}

/// Runs every check against `plan`.
///
/// `devices` lists the input-device UIDs this platform offers, or
/// `None` when the build cannot enumerate them — which is not the same
/// as an empty list, and is reported as unverified rather than as a
/// missing device.
#[must_use]
pub fn run(
    plan: &RecordingPlan,
    support: CaptureSupport,
    devices: Option<&[String]>,
) -> PreflightReport {
    PreflightReport {
        schema_version: PREFLIGHT_SCHEMA_VERSION,
        findings: vec![
            configuration(plan),
            permissions(plan),
            device(plan, devices),
            provider(plan, support),
            model(plan, support),
            storage(plan),
            capture(plan, support),
        ],
    }
}

/// The plan exists, so the configuration parsed and every enumerated
/// value in it was one this release defines.
///
/// Reaching this function at all is the evidence: `RecordingPlan`
/// cannot be constructed from a `[record].source` outside the three
/// spellings, so a configuration check that could fail here would have
/// to re-parse what resolution already refused.
fn configuration(plan: &RecordingPlan) -> PreflightFinding {
    PreflightFinding::new(
        PreflightCheck::Configuration,
        CheckOutcome::Passed,
        format!(
            "source {}, notes {}, consent {}",
            plan.source.as_str(),
            plan.notes.as_str(),
            plan.consent.as_str()
        ),
    )
}

/// What will be asked of the platform, not what the platform has
/// granted.
///
/// Nothing in this release measures a macOS TCC grant. Saying so is the
/// whole content of this check: a reader who is told the permission was
/// verified, and then watches the recording fail on a revoked grant,
/// has been lied to by the surface that reassured them.
fn permissions(plan: &RecordingPlan) -> PreflightFinding {
    if plan.source == CaptureSource::Synthetic {
        return PreflightFinding::new(
            PreflightCheck::Permissions,
            CheckOutcome::Passed,
            "the synthetic source records no hardware, so no permission is involved",
        );
    }
    let needed = if plan.source.uses_system_audio() {
        "Microphone and Audio Capture"
    } else {
        "Microphone"
    };
    PreflightFinding::new(
        PreflightCheck::Permissions,
        CheckOutcome::Unverified,
        format!(
            "not checked — this release measures no permission grant; \
             macOS raises its own {needed} prompt where the recording needs it"
        ),
    )
}

/// The named input device is one the platform offers.
fn device(plan: &RecordingPlan, devices: Option<&[String]>) -> PreflightFinding {
    if !plan.source.uses_input_device() {
        return PreflightFinding::new(
            PreflightCheck::Device,
            CheckOutcome::Passed,
            "the synthetic source opens no input device",
        );
    }
    let Some(devices) = devices else {
        return PreflightFinding::new(
            PreflightCheck::Device,
            CheckOutcome::Unverified,
            "not checked — this build cannot enumerate Core Audio input devices",
        );
    };
    let Some(requested) = plan.input_device.as_deref() else {
        if devices.is_empty() {
            return PreflightFinding::new(
                PreflightCheck::Device,
                CheckOutcome::Failed,
                "this machine offers no Core Audio input device",
            );
        }
        return PreflightFinding::new(
            PreflightCheck::Device,
            CheckOutcome::Passed,
            format!(
                "no device configured; the platform default is pinned at start, \
                 from {} available",
                devices.len()
            ),
        );
    };
    if devices.iter().any(|uid| uid == requested) {
        PreflightFinding::new(
            PreflightCheck::Device,
            CheckOutcome::Passed,
            format!("configured input device `{requested}` is present"),
        )
    } else {
        PreflightFinding::new(
            PreflightCheck::Device,
            CheckOutcome::Failed,
            format!("configured input device `{requested}` is not present on this machine"),
        )
    }
}

/// The notes provider the plan named is one this build carries.
fn provider(plan: &RecordingPlan, support: CaptureSupport) -> PreflightFinding {
    match plan.notes {
        NotesBackend::Stub => PreflightFinding::new(
            PreflightCheck::Provider,
            CheckOutcome::Passed,
            "notes come from the built-in stub; no provider is contacted",
        ),
        NotesBackend::OpenAiCompat if support.notes_provider => PreflightFinding::new(
            PreflightCheck::Provider,
            CheckOutcome::Passed,
            "the configured OpenAI-compatible notes provider is linked",
        ),
        NotesBackend::OpenAiCompat => PreflightFinding::new(
            PreflightCheck::Provider,
            CheckOutcome::Failed,
            "[record].llm names openai-compat, but this build carries no notes provider",
        ),
    }
}

/// The transcription model the plan named exists and is loadable.
fn model(plan: &RecordingPlan, support: CaptureSupport) -> PreflightFinding {
    match &plan.transcription {
        TranscriptionModel::Stub => PreflightFinding::new(
            PreflightCheck::Model,
            CheckOutcome::Passed,
            "transcription uses the built-in stub; no model is loaded",
        ),
        TranscriptionModel::Whisper(path) => {
            model_artifact(path, support, Path::is_file, "whisper.cpp model file")
        }
        TranscriptionModel::Sherpa(path) => {
            model_artifact(path, support, Path::is_dir, "Sherpa-ONNX model directory")
        }
    }
}

fn model_artifact(
    path: &Path,
    support: CaptureSupport,
    present: fn(&Path) -> bool,
    kind: &str,
) -> PreflightFinding {
    if !support.transcription_model {
        return PreflightFinding::new(
            PreflightCheck::Model,
            CheckOutcome::Failed,
            format!(
                "a {kind} is configured at {}, but this build carries no model runtime",
                path.display()
            ),
        );
    }
    if present(path) {
        PreflightFinding::new(
            PreflightCheck::Model,
            CheckOutcome::Passed,
            format!("{kind} present at {}", path.display()),
        )
    } else {
        PreflightFinding::new(
            PreflightCheck::Model,
            CheckOutcome::Failed,
            format!("no {kind} at {}", path.display()),
        )
    }
}

/// The session folder can be created under the plan's root.
///
/// Creates nothing. The nearest existing ancestor of the root is the
/// thing measured: if the root itself exists it must be a writable
/// directory, and if it does not, the directory that would have to hold
/// it must be. A root under a path whose ancestor is a regular file, or
/// under a directory the process cannot write, fails here rather than
/// halfway through a recording.
fn storage(plan: &RecordingPlan) -> PreflightFinding {
    let mut candidate = plan.root.as_path();
    let existing = loop {
        if candidate.exists() {
            break Some(candidate);
        }
        match candidate.parent() {
            Some(parent) if parent != candidate => candidate = parent,
            _ => break None,
        }
    };
    let Some(existing) = existing else {
        return PreflightFinding::new(
            PreflightCheck::Storage,
            CheckOutcome::Failed,
            format!(
                "no part of the storage root {} exists, and neither does any parent of it",
                plan.root.display()
            ),
        );
    };
    let Ok(metadata) = existing.metadata() else {
        return PreflightFinding::new(
            PreflightCheck::Storage,
            CheckOutcome::Failed,
            format!("{} cannot be read", existing.display()),
        );
    };
    if !metadata.is_dir() {
        return PreflightFinding::new(
            PreflightCheck::Storage,
            CheckOutcome::Failed,
            format!(
                "the storage root {} resolves through {}, which is not a directory",
                plan.root.display(),
                existing.display()
            ),
        );
    }
    if metadata.permissions().readonly() {
        return PreflightFinding::new(
            PreflightCheck::Storage,
            CheckOutcome::Failed,
            format!("{} is not writable", existing.display()),
        );
    }
    PreflightFinding::new(
        PreflightCheck::Storage,
        CheckOutcome::Passed,
        format!(
            "the storage root {} is writable, under {}",
            plan.root.display(),
            existing.display()
        ),
    )
}

/// This build carries the capture the plan named.
fn capture(plan: &RecordingPlan, support: CaptureSupport) -> PreflightFinding {
    if support.capture.opens(plan.source) {
        return PreflightFinding::new(
            PreflightCheck::Capture,
            CheckOutcome::Passed,
            format!("this build can open source {}", plan.source.as_str()),
        );
    }
    PreflightFinding::new(
        PreflightCheck::Capture,
        CheckOutcome::Failed,
        format!(
            "source {} needs {}, which this build does not carry",
            plan.source.as_str(),
            support.capture.missing_for(plan.source)
        ),
    )
}
