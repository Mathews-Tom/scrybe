// Copyright 2026 Mathews Tom
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     https://www.apache.org/licenses/LICENSE-2.0

//! The desktop host.
//!
//! This crate owns windowing, the tray, single-instance activation, and
//! the translation between the `WebView`'s IPC and
//! `scrybe-application`'s typed contracts. It owns no domain policy: no
//! filesystem scan, no configuration parsing, no recording state
//! machine, and no provider selection lives here. Anything that looks
//! like one of those belongs in `scrybe-application` instead.
//!
//! It is a workspace of its own so the library and CLI gate never has to
//! build a `WebView`; the gate reaches this manifest with an explicit
//! `--manifest-path`.

#![deny(clippy::unwrap_used, clippy::expect_used)]

/// Starts the desktop application and blocks until it exits.
///
/// # Errors
///
/// Returns the Tauri build or run failure verbatim. The caller reports
/// it and exits non-zero: a host that cannot create its window has
/// nothing to fall back to, so failing loudly is the only honest
/// outcome.
pub fn run() -> tauri::Result<()> {
    tauri::Builder::default().run(tauri::generate_context!())
}
