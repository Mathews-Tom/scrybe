// Copyright 2026 Mathews Tom
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     https://www.apache.org/licenses/LICENSE-2.0

//! A debug-build control socket, so the qualification harness can drive
//! the real application.
//!
//! Apple ships no standalone `WKWebView` driver and no menu-bar
//! automation that does not require an Accessibility grant, so there is
//! no supported way to synthesize a tray click or a window close from
//! outside the process. This listener is the substitute: it accepts the
//! tray's own menu-item identities and the window verbs, and dispatches
//! them through exactly the functions the platform dispatches through.
//!
//! It is `#[cfg(debug_assertions)]` in full, and the socket lives
//! inside the configured storage root, which a hermetic run makes
//! disposable. A release build contains neither the listener nor its
//! verbs.

use std::io::{BufRead as _, BufReader};
use std::os::unix::net::UnixListener;
use std::path::{Path, PathBuf};

use tauri::Manager as _;

use crate::lifecycle::{tray, window};
use crate::state::Desktop;

/// The socket's name inside the storage root.
pub const FILE_NAME: &str = ".desktop-control.sock";

/// Close the main window the way its close button does.
const CLOSE_WINDOW: &str = "close-window";
/// Destroy the main window outright, so the next open must rebuild it.
const DESTROY_WINDOW: &str = "destroy-window";

/// Starts accepting control verbs, one per line, one per connection.
///
/// A failure to bind is reported and not propagated: it costs the
/// process its qualification channel, not its ability to run.
pub fn serve(app: &tauri::AppHandle) {
    let path = app
        .state::<Desktop>()
        .application()
        .root()
        .path()
        .join(FILE_NAME);
    let handle = app.clone();

    match bind(&path) {
        Ok(listener) => {
            std::thread::spawn(move || accept(&listener, &handle));
            crate::note!(app, "control-ready");
        }
        Err(error) => {
            eprintln!(
                "scrybe-desktop: could not open the control channel at {}: {error}",
                path.display()
            );
        }
    }
}

fn bind(path: &Path) -> std::io::Result<UnixListener> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    // A socket left behind by a previous run would otherwise make bind
    // fail for a process that is the rightful owner of this root.
    if path.exists() {
        std::fs::remove_file(path)?;
    }
    UnixListener::bind(path)
}

fn accept(listener: &UnixListener, app: &tauri::AppHandle) {
    for stream in listener.incoming() {
        let stream = match stream {
            Ok(stream) => stream,
            Err(error) => {
                eprintln!("scrybe-desktop: control channel accept failed: {error}");
                continue;
            }
        };
        for line in BufReader::new(stream).lines() {
            match line {
                Ok(verb) => dispatch(app, verb.trim().to_owned()),
                Err(error) => {
                    eprintln!("scrybe-desktop: control channel read failed: {error}");
                    break;
                }
            }
        }
    }
}

/// Runs `verb` on the main thread.
///
/// Window and tray work must happen there on macOS, and a control
/// channel that ran it anywhere else would be proving something the
/// real application never does.
fn dispatch(app: &tauri::AppHandle, verb: String) {
    let handle = app.clone();
    if let Err(error) = app.run_on_main_thread(move || run(&handle, &verb)) {
        eprintln!("scrybe-desktop: could not dispatch a control verb: {error}");
    }
}

fn run(app: &tauri::AppHandle, verb: &str) {
    match verb {
        CLOSE_WINDOW => close(app),
        DESTROY_WINDOW => destroy(app),
        // Everything else is a tray menu item identity, dispatched
        // through the same function the platform's menu event uses.
        item => tray::activate(app, item),
    }
}

/// Closes through the real close path: this raises `CloseRequested`, so
/// the hide-on-close handler runs exactly as it does for the window's
/// own close button.
fn close(app: &tauri::AppHandle) {
    let Some(main) = app.get_webview_window(window::MAIN) else {
        eprintln!("scrybe-desktop: no main window to close");
        return;
    };
    if let Err(error) = main.close() {
        eprintln!("scrybe-desktop: could not close the main window: {error}");
    }
}

/// Destroys without raising `CloseRequested`, leaving the process with
/// no main window so the next open has to rebuild one.
fn destroy(app: &tauri::AppHandle) {
    let Some(main) = app.get_webview_window(window::MAIN) else {
        eprintln!("scrybe-desktop: no main window to destroy");
        return;
    };
    if let Err(error) = main.destroy() {
        eprintln!("scrybe-desktop: could not destroy the main window: {error}");
    }
}

/// Where the socket is, for a caller that has the storage root.
#[must_use]
pub fn socket_path(root: &Path) -> PathBuf {
    root.join(FILE_NAME)
}
