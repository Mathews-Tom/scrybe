// Copyright 2026 Mathews Tom
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     https://www.apache.org/licenses/LICENSE-2.0

//! The session repository and its contracts.
//!
//! One implementation of the storage-layout invariants serves the CLI,
//! the read-only agent surface, and any future frontend. Sessions are
//! addressed by opaque identity, never by path; listings and search are
//! paged; search is cancellable; and the cached scan is invalidated,
//! on every filesystem, by any artifact classification reads appearing
//! or disappearing under the configured root.

mod contract;
mod repository;
mod scan;

pub use contract::{
    ArtifactAvailability, CaptureMetadata, NotesDocument, NotesGenerationOutcome,
    NotesGenerationResult, ProviderMetadata, RepairOutcomeKind, RepairResult, SearchPage,
    SearchRequest, SessionDetail, SessionEligibility, SessionPage, SessionState, SessionSummary,
    TranscriptCursor, TranscriptPage,
};
pub use repository::{NotesGenerationRequest, NotesGenerator, SessionRepository};
