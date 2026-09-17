// Copyright 2026 Mathews Tom
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     https://www.apache.org/licenses/LICENSE-2.0

//! One session in full, the documents its view reads, and what the two
//! actions it offers did.
//!
//! Narrowed the same way `session.rs` narrows a listing row: no path,
//! no resolved folder, and no transcript body. A transcript reaches the
//! frontend only as [`TranscriptWindow`], one window at a time, because
//! a whole document is a payload a view does not need and a `WebView`
//! should not be handed.

use scrybe_application::sessions::{
    ArtifactAvailability, CaptureMetadata, NotesDocument, NotesGenerationOutcome,
    NotesGenerationResult, ProviderMetadata, RepairOutcomeKind, RepairResult, SessionEligibility,
    TranscriptPage,
};
use serde::Serialize;
use ts_rs::TS;

use crate::contract::session::SessionProgress;

/// Which durable artifacts a session currently has.
///
/// Five independent booleans, because that is what the detail view
/// renders. `playback` is its own artifact rather than a view of
/// `audio`: the pipeline writes `playback.opus` only for a two-channel
/// capture, so a mono session is complete, has audio, and has nothing
/// to play.
#[allow(clippy::struct_excessive_bools)]
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, TS)]
pub struct SessionArtifacts {
    pub notes: bool,
    pub transcript: bool,
    pub audio: bool,
    pub playback: bool,
    pub metadata: bool,
}

impl From<ArtifactAvailability> for SessionArtifacts {
    fn from(artifacts: ArtifactAvailability) -> Self {
        Self {
            notes: artifacts.notes,
            transcript: artifacts.transcript,
            audio: artifacts.audio,
            playback: artifacts.playback,
            metadata: artifacts.metadata,
        }
    }
}

/// How the session's audio was captured and encoded.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, TS)]
pub struct SessionCapture {
    #[ts(type = "number | null")]
    pub channels: Option<u16>,
    /// Canonical channel-attribution descriptor, e.g.
    /// `stereo:mic-l,system-r`.
    pub layout: Option<String>,
    #[ts(type = "number | null")]
    pub sample_rate_hz: Option<u32>,
    #[ts(type = "number | null")]
    pub bitrate_bps: Option<u32>,
}

impl From<CaptureMetadata> for SessionCapture {
    fn from(capture: CaptureMetadata) -> Self {
        Self {
            channels: capture.channels,
            layout: capture.layout,
            sample_rate_hz: capture.sample_rate_hz,
            bitrate_bps: capture.bitrate_bps,
        }
    }
}

/// Provider identities recorded at capture time.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, TS)]
pub struct SessionProviders {
    pub stt: Option<String>,
    pub llm: Option<String>,
    pub diarizer: Option<String>,
}

impl From<ProviderMetadata> for SessionProviders {
    fn from(providers: ProviderMetadata) -> Self {
        Self {
            stt: providers.stt,
            llm: providers.llm,
            diarizer: providers.diarizer,
        }
    }
}

/// Which follow-up operations this session currently accepts.
///
/// The view offers exactly what is true here. An action offered for a
/// session that cannot accept it is a button whose only outcome is an
/// error the reader could not have predicted.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, TS)]
pub struct SessionActions {
    pub repair: bool,
    pub regenerate_notes: bool,
}

impl From<SessionEligibility> for SessionActions {
    fn from(eligibility: SessionEligibility) -> Self {
        Self {
            repair: eligibility.repair,
            regenerate_notes: eligibility.regenerate_notes,
        }
    }
}

/// Everything the detail view renders about one session.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, TS)]
pub struct SessionDetail {
    /// The opaque identity every later command addresses this session
    /// by. A folder name, never a path.
    pub id: String,
    pub progress: SessionProgress,
    /// The session ULID recorded in `meta.toml`, when metadata exists.
    pub session_id: Option<String>,
    pub title: Option<String>,
    /// RFC 3339. Rendered in the viewer's locale by the frontend, which
    /// is the only side that knows the viewer's locale.
    pub started_at: Option<String>,
    /// RFC 3339.
    pub ended_at: Option<String>,
    /// `u64` on the wire is a JSON number, not a `bigint`.
    #[ts(type = "number | null")]
    pub duration_secs: Option<u64>,
    pub artifacts: SessionArtifacts,
    pub capture: SessionCapture,
    pub providers: SessionProviders,
    pub actions: SessionActions,
}

