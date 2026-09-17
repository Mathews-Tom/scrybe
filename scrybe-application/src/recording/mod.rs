// Copyright 2026 Mathews Tom
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     https://www.apache.org/licenses/LICENSE-2.0

//! Recording controller contracts.
//!
//! One process-wide state model backs every recording surface: the CLI
//! signal bridge, the macOS tray, the floating pill, the global hotkey,
//! and any future window. Each surface observes the same
//! [`RecordingSnapshot`] and requests the same idempotent stop, so no
//! surface can define its own notion of "recording".
//!
//! Transition events carry timing and state only. No audio frame, no
//! transcript text, and no notes content is ever part of a recording
//! event.

mod capture;
mod contract;
mod controller;
mod orchestration;
mod plan;
mod preflight;
mod progress;
mod run;
mod stop;

pub use contract::{
    RecordingEvent, RecordingFailure, RecordingFailureKind, RecordingSnapshot, RecordingState,
    StopAcceptance, StopSource, RECORDING_EVENT_SCHEMA_VERSION,
};
pub use controller::{
    MonotonicClock, RecordingController, RecordingEventObserver, SystemMonotonicClock,
};
pub use orchestration::{begin, check, Refusal, PREFLIGHT_FAILURE_SUMMARY};
pub use progress::{
    observe, RecordingProgress, RecordingProgressObserver, SavingStep,
    RECORDING_PROGRESS_SCHEMA_VERSION,
};

pub use run::{
    notes, run, synthetic_frames, transcription, Notes, RecordingRun, SettledConsent, StubNotes,
    StubTranscription, Transcription,
};

#[cfg(feature = "mic-capture")]
pub use capture::microphone_frames;
pub use capture::{CaptureFrames, CaptureRegistry};
pub use stop::{Stop, StopWatch};

pub use plan::{
    expand_tilde, CaptureSource, NotesBackend, RecordingOverrides, RecordingPlan, SystemBackend,
    TranscriptionModel,
};
pub use preflight::{
    CaptureCapability, CaptureSupport, CheckOutcome, PreflightCheck, PreflightFinding,
    PreflightReport, PREFLIGHT_SCHEMA_VERSION,
};

/// Runs every preflight check against a resolved plan.
///
/// Re-exported at the module root rather than left behind a `preflight`
/// path segment, so a frontend names one entry point — the plan, then
/// this — instead of reaching into a submodule.
pub use preflight::run as preflight;
