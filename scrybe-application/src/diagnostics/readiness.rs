// Copyright 2026 Mathews Tom
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     https://www.apache.org/licenses/LICENSE-2.0

//! Whether this install can record, transcribe, take notes, store the
//! result, and say where any of it goes.
//!
//! Five answers, never one. A wizard that reduced them to a single
//! "ready" would have to pick a policy for the case the product
//! deliberately allows — recording and transcription working while
//! notes are unavailable — and whichever it picked would be wrong for
//! someone. So each facet carries its own state, and the rule that
//! recording may begin is stated once, in [`Readiness::can_record`],
//! rather than reassembled by every surface that asks.
//!
//! Every state here is derived from a [`DiagnosticReport`] rather than
//! probed again. There is one set of probes, it is read-only, and this
//! is a projection of what it found — so a readiness screen and a
//! diagnostics screen can never disagree, and neither can mutate.

use serde::{Deserialize, Serialize};

use crate::diagnostics::contract::{DiagnosticCode, DiagnosticReport, Severity};

/// How one facet of the install stands.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FacetState {
    /// Works now.
    Ready,
    /// Does not work, and something can be done about it.
    Blocked,
    /// Not configured to be used at all, so nothing is wrong.
    NotConfigured,
}

/// One facet, and why it is where it is.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Facet {
    pub state: FacetState,
    /// One line a user reads. Never contains transcript, notes, or
    /// audio content.
    pub summary: String,
    /// The finding codes this state was derived from, so a surface can
    /// link straight to the diagnostic that explains it rather than
    /// restating it.
    pub codes: Vec<DiagnosticCode>,
}

/// The five answers a setup screen reports separately.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Readiness {
    pub capture: Facet,
    pub transcription: Facet,
    /// Optional by design. A blocked notes facet does not block
    /// recording; the session records a `notes_missing` outcome.
    pub notes: Facet,
    pub storage: Facet,
    /// Never blocking. Egress is a fact to disclose, not a fault.
    pub egress: Facet,
}

impl Readiness {
    /// Whether a recording may be started.
    ///
    /// Capture, transcription, and storage, and the test is that none
    /// of them is *blocked* rather than that all of them are ready.
    /// The difference is `NotConfigured`, which for transcription means
    /// a hosted provider is configured — a legitimate choice whose
    /// readiness depends on a credential this application never
    /// handles, so this layer cannot assert it and must not treat its
    /// own inability to assert it as a fault.
    ///
    /// Notes deliberately do not appear at all: the product records
    /// with notes unavailable and records a `notes_missing` outcome
    /// afterwards. Egress is disclosure rather than a gate.
    #[must_use]
    pub fn can_record(&self) -> bool {
        self.capture.state != FacetState::Blocked
            && self.transcription.state != FacetState::Blocked
            && self.storage.state != FacetState::Blocked
    }

    /// Derives every facet from one read-only diagnosis.
    #[must_use]
    pub fn from_report(report: &DiagnosticReport) -> Self {
        Self {
            capture: capture(report),
            transcription: transcription(report),
            notes: notes(report),
            storage: storage(report),
            egress: egress(report),
        }
    }
}

/// The findings carrying any of `codes`, in report order.
fn matching<'report>(
    report: &'report DiagnosticReport,
    codes: &[DiagnosticCode],
) -> Vec<&'report crate::diagnostics::contract::DiagnosticFinding> {
    report
        .findings
        .iter()
        .filter(|finding| codes.contains(&finding.code))
        .collect()
}

fn facet(
    state: FacetState,
    summary: impl Into<String>,
    found: &[&crate::diagnostics::contract::DiagnosticFinding],
) -> Facet {
    Facet {
        state,
        summary: summary.into(),
        codes: found.iter().map(|finding| finding.code).collect(),
    }
}