impl From<scrybe_application::sessions::SessionDetail> for SessionDetail {
    fn from(detail: scrybe_application::sessions::SessionDetail) -> Self {
        Self {
            id: detail.id.as_str().to_owned(),
            progress: detail.state.into(),
            session_id: detail.session_id,
            title: detail.title,
            started_at: detail.started_at.map(|at| at.to_rfc3339()),
            ended_at: detail.ended_at.map(|at| at.to_rfc3339()),
            duration_secs: detail.duration_secs,
            artifacts: detail.artifacts.into(),
            capture: detail.capture.into(),
            providers: detail.providers.into(),
            actions: detail.eligibility.into(),
        }
    }
}

/// A session's durable notes.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, TS)]
pub struct SessionNotes {
    pub id: String,
    pub progress: SessionProgress,
    /// `null` when the session has no durable notes, which is not the
    /// same as notes that are empty.
    pub markdown: Option<String>,
}

impl From<NotesDocument> for SessionNotes {
    fn from(document: NotesDocument) -> Self {
        Self {
            id: document.id.as_str().to_owned(),
            progress: document.state.into(),
            markdown: document.markdown,
        }
    }
}

/// One window onto a session's transcript.
///
/// Counted in lines, so a window boundary can never split a UTF-8
/// sequence and a view can address a position without holding the
/// document.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, TS)]
pub struct TranscriptWindow {
    pub id: String,
    pub progress: SessionProgress,
    /// The line this window starts at.
    pub cursor: usize,
    /// Transcript lines, newline-stripped.
    pub lines: Vec<String>,
    /// Lines in the whole transcript.
    pub total_lines: usize,
    /// Where the next window starts, or `null` at the end.
    pub next: Option<usize>,
}

impl From<TranscriptPage> for TranscriptWindow {
    fn from(page: TranscriptPage) -> Self {
        Self {
            id: page.id.as_str().to_owned(),
            progress: page.state.into(),
            cursor: page.cursor.line,
            lines: page.lines,
            total_lines: page.total_lines,
            next: page.next.map(|cursor| cursor.line),
        }
    }
}

/// What an explicit repair did.
///
/// Mirrors `scrybe_application::sessions::RepairOutcomeKind` through an
/// exhaustive match, so a new outcome cannot reach the frontend
/// unannounced.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum RepairKind {
    Recovered,
    MetadataReconstructed,
    NothingToRepair,
}

impl From<RepairOutcomeKind> for RepairKind {
    fn from(kind: RepairOutcomeKind) -> Self {
        match kind {
            RepairOutcomeKind::Recovered => Self::Recovered,
            RepairOutcomeKind::MetadataReconstructed => Self::MetadataReconstructed,
            RepairOutcomeKind::NothingToRepair => Self::NothingToRepair,
        }
    }
}

/// The result of recovering one session.
#[derive(Clone, Debug, PartialEq, Serialize, TS)]
pub struct SessionRepair {
    pub id: String,
    pub outcome: RepairKind,
    /// The session's state after the repair, so the view re-renders its
    /// actions from what is true now rather than from what it assumed.
    pub progress: SessionProgress,
    pub recovered_secs: Option<f64>,
    #[ts(type = "number | null")]
    pub channels: Option<u16>,
    pub wrote_metadata: bool,
}

impl From<RepairResult> for SessionRepair {
    fn from(result: RepairResult) -> Self {
        Self {
            id: result.id.as_str().to_owned(),
            outcome: result.outcome.into(),
            progress: result.state.into(),
            recovered_secs: result.recovered_secs,
            channels: result.channels,
            wrote_metadata: result.wrote_metadata,
        }
    }
}

/// Whether regeneration replaced the durable notes.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum NotesOutcome {
    Replaced,
    /// The generator produced the document already on disk.
    Unchanged,
}

impl From<NotesGenerationOutcome> for NotesOutcome {
    fn from(outcome: NotesGenerationOutcome) -> Self {
        match outcome {
            NotesGenerationOutcome::Replaced => Self::Replaced,
            NotesGenerationOutcome::Unchanged => Self::Unchanged,
        }
    }
}

/// The result of regenerating one session's notes.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, TS)]
pub struct NotesRegeneration {
    pub id: String,
    pub outcome: NotesOutcome,
    /// Bytes of the durable `notes.md` after the operation.
    pub bytes: usize,
}

impl From<NotesGenerationResult> for NotesRegeneration {
    fn from(result: NotesGenerationResult) -> Self {
        Self {
            id: result.id.as_str().to_owned(),
            outcome: result.outcome.into(),
            bytes: result.bytes,
        }
    }
}
