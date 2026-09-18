// Copyright 2026 Mathews Tom
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     https://www.apache.org/licenses/LICENSE-2.0

//! The native macOS surfaces a running recording is shown on.
//!
//! The floating panel, the global hotkey, and the arithmetic a
//! status-bar indicator draws from. They exist as a crate of their own
//! for a reason that is not organisational:
//!
//! - they were `mod` items private to the `scrybe` binary, which
//!   declares no `[lib]`, so no other crate could reach them at all;
//! - and `scrybe-desktop/src-tauri` sets `unsafe_code = "forbid"`,
//!   which a local `#![allow]` cannot override. The panel is seven
//!   `msg_send!` blocks under exactly such an allow, legal only because
//!   the root workspace chose `deny`. So the `AppKit` code can never be
//!   inlined into the desktop host, whatever happens to the
//!   command-line binary's target list.
//!
//! Adding a `[lib]` to `scrybe-cli` was considered and rejected: it
//! publishes to crates.io as `scrybe`, so every newly-`pub` module here
//! would become a `SemVer` commitment on a package whose purpose is a
//! command-line tool, and it would make one frontend depend on a
//! sibling frontend, which nothing in this workspace does.
//!
//! Nothing here depends on the service layer. A widget is handed a
//! [`ShellView`] and hands back a stop request; whichever frontend owns
//! the recording is what turns one into the other.

#[cfg(target_os = "macos")]
pub mod floating_panel;
pub mod hotkey;
pub mod status;
mod view;

pub use view::{ShellState, ShellView};
