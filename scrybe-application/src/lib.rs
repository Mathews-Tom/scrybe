// Copyright 2026 Mathews Tom
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     https://www.apache.org/licenses/LICENSE-2.0

//! # scrybe-application
//!
//! The user-facing use cases every Scrybe frontend shares: listing and
//! reading sessions, reading and updating configuration, diagnosing the
//! install, and driving a recording.
//!
//! This crate sits between `scrybe-core` — which owns capture, the
//! pipeline, storage primitives, and the Tier-1 contracts frozen in
//! `docs/system-design.md` §12.1 — and the frontends. It adapts those
//! contracts; it does not restate or replace them. The CLI and the
//! read-only agent surface consume the same services, so neither needs
//! its own filesystem scan, configuration policy, diagnostic rules, or
//! recording state model.
//!
//! Three properties hold across every service here:
//!
//! - **Root confinement.** A consumer supplies a [`SessionRef`], never
//!   a path. Absolute paths, separators, traversal, drive-relative and
//!   home-relative forms are refused at construction.
//! - **Serializable contracts.** Every request, response, and event
//!   type serializes, so a process boundary can be introduced later
//!   without reshaping them.
//! - **No presentation dependency.** Nothing here links a `WebView`,
//!   `AppKit`, `WinUI`, GTK, Compose, or `SwiftUI`. Rendering belongs
//!   to the frontend that owns it.

#![deny(clippy::unwrap_used, clippy::expect_used)]
#![allow(clippy::module_name_repetitions)]

pub mod config;
pub mod diagnostics;
pub mod error;
pub mod identity;
pub mod paging;
pub mod recording;
pub mod sessions;

pub use error::{ApplicationError, ErrorCode, ErrorPayload};
pub use identity::{IdentityRejection, PartialFileRef, SessionRef, StorageRoot};
pub use paging::{Page, PageRequest, DEFAULT_PAGE_LIMIT, MAX_PAGE_LIMIT};

/// Convenience alias for a fallible application-service call.
pub type Result<T> = std::result::Result<T, ApplicationError>;
