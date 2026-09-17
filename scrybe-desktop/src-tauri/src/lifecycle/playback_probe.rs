// Copyright 2026 Mathews Tom
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     https://www.apache.org/licenses/LICENSE-2.0

//! Driving the playback scheme from the qualification harness.
//!
//! Whether `scrybe-audio://` works is not a fact about this process. It
//! is a fact about the `WebView`: the content security policy decides
//! whether a media element may load the URL at all, and a policy that
//! names the scheme under the wrong directive blocks the load without
//! the handler ever being asked. Nothing in Rust can observe that, and
//! the check that compares the shipped policy against a constant cannot
//! either — it passes for whatever string is written in both places.
//!
//! So this asks the real `WebView` to load a real URL on the scheme,
//! through the same media load the player performs, and the handler
//! records what it served. A run that sees a `playback-served` record
//! saw the webview reach the scheme. A run that sees only
//! `playback-requested` saw the policy stop it.
//!
//! `#[cfg(debug_assertions)]` in full, like the channel it is reached
//! through. `scripts/qualify-desktop-app.py` asserts the verb name
//! below is absent from a release binary.

use tauri::Manager as _;

use crate::lifecycle::window;

// The `probe-` prefix is not decoration; see `model_probe`. A verb
// named `playback` would appear in every release build through the
// generated `allow-…` permission names and the scheme's own path
// segment, and the compiled-out check would fail against a binary that
// is in fact clean.
/// Ask the main webview to load one session's playback audio.
pub const PLAY: &str = "probe-playback";

/// Whether `verb` is this module's, and runs it if so.
#[must_use]
pub fn run(app: &tauri::AppHandle, verb: &str) -> bool {
    let mut words = verb.split_whitespace();
    match (words.next(), words.next()) {
        (Some(PLAY), Some(id)) => {
            play(app, id);
            true
        }
        _ => false,
    }
}

fn play(app: &tauri::AppHandle, id: &str) {
    let Some(main) = app.get_webview_window(window::MAIN) else {
        eprintln!("scrybe-desktop: no main window to play through");
        return;
    };
    let url = format!("{}://localhost/{id}/playback", crate::playback::SCHEME);
    crate::note!(app, "playback-requested", &url);
    // `new Audio(url)` and the panel's `<audio src>` are the same load
    // as far as the policy is concerned: both are a media fetch, and
    // `media-src` is what governs both. Driving it from here rather
    // than through the interface is what lets a run reach the scheme
    // without first having to steer a reader's way through the list.
    let script = format!(
        "(() => {{ const audio = new Audio({url:?}); audio.preload = 'auto'; \
         globalThis.__scrybePlaybackProbe = audio; audio.load(); }})()"
    );
    if let Err(error) = main.eval(script) {
        eprintln!("scrybe-desktop: could not drive a playback load: {error}");
    }
}
