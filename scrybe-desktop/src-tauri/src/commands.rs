// Copyright 2026 Mathews Tom
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     https://www.apache.org/licenses/LICENSE-2.0

//! The complete command surface.
//!
//! Reads, the cancellation that governs one of them, and the two
//! explicit mutations a reader can ask for. Every one forwards to a
//! service method and converts the result into a narrowed transport
//! type; none of them interprets a path, opens a file, or decides
//! policy. [`COMMANDS`] is the list `build.rs` generates
//! access-control permissions from, so a command added here without a
//! matching capability entry is rejected at the IPC boundary rather
//! than silently reachable.
//!
//! The two commands that walk the storage root are marked
//! `#[tauri::command(async)]`. A command without it runs on the main
//! thread, which on this platform is also the thread the `WebView`
//! draws on: a listing or a search over a cold cache classifies every
//! folder under the root, and doing that on the drawing thread is a
//! window that stops responding for the duration. The attribute leaves
//! the bodies synchronous and moves them onto the runtime's pool. So is
//! every command below that reads or writes a session's artifacts, for
//! the same reason.
//!
//! Every session a command names arrives as a [`SessionRef`], never as
//! a `String` this module validates. The parse is the deserialization,
//! so a name that could leave the storage root is refused at the IPC
//! boundary and no `Path` is ever built from an unchecked string.

// Tauri resolves a command's managed state by value, so every command
// below takes `State` that way whether or not it consumes it.
#![allow(clippy::needless_pass_by_value)]

use scrybe_application::paging::PageRequest;
use scrybe_application::sessions::{ConfiguredNotesGenerator, SearchRequest};
use scrybe_application::{ApplicationError, ErrorCode, SessionRef};
use tauri::{Manager, State};

use crate::contract::{
    CommandFailure, NotesRegeneration, RecordingStatus, SessionDetail, SessionNotes, SessionRepair,
    SessionRows, SettingsSummary, TranscriptWindow,
};
use crate::state::Desktop;

