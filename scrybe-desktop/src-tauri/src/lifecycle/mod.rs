// Copyright 2026 Mathews Tom
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     https://www.apache.org/licenses/LICENSE-2.0

//! One process, one tray, one recoverable window.
//!
//! The guarantees this module exists to hold:
//!
//! - closing the window hides it; the process and the tray survive,
//!   because a persistent recorder that exits when its window closes
//!   would drop a recording with it;
//! - the tray reopens the window, rebuilding it from configuration if
//!   it was destroyed rather than hidden;
//! - a second launch activates this process instead of creating a
//!   second owner of the same storage root and recording state;
//! - quit exits immediately while idle, and refuses while a recording
//!   is in flight rather than abandoning it.

#[cfg(debug_assertions)]
pub mod channel;
#[cfg(debug_assertions)]
pub mod control;
pub mod menu;
#[cfg(debug_assertions)]
pub mod model_probe;
pub mod navigation;
pub mod tray;
pub mod window;

use scrybe_application::recording::RecordingState;
use tauri::Manager as _;

use crate::state::Desktop;

/// Records one observation for the qualification harness.
///
/// Expands to nothing in a release build, so neither the writer nor the
/// event string reaches release codegen.
#[cfg(debug_assertions)]
#[macro_export]
macro_rules! note {
    ($app:expr, $event:expr) => {
        $crate::lifecycle::channel::append($app, $event, None)
    };
    ($app:expr, $event:expr, $detail:expr) => {
        $crate::lifecycle::channel::append($app, $event, Some($detail))
    };
}

/// The release expansion.
///
/// It consumes the application handle so the call site does not become
/// an unused binding, and discards everything else: the event name and
/// detail are never named in the expansion, so no release build carries
/// their string literals. Expanding to a block rather than to nothing
/// is what keeps a call in expression position valid — one of these is
/// the `RunEvent::Exit` match arm, and an empty expansion is a legal
/// statement but not a legal expression, so a release build of this
/// tree would not compile at all.
#[cfg(not(debug_assertions))]
#[macro_export]
macro_rules! note {
    ($app:expr $(, $ignored:expr)* $(,)?) => {{
        let _ = $app;
    }};
}

/// Keeps the process alive when its last window goes away.
///
/// Tauri's default is to exit once no window remains, which is right
/// for a document application and wrong for this one: the tray is the
/// application's persistent surface, and a recording that outlives its
/// window must outlive it. An exit the application asked for carries a
/// status code and is allowed through; an implicit one carries none and
/// is refused.
pub fn keep_running_without_a_window(app: &tauri::AppHandle, event: tauri::RunEvent) {
    match event {
        tauri::RunEvent::ExitRequested {
            api, code: None, ..
        } => {
            api.prevent_exit();
            crate::note!(app, "implicit-exit-prevented");
        }
        tauri::RunEvent::Exit => {
            #[cfg(debug_assertions)]
            control::remove_socket(app);
            crate::note!(app, "exited");
        }
        _ => {}
    }
}

/// What a quit request should do.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Quit {
    /// Nothing is in flight. Exit now.
    Now,
    /// A recording is in flight. Exiting would abandon it.
    ///
    /// The confirmation this would offer a user — stop and save, or
    /// cancel — belongs with the recording controls, which this host
    /// does not yet have. Until then the honest behavior is to refuse
    /// loudly rather than to lose the recording quietly.
    Refused(RecordingState),
}

/// Whether the process may exit right now.
#[must_use]
pub const fn quit_decision(state: RecordingState) -> Quit {
    match state {
        RecordingState::Idle | RecordingState::Completed | RecordingState::Failed => Quit::Now,
        in_flight @ (RecordingState::Preparing
        | RecordingState::Recording
        | RecordingState::Saving) => Quit::Refused(in_flight),
    }
}

/// Exits if nothing is in flight; otherwise reports why it did not.
pub fn request_quit(app: &tauri::AppHandle) {
    let state = app
        .state::<Desktop>()
        .application()
        .recording()
        .snapshot()
        .state;

    match quit_decision(state) {
        Quit::Now => {
            crate::note!(app, "quit-accepted");
            app.exit(0);
        }
        Quit::Refused(in_flight) => {
            let label = state_label(in_flight);
            crate::note!(app, "quit-refused", label);
            eprintln!(
                "scrybe-desktop: refusing to quit while {label}; \
                 stop the recording first"
            );
        }
    }
}

/// The wire spelling of a state, for the lifecycle record and the
/// stderr message.
const fn state_label(state: RecordingState) -> &'static str {
    match state {
        RecordingState::Idle => "idle",
        RecordingState::Preparing => "preparing",
        RecordingState::Recording => "recording",
        RecordingState::Saving => "saving",
        RecordingState::Completed => "completed",
        RecordingState::Failed => "failed",
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn test_quit_while_idle_exits_immediately() {
        assert_eq!(quit_decision(RecordingState::Idle), Quit::Now);
    }

    #[test]
    fn test_quit_while_a_recording_is_in_flight_is_refused() {
        for state in [
            RecordingState::Preparing,
            RecordingState::Recording,
            RecordingState::Saving,
        ] {
            assert_eq!(quit_decision(state), Quit::Refused(state));
        }
    }

    #[test]
    fn test_quit_after_a_recording_settles_exits_immediately() {
        // `Completed` and `Failed` are observable terminal states that
        // settle back to idle. Nothing is in flight in either, so
        // refusing would strand the process.
        assert_eq!(quit_decision(RecordingState::Completed), Quit::Now);
        assert_eq!(quit_decision(RecordingState::Failed), Quit::Now);
    }
}
