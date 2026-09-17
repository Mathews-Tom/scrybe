// Copyright 2026 Mathews Tom
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     https://www.apache.org/licenses/LICENSE-2.0

//! The replacement menu still carries the standard items.
//!
//! Installing a custom application menu replaces Tauri's default
//! outright, and that default is where the platform's own items come
//! from: the application submenu with Services, Edit, a View submenu
//! with Full Screen, and a Window submenu with Minimize, Zoom, and
//! Close Window. Carrying only Edit forward left ⌘W, ⌘M, and Full
//! Screen as keys that do nothing — ⌘W most importantly, since closing
//! the window is the documented way to dismiss it, it hides rather than
//! destroys, and the tray brings it back.
//!
//! A `Menu` is a platform object that `muda` refuses to build off the
//! main thread, so the built menu cannot be inspected from a test
//! process. This reads the source that builds it, the way
//! `capability_audit.rs` reads `build.rs`, so that dropping an item
//! again has to be done in the open.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::Path;

/// What the menu must still construct, and why each one is there.
const REQUIRED: &[(&str, &str)] = &[
    (
        "PredefinedMenuItem::services(",
        "Services, in the application submenu",
    ),
    ("PredefinedMenuItem::fullscreen(", "Full Screen, under View"),
    (
        "PredefinedMenuItem::minimize(",
        "Minimize (⌘M), under Window",
    ),
    ("PredefinedMenuItem::maximize(", "Zoom, under Window"),
    (
        "PredefinedMenuItem::close_window(",
        "Close Window (⌘W), under Window — the keyboard equivalent of the \
         close button, which hides the window the tray reopens",
    ),
    ("PredefinedMenuItem::undo(", "Undo, under Edit"),
    ("PredefinedMenuItem::redo(", "Redo, under Edit"),
    ("PredefinedMenuItem::cut(", "Cut, under Edit"),
    ("PredefinedMenuItem::copy(", "Copy, under Edit"),
    ("PredefinedMenuItem::paste(", "Paste, under Edit"),
    ("PredefinedMenuItem::select_all(", "Select All, under Edit"),
    (
        "AboutMetadata",
        "the name, version, and copyright the About panel shows; \
         `about(app, None, None)` renders a panel carrying none of them",
    ),
    ("\"View\"", "the View submenu itself"),
    ("\"Window\"", "the Window submenu itself"),
];

#[test]
fn test_the_replacement_menu_carries_the_items_the_default_would_have() {
    let source = std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("src/lifecycle/menu.rs"),
    )
    .unwrap();

    let missing: Vec<&(&str, &str)> = REQUIRED
        .iter()
        .filter(|(needle, _)| !source.contains(needle))
        .collect();

    assert!(
        missing.is_empty(),
        "\nthe application menu replaces the platform default, so an item \
         it does not build is an item the application does not have. \
         Missing:\n{}\n",
        missing
            .iter()
            .map(|(needle, why)| format!("  {needle} — {why}"))
            .collect::<Vec<_>>()
            .join("\n"),
    );
}