/// Every command the host registers.
///
/// `build.rs` turns each entry into an `allow-<command>` permission and
/// `tests/capability_audit.rs` proves the capability files and this list
/// describe the same surface.
pub const COMMANDS: &[&str] = &[
    "list_sessions",
    "search_sessions",
    "cancel_query",
    "get_session",
    "read_notes",
    "read_transcript_page",
    "repair_session",
    "regenerate_notes",
    "reveal_session",
    "copy_notes",
    "copy_transcript",
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
#[tauri::command(async)]
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
/// `request_id` is the name the caller gave this search. It is what
/// [`cancel_query`] addresses, and it is the only way a search can be
/// abandoned across this boundary: a `CancellationToken` does not
/// serialize and `invoke` has no abort.
///
/// # Errors
///
/// As `list_sessions`, plus `cancelled` when [`cancel_query`] reached
/// this search before it finished.
#[tauri::command(async)]
pub fn search_sessions(
    desktop: State<'_, Desktop>,
    request_id: String,
    query: String,
    offset: usize,
    limit: usize,
) -> Result<SessionRows, CommandFailure> {
    let (cancel, generation) = desktop.queries().begin(&request_id);
    let request = SearchRequest::new(query).at(PageRequest::new(offset, limit));
    let outcome = desktop
        .application()
        .sessions()
        .search_sessions(&request, &cancel);
    desktop.queries().finish(&request_id, generation);
    outcome.map(Into::into).map_err(Into::into)
}

/// Asks the query named `request_id` to stop, and says whether one was
/// running under that name.
///
/// Synchronous on purpose. It does no I/O, and a cancel queued behind
/// the very search it is trying to stop would arrive after the work it
/// exists to save.
#[must_use]
#[tauri::command]
pub fn cancel_query(desktop: State<'_, Desktop>, request_id: String) -> bool {
    desktop.queries().cancel(&request_id)
}

/// Everything the detail view renders about one session.
///
/// # Errors
///
/// A session no longer under the configured root, an identity that
/// matches more than one, or an unreadable storage root.
#[tauri::command(async)]
pub fn get_session(
    desktop: State<'_, Desktop>,
    id: SessionRef,
) -> Result<SessionDetail, CommandFailure> {
    desktop
        .application()
        .sessions()
        .get_session(&id)
        .map(Into::into)
        .map_err(Into::into)
}

/// A session's durable notes.
///
/// # Errors
///
/// As `get_session`, plus an unreadable `notes.md`.
#[tauri::command(async)]
pub fn read_notes(
    desktop: State<'_, Desktop>,
    id: SessionRef,
) -> Result<SessionNotes, CommandFailure> {
    desktop
        .application()
        .sessions()
        .read_notes(&id)
        .map(Into::into)
        .map_err(Into::into)
}

/// One window of a session's durable transcript.
///
/// The whole document never crosses this boundary. `offset` is the
/// first line and `limit` the window size, both counted in lines, so a
/// window can never split a UTF-8 sequence.
///
/// # Errors
///
/// As `read_notes`.
#[tauri::command(async)]
pub fn read_transcript_page(
    desktop: State<'_, Desktop>,
    id: SessionRef,
    offset: usize,
    limit: usize,
) -> Result<TranscriptWindow, CommandFailure> {
    desktop
        .application()
        .sessions()
        .read_transcript_page(&id, PageRequest::new(offset, limit))
        .map(Into::into)
        .map_err(Into::into)
}

/// Recovers an interrupted session. An explicit mutation, and the only
/// thing it can do is complete a recording that was already made.
///
/// # Errors
///
/// As `get_session`, plus a refusal when the session has nothing to
/// recover and a failure when recovery itself does not complete.
#[tauri::command(async)]
pub fn repair_session(
    desktop: State<'_, Desktop>,
    id: SessionRef,
) -> Result<SessionRepair, CommandFailure> {
    desktop
        .application()
        .sessions()
        .repair_session(&id)
        .map(Into::into)
        .map_err(Into::into)
}

/// Replaces a session's notes from its durable transcript. An explicit
/// mutation, and the only document it can replace is `notes.md`.
///
/// `AppHandle` rather than `State`, because this command awaits a
/// provider and a borrow of managed state cannot be held across that.
///
/// # Errors
///
/// As `get_session`, plus a refusal when the session has no durable
/// transcript, an unreadable configuration, and whatever the configured
/// provider reports — including a build compiled without one.
#[tauri::command]
pub async fn regenerate_notes(
    app: tauri::AppHandle,
    id: SessionRef,
) -> Result<NotesRegeneration, CommandFailure> {
    let config = app.state::<Desktop>().application().config().load()?;
    let generator = ConfiguredNotesGenerator::new(config);
    let outcome = app
        .state::<Desktop>()
        .application()
        .sessions()
        .regenerate_notes(&id, &generator)
        .await;
    outcome.map(Into::into).map_err(Into::into)
}

/// Shows the session's folder in the platform's file manager.
///
/// The frontend names a session, never a path. The folder is resolved
/// in Rust beneath the configured storage root from an identity that
/// structurally cannot contain a separator, and only after the
/// repository has confirmed the session is one it knows.
///
/// # Errors
///
/// As `get_session`, plus a platform that declines to open it.
#[tauri::command(async)]
pub fn reveal_session(
    desktop: State<'_, Desktop>,
    app: tauri::AppHandle,
    id: SessionRef,
) -> Result<(), CommandFailure> {
    // Resolution first, so a name no session answers to is a refusal
    // rather than a file manager opening on nothing.
    let detail = desktop.application().sessions().get_session(&id)?;
    let folder = desktop
        .application()
        .sessions()
        .root()
        .resolve(&detail.id);
    crate::setup::open(&app, &folder.display().to_string())
}

/// Puts a session's durable notes on the system clipboard.
///
/// The document is read and placed on the clipboard in Rust. The
/// frontend names a session and learns only whether it worked.
///
/// # Errors
///
/// As `read_notes`, plus a refusal when the session has no durable
/// notes and a platform that declines the clipboard.
#[tauri::command(async)]
pub fn copy_notes(desktop: State<'_, Desktop>, id: SessionRef) -> Result<(), CommandFailure> {
    let document = desktop.application().sessions().read_notes(&id)?;
    copy(&document.markdown.ok_or_else(|| absent(&id, NOTES))?)
}

/// Puts a session's durable transcript on the system clipboard.
///
/// The only place the whole transcript is ever assembled is here, in
/// Rust, on the way to the clipboard. The view reads the document one
/// window at a time and never holds it, which is the point of
/// `read_transcript_page`; a copy that fetched every window and joined
/// them would put back exactly what that avoids.
///
/// # Errors
///
/// As `copy_notes`.
#[tauri::command(async)]
pub fn copy_transcript(desktop: State<'_, Desktop>, id: SessionRef) -> Result<(), CommandFailure> {
    let document = desktop.application().sessions().read_transcript(&id)?;
    copy(&document.markdown.ok_or_else(|| absent(&id, TRANSCRIPT))?)
}

const NOTES: &str = "notes";
const TRANSCRIPT: &str = "transcript";

fn absent(id: &SessionRef, what: &str) -> CommandFailure {
    ApplicationError::new(
        ErrorCode::NotApplicable,
        format!("session {id} has no durable {what} to copy"),
    )
    .into()
}

/// Hands `text` to the platform's own clipboard.
///
/// `pbcopy(1)` rather than a Tauri plugin, for the reason
/// `setup::open` uses `open(1)`: a plugin's permission would have to
/// appear in the frontend's capability file, and this host grants the
/// frontend nothing beyond its own commands.
#[cfg(target_os = "macos")]
fn copy(text: &str) -> Result<(), CommandFailure> {
    use std::io::Write as _;

    let mut child = std::process::Command::new("/usr/bin/pbcopy")
        .stdin(std::process::Stdio::piped())
        .spawn()
        .map_err(|source| unreachable_clipboard("could not be opened", source))?;
    let Some(mut stdin) = child.stdin.take() else {
        return Err(declined_clipboard("accepted no input"));
    };
    // Written and closed before the wait, in one place, so a write that
    // fails still closes the pipe the child is blocked on rather than
    // returning early and leaving it waiting.
    let written = stdin.write_all(text.as_bytes());
    drop(stdin);
    match (written, child.wait()) {
        (Ok(()), Ok(status)) if status.success() => Ok(()),
        (Ok(()), Ok(_)) => Err(declined_clipboard("declined it")),
        (Err(source), _) | (Ok(()), Err(source)) => {
            Err(unreachable_clipboard("could not be written", source))
        }
    }
}

#[cfg(target_os = "macos")]
fn unreachable_clipboard(what: &str, source: std::io::Error) -> CommandFailure {
    ApplicationError::new(
        ErrorCode::NotApplicable,
        format!("the platform clipboard {what}"),
    )
    .with_source(source)
    .into()
}

#[cfg(target_os = "macos")]
fn declined_clipboard(what: &str) -> CommandFailure {
    ApplicationError::new(
        ErrorCode::NotApplicable,
        format!("the platform clipboard {what}"),
    )
    .into()
}

#[cfg(not(target_os = "macos"))]
fn copy(_text: &str) -> Result<(), CommandFailure> {
    Err(ApplicationError::new(
        ErrorCode::NotApplicable,
        "this platform has no clipboard this application can reach yet",
    )
    .into())
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
