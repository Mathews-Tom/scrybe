// Copyright 2026 Mathews Tom
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     https://www.apache.org/licenses/LICENSE-2.0

//! The one tray item the process owns for its whole life.

use scrybe_application::recording::{RecordingState, StopSource};
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::Manager as _;

use crate::lifecycle::{request_quit, window};

/// Identifies the tray item, so a second build attempt would fail
/// loudly rather than silently adding another icon to the menu bar.
pub const ID: &str = "main";

/// Menu item identities. They are also the verbs the debug control
/// channel accepts, so a qualification run drives the same dispatch the
/// platform drives rather than a parallel one written for testing.
pub const OPEN: &str = "open";
pub const RECORD: &str = "record";
pub const QUIT: &str = "quit";

/// The item a reader stops a recording from.
pub const STOP: &str = "stop";

/// Builds the tray item and wires its menu.
///
/// Both recording actions are present from the start and enabled or
/// disabled per state by [`reflect`], rather than appearing and
/// disappearing: a menu whose shape changes under the reader is harder
/// to aim at than one whose items grey out.
///
/// # Errors
///
/// A menu item, menu, or tray icon that cannot be created, or an
/// application with no default window icon to use. A process with no
/// tray cannot be reopened after its window is hidden, so this failure
/// is fatal to the caller rather than something to continue past.
pub fn build(app: &tauri::AppHandle) -> tauri::Result<()> {
    let record = MenuItem::with_id(app, RECORD, "Record now", true, None::<&str>)?;
    let stop = MenuItem::with_id(app, STOP, "Stop & save", false, None::<&str>)?;
    let open = MenuItem::with_id(app, OPEN, "Open Scrybe", true, None::<&str>)?;
    let separator = PredefinedMenuItem::separator(app)?;
    let quit = MenuItem::with_id(app, QUIT, "Quit Scrybe", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&record, &stop, &open, &separator, &quit])?;
    // Held so `reflect` can enable and disable them as the recording
    // state moves. Without this the tray would need to rebuild its menu
    // per transition, which on macOS closes an open menu under the
    // reader's cursor.
    app.manage(Items { record, stop });

    let icon = app.default_window_icon().cloned().ok_or_else(|| {
        tauri::Error::InvalidIcon(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "the bundle declares no default window icon to use for the tray",
        ))
    })?;

    TrayIconBuilder::with_id(ID)
        .icon(icon)
        .icon_as_template(true)
        .title("Scrybe")
        .tooltip("Scrybe")
        .menu(&menu)
        .show_menu_on_left_click(true)
        .build(app)?;

    crate::note!(
        app,
        "tray-ready",
        "record:enabled,stop:disabled,open:enabled,quit:enabled"
    );
    Ok(())
}

/// The two recording items, held for their lifetime.
struct Items {
    record: MenuItem<tauri::Wry>,
    stop: MenuItem<tauri::Wry>,
}

/// Enables and disables the recording items for `state`.
///
/// Called from the transition observer, which runs on whichever thread
/// drove the transition — including, for a stop from this very menu,
/// the main thread. `set_enabled` is a bounded `muda` call that takes
/// no application lock, so it cannot be the far side of a deadlock with
/// the controller's.
pub fn reflect(app: &tauri::AppHandle, state: RecordingState) {
    let Some(items) = app.try_state::<Items>() else {
        return;
    };
    // The same rule the controller states, read from the controller's
    // own contract rather than restated: a start is accepted only from
    // idle, and a stop only while preparing or recording.
    let _ = items.record.set_enabled(state.accepts_start());
    let _ = items.stop.set_enabled(state.accepts_stop());
}

/// Runs the action behind one menu item, wherever it came from.
///
/// The tray menu, the application menu, and the debug control channel
/// all arrive here, so there is one decision behind each verb rather
/// than one per entry point. The only step a qualification run cannot
/// drive is the platform delivering the click itself.
///
/// No handler is registered on [`TrayIconBuilder`], deliberately.
/// `TrayIcon::register` pushes such a handler into the same
/// `manager.menu.global_event_listeners` vector the builder's own
/// listeners are moved into, and the runtime calls every entry in that
/// vector for every menu event — Tauri documents this for the tray
/// builder, whose handler "is called for any menu event, whether it is
/// coming from this window, another window or from the tray icon
/// menu". A registration here as well as on the builder therefore did
/// not split the two menus apart; it ran this function twice for every
/// item either menu raised. `tests/menu_event_listener.rs` keeps the
/// count at one.
pub fn activate(app: &tauri::AppHandle, item: &str) {
    match item {
        OPEN => window::show(app),
        QUIT => request_quit(app),
        RECORD => start(app),
        STOP => {
            let _ = crate::recording::request_stop(app, StopSource::Tray);
        }
        other => eprintln!("scrybe-desktop: unhandled tray menu item `{other}`"),
    }
}

/// Starts a recording from the tray, with no title.
///
/// The tray has nowhere to type one, and the session names itself from
/// its start time either way; a reader who wants a title uses the
/// window.
fn start(app: &tauri::AppHandle) {
    // Dispatched rather than run here. This runs on the main thread —
    // the thread the `WebView` draws on — and a recording's preflight
    // stats the storage root and may enumerate a device. Holding the
    // drawing thread for that is a menu that stays open and a window
    // that stops responding.
    let handle = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        if let Err(error) = crate::recording::start(&handle, None) {
            eprintln!(
                "scrybe-desktop: the tray could not start a recording: {}",
                error.message
            );
        }
    });
}
