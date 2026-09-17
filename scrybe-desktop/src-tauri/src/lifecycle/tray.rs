// Copyright 2026 Mathews Tom
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     https://www.apache.org/licenses/LICENSE-2.0

//! The one tray item the process owns for its whole life.

use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::TrayIconBuilder;

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

/// `Record now` stays disabled until recording control exists. The same
/// constant feeds the menu item and the lifecycle record, so the
/// recorded state cannot disagree with the rendered one.
const RECORD_ENABLED: bool = false;

/// Builds the tray item and wires its menu.
///
/// `Record now` is present and disabled. Recording control belongs to
/// the work that adds it; showing the action as not yet available is
/// honest, and hiding it would make the tray's shape change under the
/// user when it arrives.
///
/// # Errors
///
/// A menu item, menu, or tray icon that cannot be created, or an
/// application with no default window icon to use. A process with no
/// tray cannot be reopened after its window is hidden, so this failure
/// is fatal to the caller rather than something to continue past.
pub fn build(app: &tauri::AppHandle) -> tauri::Result<()> {
    let record = MenuItem::with_id(app, RECORD, "Record now", RECORD_ENABLED, None::<&str>)?;
    let open = MenuItem::with_id(app, OPEN, "Open Scrybe", true, None::<&str>)?;
    let separator = PredefinedMenuItem::separator(app)?;
    let quit = MenuItem::with_id(app, QUIT, "Quit Scrybe", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&record, &open, &separator, &quit])?;

    let icon = app.default_window_icon().cloned().ok_or_else(|| {
        tauri::Error::InvalidIcon(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "the bundle declares no default window icon to use for the tray",
        ))
    })?;

    TrayIconBuilder::with_id(ID)
        .icon(icon)
        .icon_as_template(true)
        .tooltip("Scrybe")
        .menu(&menu)
        .show_menu_on_left_click(true)
        .on_menu_event(|app, event| activate(app, event.id.as_ref()))
        .build(app)?;

    crate::note!(
        app,
        "tray-ready",
        if RECORD_ENABLED {
            "record:enabled,open:enabled,quit:enabled"
        } else {
            "record:disabled,open:enabled,quit:enabled"
        }
    );
    Ok(())
}

/// Runs the action behind one menu item.
///
/// Separate from the menu wiring so the debug control channel reaches
/// the same dispatch. The only step a qualification run cannot drive is
/// the platform delivering the click itself.
pub fn activate(app: &tauri::AppHandle, item: &str) {
    match item {
        OPEN => window::show(app),
        QUIT => request_quit(app),
        // Disabled, so the platform never delivers it. Reachable only
        // through the debug control channel, where saying so is more
        // useful than silence.
        RECORD => eprintln!("scrybe-desktop: `Record now` is not available yet"),
        other => eprintln!("scrybe-desktop: unhandled tray menu item `{other}`"),
    }
}
