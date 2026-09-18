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
pub mod hotkey;
pub mod menu;
#[cfg(debug_assertions)]
pub mod model_probe;
pub mod navigation;
#[cfg(debug_assertions)]
pub mod playback_probe;
#[cfg(unix)]
pub mod signals;
pub mod tray;
pub mod window;

use scrybe_application::recording::{RecordingState, StopSource};
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
    /// A recording is in flight. Stop it, and exit once it is durable.
    ///
    /// Not a refusal: a reader who asks a recorder to quit is asking it
    /// to finish, not to argue. Not an immediate exit either — the
    /// session is recoverable at this point but not complete, and a
    /// process that left now would hand the reader a folder they have
    /// to repair rather than a session they can read.
    Deferred(RecordingState),
}

/// Whether the process may exit right now.
#[must_use]
pub const fn quit_decision(state: RecordingState) -> Quit {
    match state {
        RecordingState::Idle | RecordingState::Completed | RecordingState::Failed => Quit::Now,
        in_flight @ (RecordingState::Preparing
        | RecordingState::Recording
        | RecordingState::Saving) => Quit::Deferred(in_flight),
    }
}

/// Exits if nothing is in flight; otherwise stops the recording and
/// exits once it is durable.
///
/// What is on disk at the moment of a quit during `Saving`: the session
/// folder exists, the journal under it holds every accepted transcript
/// chunk, and the audio has been written but may not yet be merged,
/// encoded, or described by a `meta.toml`. That is precisely the state
/// `scrybe repair` reconstructs a session from — so the work is
/// recoverable, not lost, even if the process is killed here. Deferring
/// the exit is what turns "recoverable" into "already finished", and it
/// is why this waits rather than exiting and relying on the repair.
///
/// Nothing on this path deletes, truncates, or renames anything. The
/// only action it takes on a recording is to request a stop, which is
/// the same request the window's `Stop & save` makes.
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
        Quit::Deferred(in_flight) => {
            let label = state_label(in_flight);
            crate::note!(app, "quit-deferred", label);
            eprintln!("scrybe-desktop: finishing the recording before quitting ({label})");
            let desktop = app.state::<Desktop>();
            let pending = app.state::<PendingQuit>();
            let settled = defer_quit(
                desktop.application().recording(),
                &pending,
                || {
                    // A stop from the termination source. Idempotent:
                    // if the reader already pressed `Stop & save`, the
                    // controller reports `AlreadyStopping` and this
                    // changes nothing.
                    let _ = crate::recording::request_stop(app, StopSource::Termination);
                },
                || {},
            );
            if let Some(state) = settled {
                crate::note!(app, "quit-accepted", state_label(state));
                app.exit(0);
            }
        }
    }
}

/// Arms `pending`, requests a stop, then rechecks the controller
/// directly rather than trusting the transition observer alone.
///
/// Before this existed, arming happened only after the stop request
/// returned. A recording that reached a terminal state in the gap
/// between the caller's read (which decided to defer) and the flag
/// going up found nothing armed when its own transition fired, so the
/// observer's call to [`exit_when_settled`] was a no-op — and arming
/// afterward had nothing left to wake, because no further transition
/// fires for a recording that already settled. The process hung until
/// force-quit, and the flag stayed primed to fire early against
/// whichever later recording settled next.
///
/// `between_read_and_arm` runs first, before the flag goes up.
/// Production passes one that does nothing; `tests::request_quit`
/// below passes one that blocks until a concurrent terminal transition
/// has already landed — including the (then-unarmed, so no-op)
/// observer call that transition triggers — which is what makes the
/// gap this closes reproducible on every run instead of won by luck.
/// The function is Tauri-free by design, so that test drives it
/// directly against a real [`RecordingController`] and [`PendingQuit`]
/// with no mock application involved.
///
/// Returns the state this call itself must report, if any. `Some`
/// only when the recording had already reached a terminal state by
/// the time this returned *and* this call — not the observer — is the
/// one that consumed `pending`'s flag; the observer already having
/// taken it, or the recording still being in flight, both mean nothing
/// left for this call to report.
fn defer_quit(
    controller: &scrybe_application::recording::RecordingController,
    pending: &PendingQuit,
    request_stop: impl FnOnce(),
    between_read_and_arm: impl FnOnce(),
) -> Option<RecordingState> {
    between_read_and_arm();
    pending.arm();
    request_stop();
    let state = controller.snapshot().state;
    (state.is_terminal() && pending.take()).then_some(state)
}

/// Whether a quit is waiting for a recording to finish.
#[derive(Default)]
pub struct PendingQuit {
    armed: std::sync::atomic::AtomicBool,
}

impl PendingQuit {
    fn arm(&self) {
        self.armed.store(true, std::sync::atomic::Ordering::SeqCst);
    }

