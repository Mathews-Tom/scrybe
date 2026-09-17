// Copyright 2026 Mathews Tom
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     https://www.apache.org/licenses/LICENSE-2.0

//! One quit is one decision, because one listener hears the event.
//!
//! `TrayIconBuilder::on_menu_event` is not a second channel.
//! `TrayIcon::register` pushes its handler into the same
//! `manager.menu.global_event_listeners` vector that the builder's own
//! listeners are moved into, and the runtime's menu-event arm calls
//! every entry in that vector for every menu event, whichever menu it
//! came from — Tauri's documentation for the tray builder says the
//! handler "is called for any menu event, whether it is coming from
//! this window, another window or from the tray icon menu".
//!
//! Both custom quit items carry the tray's identity, so with a
//! registration in each place one quit ran the decision twice: two
//! `quit-accepted` records and two `exit(0)` calls while idle, or two
//! refusals and two duplicate stderr lines with a recording in flight.
//! The headline this module's parent makes — one decision, three entry
//! points — was false in the shipped tree.
//!
//! The qualification harness cannot see it, because it drives quit over
//! the control socket, which calls the activation directly rather than
//! through a menu event. The registration count is not reachable
//! through any public API either, and `muda` refuses to build a menu
//! off the main thread, so this reads the source the way
//! `capability_audit.rs` reads `build.rs`. It is what stands between a
//! future edit and a second listener reintroduced in silence.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::{Path, PathBuf};

/// The registration this crate is allowed to make exactly once.
///
/// Spelled in pieces so that this file, which is not under `src/`, can
/// never be mistaken for one of the call sites it counts even if it is
/// moved there.
const REGISTRATION: &str = concat!(".on_", "menu_event(");

/// Every Rust source file the host crate compiles, at any depth.
fn sources() -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut pending = vec![Path::new(env!("CARGO_MANIFEST_DIR")).join("src")];
    while let Some(directory) = pending.pop() {
        for entry in std::fs::read_dir(&directory).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                pending.push(path);
            } else if path.extension().is_some_and(|extension| extension == "rs") {
                found.push(path);
            }
        }
    }
    found.sort();
    found
}

#[test]
fn test_the_host_registers_exactly_one_global_menu_event_listener() {
    let mut sites = Vec::new();
    for path in sources() {
        let text = std::fs::read_to_string(&path).unwrap();
        for _ in 0..text.matches(REGISTRATION).count() {
            sites.push(path.display().to_string());
        }
    }

    assert_eq!(
        sites.len(),
        1,
        "\nevery `{REGISTRATION}` in this crate pushes onto the one \
         `manager.menu.global_event_listeners` vector, and the runtime \
         calls every entry in it for every menu event. {} registrations \
         means one quit runs the quit decision {} times.\nfound: {sites:#?}\n",
        sites.len(),
        sites.len(),
    );
}
