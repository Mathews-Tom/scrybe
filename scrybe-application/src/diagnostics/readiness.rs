// Copyright 2026 Mathews Tom
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     https://www.apache.org/licenses/LICENSE-2.0

//! Whether this install can record, transcribe, take notes, store the
//! result, and say where any of it goes.
//!
//! Five answers, never one. A wizard that reduced them to a single
//! "ready" would have to pick a policy for the cases the product
//! deliberately allows — recording and transcription working while
//! notes are unavailable, and capture being unmeasured rather than
//! known good — and whichever it picked would be wrong for someone. So each facet carries its own state, and the rule that
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
    /// Nothing here can say whether it works. Distinct from `Ready`,
    /// which is a claim, and from `Blocked`, which is a different
    /// claim: this is the absence of one, and a surface that renders it
    /// must say what was not checked rather than imply either.
    Unverified,
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
    /// Optional by design: a blocked notes facet does not block
    /// recording. What a recording without notes produces is the
    /// recording surface's to state, not this one's.
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
    /// The difference is `NotConfigured` and `Unverified`, neither of
    /// which is a fault. `NotConfigured` for transcription means a
    /// hosted provider is configured — a legitimate choice whose
    /// readiness depends on a credential this application never
    /// handles. `Unverified` is capture, which nothing here measures;
    /// refusing to record on the strength of an unmeasured permission
    /// would be as wrong as asserting one, and macOS raises its own
    /// dialog where a recording actually needs the grant.
    ///
    /// Notes deliberately do not appear at all: notes being
    /// unavailable does not stop a recording from being started.
    /// Egress is disclosure rather than a gate.
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

/// Capture is reported as unverified, because nothing in this layer
/// checks it.
///
/// It used to be reported as `Ready` whenever no permission-denied
/// finding was present — but no probe produces such a finding, so the
/// branch that consumed them was unreachable and the facet was the
/// constant `Ready`. An installation whose microphone permission had
/// been refused was told it was ready to record.
///
/// The declared deviation for this release is that the wizard does not
/// *prompt* for a permission. It does not extend to never *detecting*
/// a refusal and reporting readiness anyway. Detecting one without
/// prompting needs `AVCaptureDevice.authorizationStatus`, which is an
/// AVFoundation call this layer has no binding for; the command-line
/// Doctor's probes are not that, because they capture live audio and
/// ask the user first, which is precisely what a read-only diagnosis
/// must not do.
///
/// So this states what it knows, which is nothing, and the surfaces
/// that render it say what was not checked. `can_record` treats it as
/// not-blocking, exactly as before: refusing to start a recording on
/// the strength of an unmeasured permission would replace one wrong
/// claim with another, and macOS raises its own dialog at the point a
/// recording actually needs the grant.
fn capture(_report: &DiagnosticReport) -> Facet {
    facet(
        FacetState::Unverified,
        "whether macOS has granted microphone and system-audio recording is not checked here;          macOS asks the first time a recording needs it",
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
