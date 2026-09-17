// Copyright 2026 Mathews Tom
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     https://www.apache.org/licenses/LICENSE-2.0

//! Recording state, as a status display sees it.
//!
//! State and elapsed milliseconds only. No sample, no device, no
//! provider, and no session path crosses this boundary, and the
//! frontend has no way to drive a transition: the host exposes a read
//! and a subscription, and nothing else.

use scrybe_application::recording::{
    CheckOutcome as ServiceCheckOutcome, PreflightCheck as ServicePreflightCheck, PreflightReport,
    RecordingEvent, RecordingSnapshot, RecordingState as ServiceRecordingState,
};
use serde::Serialize;
use ts_rs::TS;

/// The window event one transition is delivered on.
///
/// Emitted into the generated TypeScript, so the frontend subscribes to
/// the name the host actually publishes rather than to a copy that can
/// drift from it.
pub const TRANSITION_EVENT: &str = "recording-transition";

/// Mirrors `scrybe_application::recording::RecordingState` through an
/// exhaustive match.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum RecordingState {
    Idle,
    Preparing,
    Recording,
    Saving,
    Completed,
    Failed,
}

impl From<ServiceRecordingState> for RecordingState {
    fn from(state: ServiceRecordingState) -> Self {
        match state {
            ServiceRecordingState::Idle => Self::Idle,
            ServiceRecordingState::Preparing => Self::Preparing,
            ServiceRecordingState::Recording => Self::Recording,
            ServiceRecordingState::Saving => Self::Saving,
            ServiceRecordingState::Completed => Self::Completed,
            ServiceRecordingState::Failed => Self::Failed,
        }
    }
}

/// Where recording stands right now.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, TS)]
pub struct RecordingStatus {
    /// The service layer's event schema version, forwarded unchanged so
    /// a frontend can refuse a payload it was not built for.
    pub schema_version: u32,
    pub state: RecordingState,
    /// `u64` on the wire is a JSON number, not a `bigint`.
    #[ts(type = "number")]
    pub elapsed_ms: u64,
    pub stop_requested: bool,
    /// The failure summary the service layer wrote. Present only in
    /// `failed`.
    pub failure_summary: Option<String>,
}

impl From<RecordingSnapshot> for RecordingStatus {
    fn from(snapshot: RecordingSnapshot) -> Self {
        Self {
            schema_version: snapshot.schema_version,
            state: snapshot.state.into(),
            elapsed_ms: snapshot.elapsed_ms,
            stop_requested: snapshot.stop_requested,
            failure_summary: snapshot.failure.map(|failure| failure.summary),
        }
    }
}

/// One observed transition.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, TS)]
pub struct RecordingTransition {
    pub schema_version: u32,
    /// Monotonic per process. A frontend that sees a gap knows it
    /// missed an event rather than guessing.
    #[ts(type = "number")]
    pub sequence: u64,
    pub from: RecordingState,
    pub to: RecordingState,
    #[ts(type = "number")]
    pub elapsed_ms: u64,
    pub failure_summary: Option<String>,
}

impl From<&RecordingEvent> for RecordingTransition {
    fn from(event: &RecordingEvent) -> Self {
        Self {
            schema_version: event.schema_version,
            sequence: event.sequence,
            from: event.from.into(),
            to: event.to.into(),
            elapsed_ms: event.elapsed_ms,
            failure_summary: event
                .failure
                .as_ref()
                .map(|failure| failure.summary.clone()),
        }
    }
}

/// Whether this installation can record, check by check.
///
/// The service layer's report, narrowed to what a reader sees. Every
/// check is forwarded — including the one that says it checked nothing
/// — because a surface that dropped the unverified findings would
/// present seven answers as though all of them were measured.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, TS)]
pub struct PreflightView {
    /// The service layer's preflight schema version, forwarded
    /// unchanged so a frontend can refuse a payload it was not built
    /// for.
    pub schema_version: u32,
    /// Whether every check that can block passed.
    pub can_record: bool,
    pub findings: Vec<PreflightFindingView>,
}

/// One check, as a reader sees it.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, TS)]
pub struct PreflightFindingView {
    pub check: PreflightCheckView,
    pub outcome: CheckOutcomeView,
    pub summary: String,
}

/// Mirrors `scrybe_application::recording::PreflightCheck` through an
/// exhaustive match.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum PreflightCheckView {
    Configuration,
    Permissions,
    Device,
    Provider,
    Model,
    Storage,
    Capture,
}

/// Mirrors `scrybe_application::recording::CheckOutcome` through an
/// exhaustive match.
///
/// `Unverified` is a state in its own right and not a shade of
/// `Passed`: the surface that renders it must say what was not checked.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum CheckOutcomeView {
    Passed,
    Unverified,
    Failed,
}

impl From<ServicePreflightCheck> for PreflightCheckView {
    fn from(check: ServicePreflightCheck) -> Self {
        match check {
            ServicePreflightCheck::Configuration => Self::Configuration,
            ServicePreflightCheck::Permissions => Self::Permissions,
            ServicePreflightCheck::Device => Self::Device,
            ServicePreflightCheck::Provider => Self::Provider,
            ServicePreflightCheck::Model => Self::Model,
            ServicePreflightCheck::Storage => Self::Storage,
            ServicePreflightCheck::Capture => Self::Capture,
        }
    }
}

impl From<ServiceCheckOutcome> for CheckOutcomeView {
    fn from(outcome: ServiceCheckOutcome) -> Self {
        match outcome {
            ServiceCheckOutcome::Passed => Self::Passed,
            ServiceCheckOutcome::Unverified => Self::Unverified,
            ServiceCheckOutcome::Failed => Self::Failed,
        }
    }
}

impl From<PreflightReport> for PreflightView {
    fn from(report: PreflightReport) -> Self {
        Self {
            schema_version: report.schema_version,
            can_record: report.can_record(),
            findings: report
                .findings
                .into_iter()
                .map(|finding| PreflightFindingView {
                    check: finding.check.into(),
                    outcome: finding.outcome.into(),
                    summary: finding.summary,
                })
                .collect(),
        }
    }
}
