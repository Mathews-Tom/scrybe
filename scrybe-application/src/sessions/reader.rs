// Copyright 2026 Mathews Tom
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     https://www.apache.org/licenses/LICENSE-2.0

//! The read-only view of the session repository.
//!
//! A surface handed a [`SessionReader`] can list, search, and read. It
//! cannot repair a session, regenerate notes, write configuration, or
//! reach any other mutation, because no method exists to call — the
//! restriction is a property of the type rather than a rule someone has
//! to remember while writing a handler.
//!
//! This is what the read-only agent surface is given. `SessionReader`
//! is implemented by [`SessionRepository`], so the reads an agent
//! performs are the same reads the CLI performs, with the same
//! classification, confinement, paging, and cache behavior.

use crate::cancellation::CancellationToken;
use crate::identity::SessionRef;
use crate::paging::PageRequest;
use crate::sessions::contract::{
    NotesDocument, SearchPage, SearchRequest, SessionDetail, SessionPage, TranscriptDocument,
    TranscriptPage,
};
use crate::sessions::SessionRepository;
use crate::Result;

/// Everything a mutation-free consumer may do with sessions.
pub trait SessionReader {
    /// One page of sessions, most recently started first.
    ///
    /// # Errors
    ///
    /// See [`SessionRepository::list_sessions`].
    fn list_sessions(&self, page: PageRequest) -> Result<SessionPage>;

    /// One page of sessions matching `request`.
    ///
    /// # Errors
    ///
    /// See [`SessionRepository::search_sessions`].
    fn search_sessions(
        &self,
        request: &SearchRequest,
        cancel: &CancellationToken,
    ) -> Result<SearchPage>;

    /// One session's detail.
    ///
    /// # Errors
    ///
    /// See [`SessionRepository::get_session`].
    fn get_session(&self, id: &SessionRef) -> Result<SessionDetail>;

    /// One session's durable notes.
    ///
    /// # Errors
    ///
    /// See [`SessionRepository::read_notes`].
    fn read_notes(&self, id: &SessionRef) -> Result<NotesDocument>;

    /// One session's complete durable transcript.
    ///
    /// # Errors
    ///
    /// See [`SessionRepository::read_transcript`].
    fn read_transcript(&self, id: &SessionRef) -> Result<TranscriptDocument>;

    /// One window of a session's durable transcript.
    ///
    /// # Errors
    ///
    /// See [`SessionRepository::read_transcript_page`].
    fn read_transcript_page(&self, id: &SessionRef, page: PageRequest) -> Result<TranscriptPage>;
}

impl SessionReader for SessionRepository {
    fn list_sessions(&self, page: PageRequest) -> Result<SessionPage> {
        Self::list_sessions(self, page)
    }

    fn search_sessions(
        &self,
        request: &SearchRequest,
        cancel: &CancellationToken,
    ) -> Result<SearchPage> {
        Self::search_sessions(self, request, cancel)
    }

    fn get_session(&self, id: &SessionRef) -> Result<SessionDetail> {
        Self::get_session(self, id)
    }

    fn read_notes(&self, id: &SessionRef) -> Result<NotesDocument> {
        Self::read_notes(self, id)
    }

    fn read_transcript(&self, id: &SessionRef) -> Result<TranscriptDocument> {
        Self::read_transcript(self, id)
    }

    fn read_transcript_page(&self, id: &SessionRef, page: PageRequest) -> Result<TranscriptPage> {
        Self::read_transcript_page(self, id, page)
    }
}