    /// Whether a quit was deferred, clearing the flag.
    ///
    /// Taken rather than read so two terminal transitions in one
    /// process — a failed attempt followed by a successful one — cannot
    /// exit twice.
    fn take(&self) -> bool {
        self.armed.swap(false, std::sync::atomic::Ordering::SeqCst)
    }
}

/// Exits if a quit was deferred and the recording has now settled.
///
/// Called from the transition observer. `Completed` and `Failed` are
/// both terminal: a failed recording has nothing left to finish, and
/// holding the process open for one would leave a reader who asked to
/// quit with a window they cannot close.
pub fn exit_when_settled(app: &tauri::AppHandle, state: RecordingState) {
    if !state.is_terminal() {
        return;
    }
    if app.state::<PendingQuit>().take() {
        crate::note!(app, "quit-accepted", state_label(state));
        app.exit(0);
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

    use std::sync::mpsc;
    use std::sync::Arc;

    use scrybe_application::{ScrybeApplication, StorageRoot};

    #[test]
    fn test_quit_while_idle_exits_immediately() {
        assert_eq!(quit_decision(RecordingState::Idle), Quit::Now);
    }

    /// A quit while something is in flight does not refuse and does not
    /// exit. It stops the recording and waits — the only choice that
    /// neither strands the reader nor destroys work they believe they
    /// have.
    #[test]
    fn test_quit_while_a_recording_is_in_flight_waits_for_it() {
        for state in [
            RecordingState::Preparing,
            RecordingState::Recording,
            RecordingState::Saving,
        ] {
            assert_eq!(quit_decision(state), Quit::Deferred(state));
        }
    }

    /// A deferred quit fires once. Two terminal transitions in one
    /// process — a refused attempt, then a real recording — must not
    /// exit twice.
    #[test]
    fn test_a_deferred_quit_is_taken_exactly_once() {
        let pending = PendingQuit::default();
        pending.arm();

        assert!(pending.take());
        assert!(!pending.take());
    }

    #[test]
    fn test_a_quit_that_was_never_deferred_does_not_fire() {
        assert!(!PendingQuit::default().take());
    }

    #[test]
    fn test_quit_after_a_recording_settles_exits_immediately() {
        // `Completed` and `Failed` are observable terminal states that
        // settle back to idle. Nothing is in flight in either, so
        // refusing would strand the process.
        assert_eq!(quit_decision(RecordingState::Completed), Quit::Now);
        assert_eq!(quit_decision(RecordingState::Failed), Quit::Now);
    }

    /// The exact race B-4 named: a recording that reaches a terminal
    /// state in the gap between the read that decided to defer and
    /// the flag going up must still be reported, not lost. Channel
    /// synchronized rather than sleep-based: the hook blocks
    /// `defer_quit` until a concurrent thread has driven the
    /// controller to `Failed` and run the check the transition
    /// observer would itself have run at that point, proving that
    /// check found nothing armed, exactly as it would in production,
    /// before letting `defer_quit` proceed to arm and recheck. Run
    /// many times because a race proven on one interleaving proves
    /// nothing about the next.
    #[test]
    fn test_a_recording_settling_between_the_read_and_the_arm_still_exits() {
        const ATTEMPTS: usize = 50;
        for attempt in 0..ATTEMPTS {
            let directory = tempfile::tempdir().unwrap();
            let application = ScrybeApplication::new(
                StorageRoot::new(directory.path()),
                directory.path().join("config.toml"),
            );
            let controller = Arc::clone(application.recording());
            controller.begin_preparing().unwrap();
            controller.mark_recording().unwrap();

            let pending = Arc::new(PendingQuit::default());

            let (go_tx, go_rx) = mpsc::channel::<()>();
            let (landed_tx, landed_rx) = mpsc::channel::<()>();

            let settler_controller = Arc::clone(&controller);
            let settler_pending = Arc::clone(&pending);
            let settler = std::thread::spawn(move || {
                go_rx.recv().unwrap();
                settler_controller
                    .fail("induced for the race test")
                    .unwrap();
                // Stands in for the transition observer's own call to
                // `exit_when_settled`, which at this exact point in a
                // real application is what runs. It must find nothing
                // armed yet, since `defer_quit` has not reached `arm`
                // yet; that is the no-op the bug relied on.
                let state = settler_controller.snapshot().state;
                let took_too_early = state.is_terminal() && settler_pending.take();
                let observer_message =
                    format!("attempt {attempt}: the observer must not have anything to take yet");
                assert!(!took_too_early, "{observer_message}");
                landed_tx.send(()).unwrap();
            });

            let settled = defer_quit(
                &controller,
                &pending,
                || {},
                || {
                    go_tx.send(()).unwrap();
                    landed_rx.recv().unwrap();
                },
            );

            settler.join().unwrap();

            let lost_message = format!(
                "attempt {attempt}: a recording that settled in the gap between the read and the arm must still be reported by this call, not lost"
            );
            assert_eq!(settled, Some(RecordingState::Failed), "{lost_message}");
        }
    }
}
