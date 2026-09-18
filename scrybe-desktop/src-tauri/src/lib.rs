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
pub mod playback;
pub mod queries;
pub mod recording;
pub mod retention;
pub mod setup;
pub mod state;

use std::sync::Arc;

use tauri::{Emitter, Manager};

use crate::contract::{RecordingTransition, TRANSITION_EVENT};
use crate::lifecycle::{hotkey, menu, navigation, signals, tray, window};
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
        // A policy cannot govern a top-level navigation, so the
        // content security policy alone left the frontend able to put
        // what it reads into a URL and leave. Registered as a plugin
        // because the main window is created from configuration before
        // any of this crate's code runs.
        .plugin(navigation::guard())
        // The one scheme this application registers, and the only way
        // an `<audio>` element can name a file under the storage root.
        // Registered as `playback::serve` itself rather than through a
        // closure, so what the tests drive is what the application
        // answers with.
        .register_uri_scheme_protocol(playback::SCHEME, playback::serve)
        .manage(desktop)
        // One acquisition at a time, so a cancel command can reach
        // whichever install is running without the frontend having to
        // carry a handle for it.
        .manage(std::sync::Arc::new(setup::ModelAcquisition::default()))
        // One recording at a time, so a stop from any surface reaches
        // whichever session is running without the frontend having to
        // carry a handle for it.
        .manage(std::sync::Arc::new(recording::LiveRecording::default()))
        // Whether a quit is waiting for a recording to become durable.
        // One flag for the process, so a quit asked for from the tray
        // and one asked for from the menu are the same deferred exit.
        .manage(lifecycle::PendingQuit::default())
        // Replaces the platform default, whose predefined quit item
        // terminates the process natively without reaching the
        // exit-request path. The item this installs carries the tray's
        // own identity, so the handler below runs one quit decision for
        // the menu, the tray, and the debug control channel alike.
        .menu(menu::build)
        // The process's only menu-event listener, for both menus. A
        // handler on `TrayIconBuilder` would not be a second channel:
        // it lands in the same global vector as this one and the
        // runtime calls every entry for every menu event, so the two
        // together ran each item twice. A predefined item never
        // arrives here — `muda` gives each one a native selector, so
        // the platform acts on it without raising a menu event.
        .on_menu_event(|app, event| tray::activate(app, event.id.as_ref()))
        .setup(|app| {
            let handle = app.handle();
            // Before the window exists, so it cannot race the first
            // listing: `commands::list_sessions` is reachable only from
            // a frontend, and no frontend is running yet. Synchronous
            // for the same reason — a spawned sweep could remove a
            // folder the reader is already looking at.
            retention::sweep_at_launch(handle);
            crate::note!(handle, "launched");
            forward_recording_transitions(handle);
            tray::build(handle)?;
            // On the main thread, as the platform requires: the hotkey
            // manager's event handler is keyed to the thread that
            // created it, and this closure is that thread. A failure is
            // reported and the process continues — a reader with no
            // accelerator still has the window, the tray and the menu,
            // and refusing to launch over a combination another
            // application happens to hold would be a worse trade.
            match hotkey::serve(handle) {
                Ok(listener) => hotkey::hold(listener),
                Err(error) => {
                    eprintln!(
                        "scrybe-desktop: the global stop accelerator is unavailable: {error}"
                    );
                    crate::note!(handle, "hotkey-unavailable");
                }
            }
            signals::serve(handle);
            #[cfg(debug_assertions)]
            menu::note_quit_identity(handle);
            #[cfg(debug_assertions)]
            lifecycle::control::serve(handle);
            // Through the same path the tray's `Open Scrybe` uses, so
            // the `window-shown` observation is recorded where a window
            // has actually been shown rather than asserted here. The
            // window is configured visible, so this shows an already
            // visible window; what it adds is that the record reflects
            // an outcome instead of an intention.
            window::show(handle);
            Ok(())
        })
        .on_window_event(window::hide_on_close)
        .invoke_handler(tauri::generate_handler![
            commands::list_sessions,
            commands::search_sessions,
            commands::cancel_query,
            commands::get_session,
            commands::read_notes,
            commands::read_transcript_page,
            commands::repair_session,
            commands::regenerate_notes,
            commands::reveal_session,
            commands::delete_session,
            commands::archive_session,
            commands::copy_notes,
            commands::copy_transcript,
            commands::settings_summary,
            commands::recording_status,
            commands::recording_preflight,
            recording::start_recording,
            recording::stop_recording,
            recording::acknowledge_recording,
            setup::settings_form,
            setup::apply_settings,
            setup::diagnostics_report,
            setup::apply_recovery,
            setup::readiness_report,
            setup::model_offer,
            setup::install_model,
            setup::cancel_model_install,
            setup::open_system_settings,
            setup::open_advanced_configuration,
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
            // Both of these are bounded and take no lock the controller
            // could be waiting on: enabling a `muda` menu item, and
            // reading one atomic. That matters because this observer
            // runs on whichever thread drove the transition, which for
            // a stop from the tray is the main thread — the one AppKit
            // requires and the one the `WebView` draws on. A blocking
            // call here against anything the controller holds would
            // present as a hang with no diagnosis.
            tray::reflect(&handle, event.to);
            // One observation per transition, so a qualification run
            // reads the transitions the application actually made
            // rather than inferring them from artifacts. The detail is
            // the two state labels and the surface that asked, all of
            // which are enumerated constants — no path, no provider, no
            // device, and nothing a meeting said.
            crate::note!(
                &handle,
                "recording-transition",
                &format!(
                    "{}->{}{}",
                    lifecycle::state_label(event.from),
                    lifecycle::state_label(event.to),
                    event
                        .stop_source
                        .map(|source| format!(",{}", source.label()))
                        .unwrap_or_default()
                )
            );
            lifecycle::exit_when_settled(&handle, event.to);
        }));
}
