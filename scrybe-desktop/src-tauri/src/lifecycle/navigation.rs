// Copyright 2026 Mathews Tom
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     https://www.apache.org/licenses/LICENSE-2.0

//! Where the `WebView` is allowed to go.
//!
//! The content security policy closes fetch, `XHR`, `WebSocket`,
//! beacon, and iframe, but a policy has no control over a top-level
//! navigation, and `form-action` governs only form submission. Frontend
//! code that could reach the session titles and the configuration paths
//! could still put them in a URL and set `location.href`, and nothing
//! in the host stopped it: the windows are built from configuration
//! with no handler attached.
//!
//! This registers the handler as a plugin rather than on a window
//! builder, because the main window is created by Tauri from
//! `tauri.conf.json` before any of this crate's code runs, and a plugin
//! hook applies to every webview in the process including that one.

use tauri::plugin::{Builder, TauriPlugin};
use tauri::{Manager as _, Url};

/// The origin the bundled application is served from.
const APPLICATION_HOST: &str = "localhost";

/// Refuses every navigation away from the application's own origin.
///
/// A refusal is recorded, because the refusal is the only externally
/// visible consequence of this handler existing. The navigation it
/// stops does not change the window's URL, and neither does a
/// navigation that is allowed but cannot be reached, so a qualification
/// run comparing URLs before and after could not tell the two apart.
#[must_use]
pub fn guard() -> TauriPlugin<tauri::Wry> {
    Builder::new("scrybe-navigation")
        .on_navigation(|webview, url| {
            let allowed = permitted(url);
            if !allowed {
                crate::note!(webview.app_handle(), "navigation-refused", url.as_str());
            }
            allowed
        })
        .build()
}

/// Whether the `WebView` may navigate to `url`.
///
/// `tauri://localhost` is the bundled application. `http://localhost`
/// is the development server, which only a debug build ever points at;
/// allowing it in a release build would reopen the hole for anything
/// that could reach a local port.
#[must_use]
pub fn permitted(url: &Url) -> bool {
    match url.scheme() {
        "tauri" => url.host_str() == Some(APPLICATION_HOST),
        "http" if cfg!(debug_assertions) => url.host_str() == Some(APPLICATION_HOST),
        _ => false,
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    fn url(text: &str) -> Url {
        Url::parse(text).unwrap()
    }

    #[test]
    fn test_the_application_origin_is_reachable() {
        assert!(permitted(&url("tauri://localhost/index.html")));
    }

    #[test]
    fn test_a_remote_origin_is_refused() {
        // The exfiltration path this closes: frontend code execution,
        // then the session titles and configuration paths in a URL, then
        // a top-level navigation the policy cannot see.
        assert!(!permitted(&url("https://example.invalid/?titles=one,two")));
        assert!(!permitted(&url("http://example.invalid/")));
        assert!(!permitted(&url("tauri://example.invalid/")));
    }

    #[test]
    fn test_a_scheme_that_is_not_a_web_origin_is_refused() {
        // `file:` would read the storage root the settings view just
        // disclosed the path of; the others are code execution and a
        // hand-off to another application.
        assert!(!permitted(&url("file:///etc/passwd")));
        assert!(!permitted(&url("javascript:alert(1)")));
        assert!(!permitted(&url("data:text/html,<script>fetch(1)</script>")));
        assert!(!permitted(&url("mailto:someone@example.invalid")));
    }
}
