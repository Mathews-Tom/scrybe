// Copyright 2026 Mathews Tom
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     https://www.apache.org/licenses/LICENSE-2.0

//! The global accelerator a recording can be stopped from without
//! reaching the window.
//!
//! The listener itself is `scrybe-widgets`' — the same one the
//! command-line shell registers — so the accelerator grammar, the
//! default, and the platform binding are one implementation rather than
//! two that could disagree about what `CmdOrCtrl+Shift+R` means.
//!
//! **Main-thread affinity.** `GlobalHotKeyManager` is `!Send` on macOS:
//! its Carbon event handler is keyed to the thread that created it, and
//! that must be the thread running the platform loop. So the listener
//! is built in Tauri's `setup`, which runs on the main thread, and then
//! held there for the process's life. What is *not* done on the main
//! thread is anything that could block: the press is delivered on
//! `global-hotkey`'s own global crossbeam channel, which a background
//! thread drains, and the stop request runs there. The main thread
//! never takes the recording controller's lock, so there is no pair of
//! locks for it to deadlock against.

use scrybe_application::recording::StopSource;
use scrybe_widgets::hotkey::{HotkeyEvent, HotkeyListener, Presses, DEFAULT_STOP_ACCELERATOR};
use tauri::Manager as _;

use crate::state::Desktop;

/// How long the draining thread waits for a press before looking again.
///
/// A blocking receive with a timeout rather than a spin: the thread is
/// asleep between presses, and the timeout is only what lets it notice
/// the process is going away.
const POLL: std::time::Duration = std::time::Duration::from_millis(200);

/// Registers the configured accelerator and drains its presses.
///
/// Returns the listener, which the caller must keep alive: dropping it
/// unregisters the accelerator from the platform.
///
/// # Errors
///
/// A malformed accelerator, or one the platform refused to register —
/// most often because another application already holds it.
pub fn serve(app: &tauri::AppHandle) -> anyhow::Result<HotkeyListener> {
    let accelerator = app
        .state::<Desktop>()
        .application()
        .config()
        .load()
        .ok()
        .and_then(|config| config.capture.hotkey)
        .unwrap_or_else(|| DEFAULT_STOP_ACCELERATOR.to_string());

    let listener = HotkeyListener::start(&accelerator)?;
    let presses = listener.presses();
    crate::note!(app, "hotkey-registered", accelerator.as_str());

    let handle = app.clone();
    std::thread::spawn(move || drain(&handle, &presses));
    Ok(listener)
}

/// Requests one stop per press, until the application goes away.
fn drain(app: &tauri::AppHandle, presses: &Presses) {
    while app.try_state::<Desktop>().is_some() {
        if presses.poll_for(POLL) == Some(HotkeyEvent::StopRequested) {
            // Through the one route every surface uses, so a press
            // racing the tray, the window, or a signal cannot produce a
            // second teardown: the controller decides, under one lock,
            // which request counts. This runs on this thread, never on
            // the main one, so taking that lock cannot block the
            // platform loop the listener is registered on.
            let _ = crate::recording::request_stop(app, StopSource::Hotkey);
        }
    }
}

/// Keeps the listener alive for the process's life.
///
/// `HotkeyListener` is `!Send`, so it cannot be Tauri-managed state and
/// cannot cross a thread. It is held on the main thread for the
/// process's lifetime instead, which is also where it was registered.
/// Its `Drop` unregisters the accelerator; leaking it deliberately is
/// what keeps the accelerator alive for as long as the process is.
pub const fn hold(listener: HotkeyListener) {
    std::mem::forget(listener);
}
