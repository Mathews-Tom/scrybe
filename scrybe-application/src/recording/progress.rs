// Copyright 2026 Mathews Tom
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     https://www.apache.org/licenses/LICENSE-2.0

//! How far through saving a recording is, and the one place the
//! pipeline's progress drives the controller.
//!
//! Saving is four ordered steps, and a reader watching a window close
//! on a long meeting needs to know which one is running. The pipeline
//! already publishes them; what did not exist was a projection of them
//! that a surface can render, and the controller driving was written
//! inline at the command-line recorder's call site where no other
//! frontend could reach it.
//!
//! **No captured content crosses this boundary.** The pipeline's
//! progress stream carries one variant that holds transcript text, and
//! [`SavingStep::of`] answers `None` for it, so there is no
//! [`RecordingProgress`] it could ever become.

use scrybe_core::session::SessionProgress;
use serde::{Deserialize, Serialize};

use crate::recording::contract::RecordingState;
use crate::recording::controller::RecordingController;

/// Version of the progress shape, so a surface can refuse a payload it
/// was not built for. Moves with [`SavingStep`] gaining, losing, or
/// reordering a variant.
pub const RECORDING_PROGRESS_SCHEMA_VERSION: u32 = 1;

/// One step of finalization, in the order the pipeline runs them.
///
/// Ordered deliberately: a surface renders "step 2 of 4", and the
/// ordering is the pipeline's, not a presentation choice. Transcript
/// finalization has to finish before the audio it was cut from can be
/// encoded; notes are generated from the finalized transcript; and
/// `meta.toml` is written last because it describes everything above
/// it.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SavingStep {
    FinalizingTranscript,
    EncodingAudio,
    GeneratingNotes,
    WritingMetadata,
}

impl SavingStep {
    /// How many steps saving has.
    pub const TOTAL: u32 = 4;

    /// This step's position, from 1.
    #[must_use]
    pub const fn index(self) -> u32 {
        match self {
            Self::FinalizingTranscript => 1,
            Self::EncodingAudio => 2,
            Self::GeneratingNotes => 3,
            Self::WritingMetadata => 4,
        }
    }

    /// Stable label used in logs and evidence.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::FinalizingTranscript => "finalizing-transcript",
            Self::EncodingAudio => "encoding-audio",
            Self::GeneratingNotes => "generating-notes",
            Self::WritingMetadata => "writing-metadata",
        }
    }

    /// The step `event` reports, if it reports one.
    ///
    /// `None` for [`SessionProgress::Recording`], which is not part of
    /// saving, and for [`SessionProgress::TranscriptAccepted`], which
    /// carries a transcribed line. That second `None` is the reason
    /// this function exists rather than the match being written at each
    /// call site: a surface cannot render a step this never produces,
    /// so no recording indicator, event, or log can come to hold what a
    /// meeting said.
    #[must_use]
    pub const fn of(event: &SessionProgress) -> Option<Self> {
        match event {
            SessionProgress::FinalizingTranscript { .. } => Some(Self::FinalizingTranscript),
            SessionProgress::EncodingAudio => Some(Self::EncodingAudio),
            SessionProgress::GeneratingNotes { .. } => Some(Self::GeneratingNotes),
            SessionProgress::WritingMetadata => Some(Self::WritingMetadata),
            SessionProgress::Recording | SessionProgress::TranscriptAccepted(_) => None,
        }
    }
}

/// How far through saving one recording is.
///
/// `index` and `total` are carried rather than derived by the reader so
/// a surface renders the same "2 of 4" the service layer counted, and a
/// step added later does not silently change what an older frontend
/// computes from a list it hardcoded.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct RecordingProgress {
    pub schema_version: u32,
    pub step: SavingStep,
    pub index: u32,
    pub total: u32,
}

impl From<SavingStep> for RecordingProgress {
    fn from(step: SavingStep) -> Self {
        Self {
            schema_version: RECORDING_PROGRESS_SCHEMA_VERSION,
            step,
            index: step.index(),
            total: SavingStep::TOTAL,
        }
    }
}

/// What a surface is handed each time saving moves a step.
pub type RecordingProgressObserver = dyn Fn(RecordingProgress) + Send + Sync;

/// The observer that drives the controller from the pipeline's own
/// boundaries and reports saving progress.
///
/// Both transitions come from the pipeline rather than from a call
/// site's idea of where it is. [`SessionProgress::Recording`] is the
/// moment capture exists, which is where preparing ends and the single
/// monotonic origin starts; the first finalization event is where
/// capture ends and saving begins. Without the second, saving never
/// spanned the finalization window, so a merge, encode, transcribe, or
/// `meta.toml` failure after a capture that ended on its own was
/// labelled a capture failure even though audio exists and the session
/// is repairable.
///
/// `also` runs after the controller has been driven, so a frontend can
/// add its own rendering — printing a line, forwarding a window event —
/// without having to reproduce any of the above.
pub fn observe<F>(
    controller: Option<std::sync::Arc<RecordingController>>,
    on_progress: Option<std::sync::Arc<RecordingProgressObserver>>,
    also: F,
) -> impl Fn(SessionProgress) + Send + Sync
where
    F: Fn(SessionProgress) + Send + Sync,
{
    move |event: SessionProgress| {
        if let Some(controller) = controller.as_deref() {
            drive(controller, &event);
        }
        if let Some(on_progress) = on_progress.as_deref() {
            if let Some(step) = SavingStep::of(&event) {
                on_progress(step.into());
            }
        }
        also(event);
    }
}

/// Moves the controller to whichever state `event` marks the start of.
///
/// A conflict is traced rather than returned: an observer has no way to
/// propagate one, and a transition arriving in a state that cannot take
/// it is a normal race — another surface stopped the recording first —
/// rather than a fault. It is recorded so the race is observable, not
/// dropped.
fn drive(controller: &RecordingController, event: &SessionProgress) {
    match event {
        SessionProgress::Recording => {
            if let Err(conflict) = controller.mark_recording() {
                tracing::debug!(%conflict, "capture began in a state that cannot record");
            }
        }
        // Only the first finalization event, and only out of
        // `Recording`. The pipeline publishes several, and every one
        // after the first would otherwise be a conflict traced once per
        // step.
        event
            if SavingStep::of(event) == Some(SavingStep::FinalizingTranscript)
                && controller.snapshot().state == RecordingState::Recording =>
        {
            if let Err(conflict) = controller.begin_saving() {
                tracing::debug!(%conflict, "finalization began in a state that cannot save");
            }
        }
        _ => {}
    }
}
