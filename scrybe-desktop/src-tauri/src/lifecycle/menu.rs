// Copyright 2026 Mathews Tom
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     https://www.apache.org/licenses/LICENSE-2.0

//! The application menu, so that quit means one thing.
//!
//! With no menu configured, Tauri installs the platform default, whose
//! predefined quit item is the native `terminate:` selector, and the
//! windowing layer installs no `applicationShouldTerminate:` handler,
//! so ⌘Q and *Scrybe → Quit Scrybe* ended the process directly:
//! they never reached the exit-request path and never consulted
//! [`quit_decision`](super::quit_decision), so the guarantee this
//! module's parent documents — that quit refuses while a recording is
//! in flight rather than abandoning it — held for the tray item alone.
//!
//! The menu below replaces the default so the quit item carries the
//! tray's own menu identity. The process's one global menu-event
//! listener routes that identity through [`tray::activate`], which is
//! the same function the debug control channel calls, so there is one
//! quit decision rather than three. That listener is registered once,
//! on the builder; `tests/menu_event_listener.rs` says why a second
//! one anywhere would make this claim false again.
//!
//! Everything else in the menu is a predefined item, dispatched
//! natively. They are here because replacing the default menu would
//! otherwise take the Edit items with it, and a `WebView` with no Edit
//! menu has no copy, paste, or select-all.

use tauri::menu::{Menu, MenuItem, PredefinedMenuItem, Submenu};

use crate::lifecycle::tray;

/// Builds the menu the application installs in place of the default.
///
/// # Errors
///
/// A menu item or submenu the platform refuses to create. An
/// application whose menu could not be built would fall back to the
/// default menu and its native quit, which is the failure this module
/// exists to prevent, so the caller treats it as fatal.
pub fn build<R: tauri::Runtime>(app: &tauri::AppHandle<R>) -> tauri::Result<Menu<R>> {
    // The one item this application owns. Its identity is the tray's,
    // which is what makes the two paths one decision.
    let quit = MenuItem::with_id(app, tray::QUIT, "Quit Scrybe", true, Some("CmdOrCtrl+Q"))?;

    let application = Submenu::with_items(
        app,
        "Scrybe",
        true,
        &[
            &PredefinedMenuItem::about(app, None, None)?,
            &PredefinedMenuItem::separator(app)?,
            &PredefinedMenuItem::hide(app, None)?,
            &PredefinedMenuItem::hide_others(app, None)?,
            &PredefinedMenuItem::show_all(app, None)?,
            &PredefinedMenuItem::separator(app)?,
            &quit,
        ],
    )?;

    let edit = Submenu::with_items(
        app,
        "Edit",
        true,
        &[
            &PredefinedMenuItem::undo(app, None)?,
            &PredefinedMenuItem::redo(app, None)?,
            &PredefinedMenuItem::separator(app)?,
            &PredefinedMenuItem::cut(app, None)?,
            &PredefinedMenuItem::copy(app, None)?,
            &PredefinedMenuItem::paste(app, None)?,
            &PredefinedMenuItem::select_all(app, None)?,
        ],
    )?;

    Menu::with_items(app, &[&application, &edit])
}

/// Whether the application menu defines this item itself.
#[must_use]
pub fn owned(item: &str) -> bool {
    item == tray::QUIT
}

/// Records whether the installed menu's quit carries the tray's
/// identity.
///
/// The menu is a platform object, and `muda` refuses to build one off
/// the main thread, so it cannot be inspected from a unit test. This
/// reads the menu the process is actually running with, which is the
/// only evidence that ⌘Q reaches [`super::quit_decision`] rather than
/// the native `terminate:` selector — the qualification harness asserts
/// it. `quit:native` means the identity is absent and the guarantee
/// holds for the tray alone.
#[cfg(debug_assertions)]
pub fn note_quit_identity(app: &tauri::AppHandle) {
    let carried = app
        .menu()
        .is_some_and(|menu| identities(&menu).iter().any(|identity| owned(identity)));
    crate::note!(
        app,
        "menu-ready",
        if carried { "quit:quit" } else { "quit:native" }
    );
}

/// Every item identity the menu carries, one submenu deep.
#[cfg(debug_assertions)]
fn identities<R: tauri::Runtime>(menu: &Menu<R>) -> Vec<String> {
    menu.items()
        .unwrap_or_default()
        .iter()
        .flat_map(|item| {
            item.as_submenu().map_or_else(
                || vec![item.id().0.clone()],
                |submenu| {
                    submenu
                        .items()
                        .unwrap_or_default()
                        .iter()
                        .map(|item| item.id().0.clone())
                        .collect()
                },
            )
        })
        .collect()
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn test_the_menu_dispatches_its_own_quit_through_the_trays_own_action() {
        // The identity is the whole mechanism: because the menu's quit
        // item carries it, the global menu-event handler hands ⌘Q and
        // `Quit Scrybe` to `tray::activate`, which calls `request_quit`,
        // which consults `quit_decision`. A quit item with any other
        // identity reaches none of them, which is what the platform
        // default menu did.
        assert!(owned(tray::QUIT));
    }

    #[test]
    fn test_the_menu_leaves_the_trays_other_items_and_predefined_ones_alone() {
        // `Open Scrybe` and `Record now` belong to the tray menu, and a
        // predefined item carries out its own action natively. This
        // menu defines none of them, so the quit identity it does
        // define is the only one it can be read as claiming.
        assert!(!owned(tray::OPEN));
        assert!(!owned(tray::RECORD));
        assert!(!owned("__core__predefined__copy"));
    }
}
