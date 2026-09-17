// Copyright 2026 Mathews Tom
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     https://www.apache.org/licenses/LICENSE-2.0

//! The one main window: hiding it, and getting it back.

use tauri::{Manager as _, WebviewWindow, WebviewWindowBuilder, WindowEvent};

/// The label every capability file scopes to, and the only window this
/// application creates.
pub const MAIN: &str = "main";

/// Hides the main window instead of destroying the process.
///
/// A persistent recorder whose process ended with its window would drop
/// an in-flight recording, so the close button hides and the tray is
/// how the window comes back.
pub fn hide_on_close(window: &tauri::Window, event: &WindowEvent) {
    if window.label() != MAIN {
        return;
    }
    let WindowEvent::CloseRequested { api, .. } = event else {
        return;
    };

    match window.hide() {
        Ok(()) => {
            api.prevent_close();
            crate::note!(&window.app_handle().clone(), "window-hidden");
        }
        // Letting the close proceed is the only remaining option, and
        // the tray can rebuild the window afterwards.
        Err(error) => eprintln!("scrybe-desktop: could not hide the main window: {error}"),
    }
}

/// Shows and focuses the main window, rebuilding it if it is gone.
///
/// Hiding is the normal path, so the window usually still exists. It
/// can still be destroyed — by the operating system, or by a close that
/// could not be prevented — and a tray whose `Open Scrybe` silently did
/// nothing would leave the application unreachable.
pub fn show(app: &tauri::AppHandle) {
    let window = match app.get_webview_window(MAIN) {
        Some(existing) => existing,
        None => match recreate(app) {
            Ok(rebuilt) => {
                crate::note!(app, "window-recreated");
                rebuilt
            }
            Err(error) => {
                eprintln!("scrybe-desktop: could not recreate the main window: {error}");
                return;
            }
        },
    };

    if let Err(error) = window.show().and_then(|()| window.set_focus()) {
        eprintln!("scrybe-desktop: could not focus the main window: {error}");
        return;
    }
    crate::note!(app, "window-shown");
}

/// Rebuilds the main window from the same configuration the first one
/// was built from, so a recreated window is not a second, divergent
/// definition of the first.
fn recreate(app: &tauri::AppHandle) -> tauri::Result<WebviewWindow> {
    let configured = app
        .config()
        .app
        .windows
        .iter()
        .find(|window| window.label == MAIN)
        .ok_or_else(|| tauri::Error::WindowNotFound)?
        .clone();

    WebviewWindowBuilder::from_config(app, &configured)?.build()
}
