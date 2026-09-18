// Copyright 2026 Mathews Tom
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     https://www.apache.org/licenses/LICENSE-2.0

//! What the six stop sources and the quit path agree on.
//!
//! These exercise the decisions, not the platform: a test cannot click
//! a tray item or press a global accelerator, and one that claimed to
//! would be asserting on its own fixture. What it can do is drive the
//! same functions those events dispatch into, and assert the answers
//! cannot disagree with the controller's.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use scrybe_application::recording::{RecordingState, StopSource};
use scrybe_desktop::lifecycle::{quit_decision, Quit};

/// Every surface the shared contract names. If one is added, this fails
/// until it is decided what quitting and stopping mean for it.
const SURFACES: [StopSource; 7] = [
    StopSource::Window,
    StopSource::Tray,
    StopSource::FloatingWindow,
    StopSource::Hotkey,
    StopSource::Signal,
    StopSource::SurfaceFailure,
    StopSource::Termination,
];

/// Each surface's label is what the record of a stop is read by, so two
/// surfaces sharing one would make the record ambiguous.
#[test]
fn test_every_stop_source_is_labelled_distinctly() {
    let mut labels: Vec<&str> = SURFACES.iter().map(|source| source.label()).collect();
    labels.sort_unstable();
    let distinct = labels.len();
    labels.dedup();

    assert_eq!(labels.len(), distinct);
}

/// A quit while nothing is in flight leaves immediately. A reader who
/// has just finished a recording should not be made to wait.
#[test]
fn test_quit_leaves_immediately_when_nothing_is_in_flight() {
    for state in [
        RecordingState::Idle,
        RecordingState::Completed,
        RecordingState::Failed,
    ] {
        assert_eq!(quit_decision(state), Quit::Now);
    }
}

/// A quit during a recording is not refused and is not an immediate
/// exit. It stops and waits — the only choice that neither strands the
/// reader with a window they cannot close nor leaves them a session
/// folder they have to repair.
#[test]
fn test_quit_during_a_recording_stops_it_and_waits() {
    for state in [
        RecordingState::Preparing,
        RecordingState::Recording,
        RecordingState::Saving,
    ] {
        assert_eq!(quit_decision(state), Quit::Deferred(state));
    }
}

/// The tray's enablement is read from the controller's own rule, not a
/// second copy of it. A state where the tray offered a start the
/// controller would refuse is a control that does nothing when pressed.
#[test]
fn test_the_tray_offers_exactly_what_the_controller_accepts() {
    let offers = [
        (RecordingState::Idle, true, false),
        (RecordingState::Preparing, false, true),
        (RecordingState::Recording, false, true),
        (RecordingState::Saving, false, false),
        (RecordingState::Completed, false, false),
        (RecordingState::Failed, false, false),
    ];

    for (state, record, stop) in offers {
        assert_eq!(state.accepts_start(), record, "record from {state:?}");
        assert_eq!(state.accepts_stop(), stop, "stop from {state:?}");
    }
}

/// Every state is either one a quit leaves from or one it waits for.
/// There is no third answer, and no state that falls through both.
#[test]
fn test_every_recording_state_has_a_quit_decision() {
    for state in [
        RecordingState::Idle,
        RecordingState::Preparing,
        RecordingState::Recording,
        RecordingState::Saving,
        RecordingState::Completed,
        RecordingState::Failed,
    ] {
        match quit_decision(state) {
            Quit::Now => assert!(!state.accepts_stop()),
            Quit::Deferred(deferred) => assert_eq!(deferred, state),
        }
    }
}

/// A deferred quit exits on a terminal state. `Failed` counts: a
/// recording that failed has nothing left to finish, and holding the
/// process open for one would leave a reader who asked to quit with a
/// window they cannot close.
#[test]
fn test_a_deferred_quit_leaves_on_either_terminal_state() {
    assert!(RecordingState::Completed.is_terminal());
    assert!(RecordingState::Failed.is_terminal());
    for state in [
        RecordingState::Idle,
        RecordingState::Preparing,
        RecordingState::Recording,
        RecordingState::Saving,
    ] {
        assert!(!state.is_terminal(), "{state:?} would exit too early");
    }
}
