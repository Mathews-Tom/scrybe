// Copyright 2026 Mathews Tom
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     https://www.apache.org/licenses/LICENSE-2.0

//! Serializable recording state types.

use serde::{Deserialize, Serialize};

/// Version of the transition-event shape. Bumped whenever
/// [`RecordingEvent`] gains, loses, or changes the meaning of a field,
/// so a consumer can refuse an event it does not understand.
pub const RECORDING_EVENT_SCHEMA_VERSION: u32 = 1;

/// The one recording state model shared by every surface.
///
/// ```text
/// Idle -> Preparing -> Recording -> Saving -> Completed -> Idle
/// Preparing | Recording | Saving -> Failed -> Idle
/// ```
///
/// [`Self::Completed`] and [`Self::Failed`] are observable terminal
/// states that settle back to [`Self::Idle`], so a surface can render
/// the outcome before the controller becomes available again.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecordingState {
    /// Nothing is recording. The only state that accepts a start.
    Idle,
    /// Configuration, permissions, devices, providers, model readiness,
    /// storage, and capture construction are being resolved. No session
    /// folder exists yet.
    Preparing,
    /// Capture is running and the elapsed clock is live.
    Recording,
    /// Capture has stopped and finalization is running. Elapsed time is
    /// frozen and another recording is refused.
    Saving,
    /// Finalization completed and the session is durable.
    Completed,
    /// The attempt failed. [`RecordingFailure`] says at which boundary.
    Failed,
}

impl RecordingState {
    /// Whether a start may be requested from this state.
    #[must_use]
    pub const fn accepts_start(self) -> bool {
        matches!(self, Self::Idle)
    }

    /// Whether a stop may still be requested from this state.
    #[must_use]
    pub const fn accepts_stop(self) -> bool {
        matches!(self, Self::Preparing | Self::Recording)
    }

    /// Whether the state has settled and will return to
    /// [`Self::Idle`].
    #[must_use]
    pub const fn is_terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Failed)
    }

    /// Whether a session may be moved out of the listing right now.
    ///
    /// `Preparing` has no session folder yet and `Recording` and
    /// `Saving` are both writing one — `Saving` especially, because
    /// finalization is the window between a reader's stop and a durable
    /// session, and a move during it corrupts what is being written.
    /// Refusing only while `Recording` would leave exactly that hole.
    #[must_use]
    pub const fn permits_retention(self) -> bool {
        matches!(self, Self::Idle | Self::Completed | Self::Failed)
    }
}

/// Which boundary a recording attempt failed at.
///
/// Kept distinct because the user-facing consequence differs: a
/// preflight failure leaves nothing on disk, a capture failure may
/// leave a recoverable journal, and a finalization failure means audio
/// exists but the session never completed.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecordingFailureKind {
    /// Failed before a session folder was created. Nothing was written.
    Preflight,
    /// Failed while capturing audio.
    Capture,
    /// Failed while merging, encoding, transcribing, or writing
    /// metadata. Durable state may exist and may be repairable.
    Finalization,
}

/// A recording failure as a surface sees it.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct RecordingFailure {
    pub kind: RecordingFailureKind,
    /// One user-safe line. Never carries transcript, notes, or audio
    /// content.
    pub summary: String,
}

impl RecordingFailure {
    /// A failure at `kind` described by `summary`.
    #[must_use]
    pub fn new(kind: RecordingFailureKind, summary: impl Into<String>) -> Self {
        Self {
            kind,
            summary: summary.into(),
        }
    }
}

/// Which surface requested the stop that was accepted.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum StopSource {
    Tray,
    FloatingWindow,
    Hotkey,
    Signal,
    SurfaceFailure,
    Window,
    Termination,
}

impl StopSource {
    /// Stable label used in logs and evidence.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Tray => "tray",
            Self::FloatingWindow => "floating-window",
            Self::Hotkey => "hotkey",
            Self::Signal => "signal",
            Self::SurfaceFailure => "surface-failure",
            Self::Window => "window",
            Self::Termination => "termination",
        }
    }
}

/// What happened to a stop request.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StopAcceptance {
    /// This request won the race. Exactly one request per recording
    /// ever receives this.
    Accepted,
    /// A stop was already in flight; this request changed nothing.
    AlreadyStopping,
    /// Nothing was recording, so there was nothing to stop.
    NotRecording,
}

