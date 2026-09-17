// Copyright 2026 Mathews Tom
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     https://www.apache.org/licenses/LICENSE-2.0

//! Serializable session request and response types.

use chrono::{DateTime, Utc};
use scrybe_core::repair::RepairOutcome;
use serde::{Deserialize, Serialize};

use crate::identity::SessionRef;
use crate::paging::{Page, PageRequest};

/// How far a session folder got through the recording pipeline.
///
/// The four states are distinguished by what is durably on disk, which
/// is what determines the action a consumer can offer:
///
/// - [`Self::Complete`] — `meta.toml` is present and parses. Every
///   artifact the session produced is final.
/// - [`Self::Repairable`] — no readable `meta.toml`, but enough durable
///   state survives for `repair_session` to recover the meeting: either
///   a journal with its manifest, or already-merged audio missing only
///   its metadata.
/// - [`Self::Unfinished`] — a recording started and left a journal, but
///   nothing durable to merge. There is no repair to offer.
/// - [`Self::Failed`] — durable state exists and is corrupt: `meta.toml`
///   or the journal manifest is present but unparseable. Repair cannot
///   proceed without a human deciding what to do.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionState {
    Complete,
    Unfinished,
    Repairable,
    Failed,
}

impl SessionState {
    /// Whether the session reached normal completion.
    #[must_use]
    pub const fn is_complete(self) -> bool {
        matches!(self, Self::Complete)
    }
}

/// Which durable artifacts a session currently has.
///
/// One independent boolean per artifact, because that is what a detail
/// view renders: five checkboxes, not a state machine. Collapsing them
/// into an enum would require enumerating every combination the
/// filesystem can actually produce, including the partial ones repair
/// exists to resolve.
#[allow(clippy::struct_excessive_bools)]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct ArtifactAvailability {
    /// `notes.md` exists.
    pub notes: bool,
    /// `transcript.md` exists.
    pub transcript: bool,
    /// `audio.opus` exists.
    pub audio: bool,
    /// Audio exists on a completed session, so it is safe to play back.
    /// Audio belonging to a session that never finished is not, because
    /// its duration and channel attribution are not yet established.
    pub playback: bool,
    /// `meta.toml` exists and parsed.
    pub metadata: bool,
}

/// Provider identities recorded at capture time.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct ProviderMetadata {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stt: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub llm: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub diarizer: Option<String>,
}

/// How the session's audio was captured and encoded.
///
/// Carries no sample data and no transcript text, so it is safe to show
/// in a main application window.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct CaptureMetadata {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channels: Option<u16>,
    /// Canonical channel-attribution descriptor, e.g. `stereo:mic-l,system-r`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub layout: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sample_rate_hz: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bitrate_bps: Option<u32>,
}

/// Which follow-up operations the session currently accepts.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct SessionEligibility {
    /// `repair_session` can recover this session.
    pub repair: bool,
    /// A durable transcript exists, so notes can be regenerated from it.
    pub regenerate_notes: bool,
}

/// One row of a listing or search result.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SessionSummary {
    pub id: SessionRef,
    pub state: SessionState,
    /// The session ULID recorded in `meta.toml`, when metadata exists.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_secs: Option<u64>,
}

/// Everything a detail view needs about one session.
///
/// The session's folder path is resolved and retained inside the
/// service layer and is deliberately not a field here: a consumer that
/// never receives a path cannot send one back.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SessionDetail {
    pub id: SessionRef,
    pub state: SessionState,
    /// The session ULID recorded in `meta.toml`, when metadata exists.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_secs: Option<u64>,
    pub artifacts: ArtifactAvailability,
    pub capture: CaptureMetadata,
    pub providers: ProviderMetadata,
    pub eligibility: SessionEligibility,
}

/// A page of listing rows.
pub type SessionPage = Page<SessionSummary>;

/// A page of search rows.
pub type SearchPage = Page<SessionSummary>;

/// A cancellable, paged search over the configured storage root.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SearchRequest {
    /// Case-insensitive substring matched against folder name, title,
    /// and — for complete sessions only — notes and transcript text.
    pub query: String,
    #[serde(default)]
    pub page: PageRequest,
}

impl SearchRequest {
    /// A first-page search for `query`.
    #[must_use]
    pub fn new(query: impl Into<String>) -> Self {
        Self {
            query: query.into(),
            page: PageRequest::first(),
        }
    }

