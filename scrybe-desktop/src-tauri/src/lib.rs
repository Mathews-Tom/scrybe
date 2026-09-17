// Copyright 2026 Mathews Tom
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     https://www.apache.org/licenses/LICENSE-2.0

//! The desktop host.
//!
//! This crate owns windowing, the tray, single-instance activation, and
//! the translation between the `WebView`'s IPC and
//! `scrybe-application`'s typed contracts. It owns no domain policy: no
//! filesystem scan, no configuration parsing, no recording state
//! machine, and no provider selection lives here. Anything that looks
//! like one of those belongs in `scrybe-application` instead.
//!
//! It is a workspace of its own so the library and CLI gate never has to
//! build a `WebView`; the gate reaches this manifest with an explicit
//! `--manifest-path`.

#![deny(clippy::unwrap_used, clippy::expect_used)]

pub mod commands;
pub mod contract;
pub mod lifecycle;
pub mod state;

use std::sync::Arc;

use tauri::{Emitter, Manager};

use crate::contract::{RecordingTransition, TRANSITION_EVENT};
use crate::lifecycle::{tray, window};
use crate::state::Desktop;

/// Starts the desktop application and blocks until it exits.
///
/// # Errors
///
/// A configuration that cannot be resolved, or a Tauri build or run
/// failure. A host that cannot resolve its own storage root or create
/// its window has nothing to fall back to, so the caller reports the
/// failure and exits non-zero.
pub fn run() -> Result<(), Box<dyn std::error::Error>> {
    let desktop = Desktop::discover()?;

    tauri::Builder::default()
        // Registered first, as the plugin requires: a second launch
        // must be turned away before it can build a window, a tray, or
        // a second view of the storage root.
        .plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| {
            crate::note!(app, "second-launch-activated");
            window::show(app);
        }))
        .manage(desktop)
        .setup(|app| {
            let handle = app.handle();
            crate::note!(handle, "launched");
            forward_recording_transitions(handle);
            tray::build(handle)?;
            #[cfg(debug_assertions)]
            lifecycle::control::serve(handle);
            crate::note!(handle, "window-shown");
            Ok(())
        })
        .on_window_event(window::hide_on_close)
        .invoke_handler(tauri::generate_handler![
            commands::list_sessions,
            commands::search_sessions,
            commands::settings_summary,
            commands::recording_status,
        ])
        .build(tauri::generate_context!())?
        .run(lifecycle::keep_running_without_a_window);

    Ok(())
}

/// Republishes the service layer's recording transitions as a window
/// event.
///
/// The observer runs on whichever thread drove the transition, so it
/// does the least possible work: narrow the event and hand it to
/// Tauri's emitter. A failed emit is reported rather than swallowed —
/// an observer cannot propagate, and a status display silently frozen
/// on a stale state is worse than a line on stderr.
fn forward_recording_transitions(app: &tauri::AppHandle) {
    let handle = app.clone();
    app.state::<Desktop>()
        .application()
        .recording()
        .subscribe(Arc::new(move |event| {
            let transition = RecordingTransition::from(event);
            if let Err(error) = handle.emit(TRANSITION_EVENT, transition) {
                eprintln!("scrybe-desktop: could not emit a recording transition: {error}");
            }
        }));
}
