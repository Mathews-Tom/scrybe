// Copyright 2026 Mathews Tom
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     https://www.apache.org/licenses/LICENSE-2.0

//! Terminating the process without losing the recording.
//!
//! A `SIGINT` or `SIGTERM` reaches this host the same way it reaches
//! the command-line recorder — a reader who started it from a terminal,
//! a supervisor shutting the session down, a `killall`. The default
//! disposition ends the process immediately, which during a recording
//! would leave a session folder needing repair.
//!
//! So the first signal is taken to mean what a quit means: stop the
//! recording and leave once it is durable. The **second** is not
//! intercepted. A reader who signals twice has decided they want out
//! now, and a recorder that could not be terminated would be a worse
//! failure than a session that has to be repaired — which is what the
//! journal under the session folder exists for.

use scrybe_application::recording::StopSource;
use tauri::Manager as _;

use crate::state::Desktop;

/// Installs the handlers for the whole process.
///
/// Both are restored to their default disposition after one delivery,
/// so the second signal terminates as the platform would.
pub fn serve(app: &tauri::AppHandle) {
    let handle = app.clone();
    tauri::async_runtime::spawn(async move {
        let Ok(mut interrupt) =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt())
        else {
            eprintln!("scrybe-desktop: SIGINT could not be handled; the default applies");
            return;
        };
        let Ok(mut terminate) =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        else {
            eprintln!("scrybe-desktop: SIGTERM could not be handled; the default applies");
            return;
        };
        tokio::select! {
            _ = interrupt.recv() => received(&handle),
            _ = terminate.recv() => received(&handle),
        }
    });
}

/// Treats one signal as a quit request.
///
/// Runs on the runtime rather than in a signal handler: everything
/// below takes the recording controller's lock, and a handler that did
/// so would be taking a lock in a context where the thread it
/// interrupted might already hold it.
fn received(app: &tauri::AppHandle) {
    if app.try_state::<Desktop>().is_none() {
        return;
    }
    crate::note!(app, "signal-received");
    // The one route every surface uses. Labelled `Signal` rather than
    // `Termination` so the record says which surface asked, and
    // idempotent against a stop the reader already requested.
    let _ = crate::recording::request_stop(app, StopSource::Signal);
    super::request_quit(app);
}
