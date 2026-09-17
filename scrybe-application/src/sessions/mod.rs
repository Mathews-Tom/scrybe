// Copyright 2026 Mathews Tom
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     https://www.apache.org/licenses/LICENSE-2.0

//! Session repository contracts.
//!
//! These are the serializable request and response shapes shared by the
//! CLI, the read-only agent surface, and any future frontend. They
//! describe a session entirely in terms a consumer can act on — state,
//! availability, eligibility — and never in terms of a filesystem path.

mod contract;

pub use contract::{
    ArtifactAvailability, CaptureMetadata, NotesDocument, NotesGenerationOutcome,
    NotesGenerationResult, ProviderMetadata, RepairOutcomeKind, RepairResult, SearchPage,
    SearchRequest, SessionDetail, SessionEligibility, SessionPage, SessionState, SessionSummary,
    TranscriptCursor, TranscriptPage,
};
