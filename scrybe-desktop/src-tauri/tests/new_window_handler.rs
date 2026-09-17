// Copyright 2026 Mathews Tom
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     https://www.apache.org/licenses/LICENSE-2.0

//! The second exfiltration door, and the call that would open it.
//!
//! The navigation guard closes scripted top-level navigation, link
//! clicks, form submissions, and meta-refresh, because the platform
//! routes all of those through the navigation-policy delegate. A
//! script-initiated `window.open` does not go there: `WebKit` routes it
//! and a `target="_blank"` click to
//! `webView:createWebViewWithConfiguration:forNavigationAction:windowFeatures:`
//! on the *UI* delegate. `wry` implements that method as "call the
//! new-window handler if one was registered, otherwise answer `None`",
//! so today nothing opens — because of a call that is absent, not
//! because of a decision this application makes. The guard never sees
//! the request.
//!
//! The qualification harness drives both probes and asserts that no
//! second webview appeared, which catches a handler answering
//! `NewWindowResponse::Create`. It cannot catch one answering
//! `NewWindowResponse::Allow`: `wry` then builds the `NSWindow` and the
//! `WKWebView` itself and Tauri never learns of them, so they appear in
//! no webview list the harness can read. The absence of the
//! registration is therefore the only thing that covers both answers,
//! and asserting the absence is the only way to keep it deliberate.
//!
//! This is not a rule against ever having a detached window — a
//! playback window is the obvious next one. It is a rule that adding
//! the handler has to be done by editing this file, which is where the
//! argument for what the handler may return belongs.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::{Path, PathBuf};

/// The registration that opens the create-web-view path.
///
/// Spelled in pieces so that this file can never be mistaken for one of
/// the call sites it counts.
const REGISTRATION: &str = concat!(".on_new_", "window(");

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
fn test_the_host_registers_no_new_window_handler() {
    let mut sites = Vec::new();
    for path in sources() {
        let text = std::fs::read_to_string(&path).unwrap();
        for _ in 0..text.matches(REGISTRATION).count() {
            sites.push(path.display().to_string());
        }
    }

    assert!(
        sites.is_empty(),
        "\n`{REGISTRATION}` is what makes `window.open` and a \
         `target=\"_blank\"` click able to open anything at all: without it \
         `wry` answers the platform's create-web-view request with nothing. \
         The navigation guard does not see that request, and a handler \
         answering `NewWindowResponse::Allow` produces a window Tauri never \
         tracks, so the qualification harness cannot see it either. Adding \
         one is a trust-boundary change and belongs in review with the \
         argument for what it may return.\nfound: {sites:#?}\n",
    );
}
