// Copyright 2026 Mathews Tom
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     https://www.apache.org/licenses/LICENSE-2.0

//! The complete command surface.
//!
//! Four reads. Every one forwards to a service method and converts the
//! result into a narrowed transport type; none of them interprets a
//! path, opens a file, or decides policy. [`COMMANDS`] is the list
//! `build.rs` generates access-control permissions from, so a command
//! added here without a matching capability entry is rejected at the
//! IPC boundary rather than silently reachable.

// Tauri resolves a command's managed state by value, so every command
// below takes `State` that way whether or not it consumes it.
#![allow(clippy::needless_pass_by_value)]

use scrybe_application::paging::PageRequest;
use scrybe_application::sessions::SearchRequest;
use scrybe_application::CancellationToken;
use tauri::State;

use crate::contract::{CommandFailure, RecordingStatus, SessionRows, SettingsSummary};
use crate::state::Desktop;

/// Every command the host registers.
///
/// `build.rs` turns each entry into an `allow-<command>` permission and
/// `tests/capability_audit.rs` proves the capability files and this list
/// describe the same surface.
pub const COMMANDS: &[&str] = &[
    "list_sessions",
    "search_sessions",
    "settings_summary",
    "recording_status",
];

/// Every command the host registers, reads and setup together.
///
/// The setup surface keeps its own list beside its own commands, so
/// adding one is an edit in one file rather than two; this is where the
/// two meet, and it is what `build.rs` and `tests/capability_audit.rs`
/// both measure the capability files against.
#[must_use]
pub fn all() -> Vec<&'static str> {
    COMMANDS
        .iter()
        .chain(crate::setup::COMMANDS.iter())
        .copied()
        .collect()
}

/// One page of the sessions under the configured root.
///
/// # Errors
///
/// Whatever the session repository reports: an unreadable or missing
/// storage root, or unreadable session metadata.
#[tauri::command]
pub fn list_sessions(
    desktop: State<'_, Desktop>,
    offset: usize,
    limit: usize,
) -> Result<SessionRows, CommandFailure> {
    desktop
        .application()
        .sessions()
        .list_sessions(PageRequest::new(offset, limit))
        .map(Into::into)
        .map_err(Into::into)
}

/// One page of the sessions matching `query`.
///
/// The token is fresh and never cancelled: cancelling a search from the
/// frontend needs a request identity the IPC boundary does not carry
/// yet, and inventing one here would be a policy this host has no
/// business owning. Paging already bounds the work.
///
/// # Errors
///
/// As `list_sessions`.
#[tauri::command]
pub fn search_sessions(
    desktop: State<'_, Desktop>,
    query: String,
    offset: usize,
    limit: usize,
) -> Result<SessionRows, CommandFailure> {
    let request = SearchRequest::new(query).at(PageRequest::new(offset, limit));
    desktop
        .application()
        .sessions()
        .search_sessions(&request, &CancellationToken::new())
        .map(Into::into)
        .map_err(Into::into)
}

/// The configuration, narrowed to what a read-only settings view shows.
///
/// # Errors
///
/// An unreadable configuration file, or one that is not well-formed.
#[tauri::command]
pub fn settings_summary(desktop: State<'_, Desktop>) -> Result<SettingsSummary, CommandFailure> {
    desktop
        .application()
        .config()
        .snapshot()
        .map(Into::into)
        .map_err(Into::into)
}

/// Where recording stands. Read-only: no transition can be requested
/// across this boundary.
#[must_use]
#[tauri::command]
pub fn recording_status(desktop: State<'_, Desktop>) -> RecordingStatus {
    desktop.application().recording().snapshot().into()
}