/// What every recording surface renders from.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct RecordingSnapshot {
    pub schema_version: u32,
    pub state: RecordingState,
    /// Milliseconds since the single monotonic start instant, frozen
    /// once the controller enters [`RecordingState::Saving`].
    pub elapsed_ms: u64,
    /// Whether a stop has been accepted.
    pub stop_requested: bool,
    /// The surface whose stop request was accepted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stop_source: Option<StopSource>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure: Option<RecordingFailure>,
}

impl RecordingSnapshot {
    /// Whether a stop control should be enabled.
    #[must_use]
    pub const fn stop_enabled(&self) -> bool {
        self.state.accepts_stop() && !self.stop_requested
    }

    /// `HH:MM:SS`, or `MM:SS` under an hour.
    #[must_use]
    pub fn elapsed_label(&self) -> String {
        let total_seconds = self.elapsed_ms / 1_000;
        let seconds = total_seconds % 60;
        let minutes = (total_seconds / 60) % 60;
        let hours = total_seconds / 3_600;
        if hours == 0 {
            format!("{minutes:02}:{seconds:02}")
        } else {
            format!("{hours:02}:{minutes:02}:{seconds:02}")
        }
    }
}

/// One state transition.
///
/// Emitted exactly once per transition, in sequence order, carrying no
/// captured content of any kind.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct RecordingEvent {
    pub schema_version: u32,
    /// Monotonically increasing per controller, starting at 1.
    pub sequence: u64,
    pub from: RecordingState,
    pub to: RecordingState,
    pub elapsed_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stop_source: Option<StopSource>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure: Option<RecordingFailure>,
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn test_only_idle_accepts_a_start() {
        for state in [
            RecordingState::Preparing,
            RecordingState::Recording,
            RecordingState::Saving,
            RecordingState::Completed,
            RecordingState::Failed,
        ] {
            assert!(!state.accepts_start(), "{state:?} must refuse a start");
        }

        assert!(RecordingState::Idle.accepts_start());
    }

    #[test]
    fn test_saving_refuses_a_further_stop() {
        assert!(!RecordingState::Saving.accepts_stop());
        assert!(RecordingState::Recording.accepts_stop());
        assert!(RecordingState::Preparing.accepts_stop());
    }

    #[test]
    fn test_a_snapshot_disables_the_stop_control_once_a_stop_is_accepted() {
        let snapshot = RecordingSnapshot {
            schema_version: RECORDING_EVENT_SCHEMA_VERSION,
            state: RecordingState::Recording,
            elapsed_ms: 5_000,
            stop_requested: true,
            stop_source: Some(StopSource::Hotkey),
            failure: None,
        };

        assert!(!snapshot.stop_enabled());
    }

    #[test]
    fn test_elapsed_label_switches_to_hours_past_one_hour() {
        let mut snapshot = RecordingSnapshot {
            schema_version: RECORDING_EVENT_SCHEMA_VERSION,
            state: RecordingState::Recording,
            elapsed_ms: 61_000,
            stop_requested: false,
            stop_source: None,
            failure: None,
        };

        assert_eq!(snapshot.elapsed_label(), "01:01");

        snapshot.elapsed_ms = 3_661_000;

        assert_eq!(snapshot.elapsed_label(), "01:01:01");
    }

    #[test]
    fn test_a_transition_event_serializes_without_any_content_field() {
        let event = RecordingEvent {
            schema_version: RECORDING_EVENT_SCHEMA_VERSION,
            sequence: 3,
            from: RecordingState::Recording,
            to: RecordingState::Saving,
            elapsed_ms: 12_000,
            stop_source: Some(StopSource::Tray),
            failure: None,
        };

        let encoded = serde_json::to_value(&event).unwrap();
        let object = encoded.as_object().unwrap();
        let mut keys: Vec<&str> = object.keys().map(String::as_str).collect();
        keys.sort_unstable();

        assert_eq!(
            keys,
            vec![
                "elapsed_ms",
                "from",
                "schema_version",
                "sequence",
                "stop_source",
                "to"
            ]
        );
    }
}
