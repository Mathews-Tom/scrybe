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