/// Capture is reported from the permission findings, and is `Ready`
/// when none of them says otherwise.
///
/// There is no positive "the microphone works" probe that does not
/// prompt, and prompting is exactly what a read-only diagnosis must
/// not do. So the honest statement is the absence of a known blocker,
/// and the wizard's own permission step is where a grant is actually
/// requested.
fn capture(report: &DiagnosticReport) -> Facet {
    let blockers = matching(
        report,
        &[
            DiagnosticCode::MicrophonePermissionDenied,
            DiagnosticCode::SystemAudioPermissionDenied,
        ],
    );
    if let Some(first) = blockers.first() {
        return facet(FacetState::Blocked, first.summary.clone(), &blockers);
    }
    facet(
        FacetState::Ready,
        "no capture permission is known to be denied",
        &[],
    )
}

fn transcription(report: &DiagnosticReport) -> Facet {
    let present = matching(report, &[DiagnosticCode::TranscriptionModelPresent]);
    if let Some(first) = present.first() {
        return facet(FacetState::Ready, first.summary.clone(), &present);
    }
    let blockers = matching(
        report,
        &[
            DiagnosticCode::TranscriptionModelAbsent,
            DiagnosticCode::TranscriptionModelUnreadable,
        ],
    );
    if let Some(first) = blockers.first() {
        return facet(FacetState::Blocked, first.summary.clone(), &blockers);
    }
    // No local-model finding at all means transcription is not
    // configured to run from one. A hosted provider's readiness is not
    // this layer's to assert: it depends on a credential this
    // application never handles.
    facet(
        FacetState::NotConfigured,
        "transcription does not run from a locally managed model",
        &[],
    )
}

fn notes(report: &DiagnosticReport) -> Facet {
    let reachable = matching(report, &[DiagnosticCode::NotesProviderReachable]);
    if let Some(first) = reachable.first() {
        return facet(FacetState::Ready, first.summary.clone(), &reachable);
    }
    let blockers = matching(report, &[DiagnosticCode::NotesProviderUnreachable]);
    if let Some(first) = blockers.first() {
        return facet(FacetState::Blocked, first.summary.clone(), &blockers);
    }
    facet(
        FacetState::NotConfigured,
        "notes do not run from a local provider",
        &[],
    )
}

fn storage(report: &DiagnosticReport) -> Facet {
    let present = matching(report, &[DiagnosticCode::StorageRootPresent]);
    if let Some(first) = present.first() {
        return facet(FacetState::Ready, first.summary.clone(), &present);
    }
    let absent = matching(report, &[DiagnosticCode::StorageRootAbsent]);
    if let Some(first) = absent.first() {
        // Absent is not blocked: the first recording creates it, and
        // the diagnosis already offers the explicit action that creates
        // it sooner.
        return facet(FacetState::Ready, first.summary.clone(), &absent);
    }
    facet(FacetState::Blocked, "the storage root is unknown", &[])
}

fn egress(report: &DiagnosticReport) -> Facet {
    let found = matching(
        report,
        &[
            DiagnosticCode::SttEgressLocal,
            DiagnosticCode::SttEgressRemote,
            DiagnosticCode::LlmEgressLocal,
            DiagnosticCode::LlmEgressRemote,
        ],
    );
    let remote = found.iter().any(|finding| {
        matches!(
            finding.code,
            DiagnosticCode::SttEgressRemote | DiagnosticCode::LlmEgressRemote
        )
    });
    let summary = if remote {
        "a configured provider sends content off this device"
    } else {
        "every configured provider runs on this device"
    };
    // Always `Ready`: this facet discloses where content goes, and a
    // deliberate hosted provider is a choice rather than a fault.
    facet(FacetState::Ready, summary, &found)
}

/// Whether anything in the report is an error.
///
/// Used by a surface that wants one headline alongside the five
/// facets; the facets remain the answer.
#[must_use]
pub fn has_errors(report: &DiagnosticReport) -> bool {
    report
        .findings
        .iter()
        .any(|finding| finding.severity >= Severity::Error)
}