    /// The same search at a different page.
    #[must_use]
    pub const fn at(mut self, page: PageRequest) -> Self {
        self.page = page;
        self
    }
}

/// A session's `notes.md`.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct NotesDocument {
    pub id: SessionRef,
    pub state: SessionState,
    /// `None` when the session has no durable notes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub markdown: Option<String>,
}

/// A session's complete `transcript.md`.
///
/// For a consumer that genuinely needs the whole document — handing it
/// to a language model, for instance. A view that renders it should
/// page instead, through [`TranscriptPage`].
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct TranscriptDocument {
    pub id: SessionRef,
    pub state: SessionState,
    /// `None` when the session has no durable transcript.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub markdown: Option<String>,
}

/// Position within a session's transcript, counted in lines so a page
/// boundary can never split a UTF-8 sequence.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct TranscriptCursor {
    pub line: usize,
}

impl TranscriptCursor {
    /// The start of the transcript.
    #[must_use]
    pub const fn start() -> Self {
        Self { line: 0 }
    }

    /// A cursor at `line`.
    #[must_use]
    pub const fn at(line: usize) -> Self {
        Self { line }
    }
}

/// One window of a session's transcript.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct TranscriptPage {
    pub id: SessionRef,
    pub state: SessionState,
    /// Where this window starts.
    pub cursor: TranscriptCursor,
    /// Transcript lines, newline-stripped.
    pub lines: Vec<String>,
    /// Lines in the whole transcript.
    pub total_lines: usize,
    /// Where the next window starts, or `None` at the end.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next: Option<TranscriptCursor>,
}

/// What an explicit repair did.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RepairOutcomeKind {
    /// A journal was merged into `audio.opus` and metadata written.
    Recovered,
    /// Audio already existed; only `meta.toml` was reconstructed.
    MetadataReconstructed,
    /// The session was already complete.
    NothingToRepair,
}

impl From<&RepairOutcome> for RepairOutcomeKind {
    fn from(value: &RepairOutcome) -> Self {
        match value {
            RepairOutcome::Repaired(_) => Self::Recovered,
            RepairOutcome::MetadataReconstructed(_) => Self::MetadataReconstructed,
            RepairOutcome::NothingToRepair => Self::NothingToRepair,
        }
    }
}

/// Result of an explicit repair.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RepairResult {
    pub id: SessionRef,
    pub outcome: RepairOutcomeKind,
    /// The session's state after the repair.
    pub state: SessionState,
    /// Audio duration recovered from the journal, when a merge ran.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recovered_secs: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channels: Option<u16>,
    /// Whether `meta.toml` was written by this repair.
    pub wrote_metadata: bool,
}

/// Whether notes regeneration replaced the durable document.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NotesGenerationOutcome {
    Replaced,
    /// The generator produced the same document already on disk.
    Unchanged,
}

/// Result of an explicit notes regeneration.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct NotesGenerationResult {
    pub id: SessionRef,
    pub outcome: NotesGenerationOutcome,
    /// Bytes of the durable `notes.md` after the operation.
    pub bytes: usize,
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn test_session_detail_never_serializes_a_filesystem_path() {
        let detail = SessionDetail {
            id: SessionRef::parse("2026-04-29-1430-acme-01HXYZ").unwrap(),
            state: SessionState::Complete,
            session_id: Some("01HXYZ".into()),
            title: Some("Acme sync".into()),
            started_at: None,
            ended_at: None,
            duration_secs: Some(600),
            artifacts: ArtifactAvailability {
                notes: true,
                transcript: true,
                audio: true,
                playback: true,
                metadata: true,
            },
            capture: CaptureMetadata::default(),
            providers: ProviderMetadata::default(),
            eligibility: SessionEligibility::default(),
        };

        let encoded = serde_json::to_value(&detail).unwrap();

        assert!(encoded.get("folder").is_none());
        assert!(encoded.get("path").is_none());
        assert_eq!(encoded["id"], "2026-04-29-1430-acme-01HXYZ");
    }

    #[test]
    fn test_repair_outcome_kinds_map_from_the_core_outcome() {
        assert_eq!(
            RepairOutcomeKind::from(&RepairOutcome::NothingToRepair),
            RepairOutcomeKind::NothingToRepair
        );
    }

    #[test]
    fn test_a_search_request_defaults_to_the_first_page() {
        let request: SearchRequest = serde_json::from_str(r#"{"query":"standup"}"#).unwrap();

        assert_eq!(request.page, PageRequest::first());
    }
}
