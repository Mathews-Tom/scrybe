// Copyright 2026 Mathews Tom
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     https://www.apache.org/licenses/LICENSE-2.0

//! Classification of one session folder from what is durably on disk.
//!
//! This is the single place the storage-layout invariants of
//! `docs/system-design.md` §8.1 are interpreted. The CLI's `list` and
//! `show` commands and the read-only agent surface each used to carry
//! their own reading of those invariants; the differences between them
//! were the bug surface this module exists to remove.

use chrono::{DateTime, Utc};
use serde::Deserialize;

use crate::identity::SessionRef;
use crate::sessions::contract::{
    ArtifactAvailability, CaptureMetadata, ProviderMetadata, SessionEligibility, SessionState,
};

pub const META_FILE: &str = "meta.toml";
pub const NOTES_FILE: &str = "notes.md";
pub const TRANSCRIPT_FILE: &str = "transcript.md";
pub const AUDIO_FILE: &str = "audio.opus";
pub const JOURNAL_DIR: &str = "journal";
pub const JOURNAL_MANIFEST_FILE: &str = "manifest.toml";

/// Forward-compatible subset of the v1 `meta.toml` schema.
///
/// Deliberately not `deny_unknown_fields`: the Tier-2 contract permits
/// a minor version to add a field, and a reader that rejects unknown
/// keys would turn that addition into a breakage. Fields absent from a
/// session written by an older build simply stay `None`.
#[derive(Debug, Clone, Deserialize)]
pub struct MetaSnapshot {
    pub session_id: String,
    #[serde(default)]
    pub title: Option<String>,
    pub started_at: DateTime<Utc>,
    #[serde(default)]
    pub ended_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub duration_secs: Option<u64>,
    #[serde(default)]
    pub providers: Option<MetaProviders>,
    #[serde(default)]
    pub audio: Option<MetaAudio>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct MetaProviders {
    #[serde(default)]
    pub stt: Option<String>,
    #[serde(default)]
    pub llm: Option<String>,
    #[serde(default)]
    pub diarizer: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct MetaAudio {
    #[serde(default)]
    pub channels: Option<u16>,
    #[serde(default)]
    pub layout: Option<String>,
    #[serde(default)]
    pub sample_rate: Option<u32>,
    #[serde(default)]
    pub bitrate_bps: Option<u32>,
}

/// What a folder scan found. Not serializable: it is the repository's
/// internal view, from which the public contracts are projected.
#[derive(Debug, Clone)]
pub struct ScannedSession {
    /// The session's stable identity. Its textual form is the folder
    /// name, already validated against the confinement rules, so
    /// nothing downstream has to re-check or fall back.
    pub id: SessionRef,
    pub state: SessionState,
    pub meta: Option<MetaSnapshot>,
    pub artifacts: ArtifactAvailability,
}

impl ScannedSession {
    /// Capture metadata safe to show in a main application window.
    #[must_use]
    pub fn capture(&self) -> CaptureMetadata {
        let Some(audio) = self.meta.as_ref().and_then(|meta| meta.audio.as_ref()) else {
            return CaptureMetadata::default();
        };
        CaptureMetadata {
            channels: audio.channels,
            layout: audio.layout.clone(),
            sample_rate_hz: audio.sample_rate,
            bitrate_bps: audio.bitrate_bps,
        }
    }

    /// Provider identities recorded at capture time.
    #[must_use]
    pub fn providers(&self) -> ProviderMetadata {
        let Some(providers) = self.meta.as_ref().and_then(|meta| meta.providers.as_ref()) else {
            return ProviderMetadata::default();
        };
        ProviderMetadata {
            stt: providers.stt.clone(),
            llm: providers.llm.clone(),
            diarizer: providers.diarizer.clone(),
        }
    }

    /// Which follow-up operations the session accepts.
    ///
    /// Repair is offered exactly when `repair_session` has durable
    /// state to work from. Notes regeneration needs a durable
    /// transcript to read, which only a complete session has.
    #[must_use]
    pub const fn eligibility(&self) -> SessionEligibility {
        SessionEligibility {
            repair: matches!(self.state, SessionState::Repairable),
            regenerate_notes: self.artifacts.transcript && self.state.is_complete(),
        }
    }
}

/// Outcome of classifying one directory entry.
#[derive(Debug)]
pub enum Classified {
    /// The folder is a session.
    Session(Box<ScannedSession>),
    /// The folder is not a session at all and is omitted from every
    /// listing, as though it did not exist.
    NotASession,
}

/// A minimal filesystem view, so classification can be exercised
/// without a temporary directory per case.
pub trait FolderView {
    fn exists(&self, relative: &str) -> bool;
    fn read(&self, relative: &str) -> Option<String>;
}

/// Classifies one session folder.
///
/// The rules, in the order they are checked:
///
/// 1. `meta.toml` present and parsing — complete.
/// 2. `meta.toml` present and not parsing — failed. Durable state
///    exists and is corrupt; repair would have to guess.
/// 3. `audio.opus` present without metadata — repairable, because
///    `repair_session` reconstructs metadata from the encoded audio.
/// 4. `journal/manifest.toml` present and parsing — repairable, the
///    ordinary crash case the offline merge recovers.
/// 5. `journal/manifest.toml` present and not parsing — failed.
/// 6. `journal/` present with no manifest — unfinished: the process
///    died before the manifest was written, so there is nothing to
///    merge and no repair to offer.
/// 7. `transcript.md` or `notes.md` present with no audio and no
///    journal — unfinished. Durable text survived a session that never
///    completed; `scrybe doctor` covers recovering the rest, and there
///    is no journal for repair to merge.
/// 8. Anything else — not a session.
pub fn classify(id: SessionRef, view: &dyn FolderView) -> Classified {
    let notes = view.exists(NOTES_FILE);
    let transcript = view.exists(TRANSCRIPT_FILE);
    let audio = view.exists(AUDIO_FILE);

    let (state, meta) = if view.exists(META_FILE) {
        match view.read(META_FILE).map(|body| toml::from_str(&body)) {
            Some(Ok(meta)) => (SessionState::Complete, Some(meta)),
            _ => (SessionState::Failed, None),
        }
    } else if audio {
        (SessionState::Repairable, None)
    } else if view.exists(JOURNAL_DIR) {
        let manifest = format!("{JOURNAL_DIR}/{JOURNAL_MANIFEST_FILE}");
        if view.exists(&manifest) {
            match view.read(&manifest).map(|body| body.parse::<toml::Table>()) {
                Some(Ok(_)) => (SessionState::Repairable, None),
                _ => (SessionState::Failed, None),
            }
        } else {
            (SessionState::Unfinished, None)
        }
    } else if notes || transcript {
        (SessionState::Unfinished, None)
    } else {
        return Classified::NotASession;
    };

    let metadata = meta.is_some();
    Classified::Session(Box::new(ScannedSession {
        id,
        state,
        meta,
        artifacts: ArtifactAvailability {
            notes,
            transcript,
            audio,
            playback: audio && state.is_complete(),
            metadata,
        },
    }))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;
    use std::collections::BTreeMap;

    fn session_ref(text: &str) -> SessionRef {
        SessionRef::parse(text).unwrap()
    }

    #[derive(Default)]
    struct Fixture {
        files: BTreeMap<String, String>,
    }

    impl Fixture {
        fn with(mut self, path: &str, body: &str) -> Self {
            self.files.insert(path.to_string(), body.to_string());
            self
        }

        fn classify(&self) -> ScannedSession {
            match classify(session_ref("2026-04-29-1430-acme-01HXYZ"), self) {
                Classified::Session(session) => *session,
                Classified::NotASession => panic!("fixture was not recognized as a session"),
            }
        }
    }

    impl FolderView for Fixture {
        fn exists(&self, relative: &str) -> bool {
            self.files.contains_key(relative)
                || self
                    .files
                    .keys()
                    .any(|key| key.starts_with(&format!("{relative}/")))
        }

        fn read(&self, relative: &str) -> Option<String> {
            self.files.get(relative).cloned()
        }
    }

    const VALID_META: &str = r#"
session_id = "01HXYZ"
title = "Acme sync"
started_at = "2026-04-29T14:30:00Z"
ended_at = "2026-04-29T15:00:00Z"
duration_secs = 1800

[providers]
stt = "whisper-local"
llm = "stub"
diarizer = "binary-channel"

[audio]
channels = 2
layout = "stereo:mic-l,system-r"
sample_rate = 48000
bitrate_bps = 32000
"#;

    #[test]
    fn test_parsing_metadata_classifies_the_session_complete() {
        let session = Fixture::default()
            .with(META_FILE, VALID_META)
            .with(AUDIO_FILE, "")
            .with(TRANSCRIPT_FILE, "# t\n")
            .with(NOTES_FILE, "## TL;DR\n")
            .classify();

        assert_eq!(session.state, SessionState::Complete);
        assert!(session.artifacts.playback);
        assert!(session.eligibility().regenerate_notes);
        assert!(!session.eligibility().repair);
    }

    #[test]
    fn test_corrupt_metadata_classifies_the_session_failed_rather_than_repairable() {
        let session = Fixture::default()
            .with(META_FILE, "session_id = \n")
            .with(AUDIO_FILE, "")
            .classify();

        assert_eq!(session.state, SessionState::Failed);
        assert!(!session.eligibility().repair);
    }

    #[test]
    fn test_audio_without_metadata_is_repairable_because_metadata_can_be_reconstructed() {
        let session = Fixture::default().with(AUDIO_FILE, "").classify();

        assert_eq!(session.state, SessionState::Repairable);
        assert!(session.eligibility().repair);
    }

    #[test]
    fn test_a_journal_with_its_manifest_is_repairable() {
        let session = Fixture::default()
            .with("journal/manifest.toml", "")
            .classify();

        assert_eq!(session.state, SessionState::Repairable);
        assert!(session.eligibility().repair);
    }

    #[test]
    fn test_a_journal_without_a_manifest_is_unfinished_and_offers_no_repair() {
        let session = Fixture::default().with("journal/mic.pcm", "").classify();

        assert_eq!(session.state, SessionState::Unfinished);
        assert!(!session.eligibility().repair);
    }

    #[test]
    fn test_a_corrupt_journal_manifest_is_failed_rather_than_repairable() {
        let session = Fixture::default()
            .with("journal/manifest.toml", "mic = \n")
            .classify();

        assert_eq!(session.state, SessionState::Failed);
        assert!(!session.eligibility().repair);
    }

    #[test]
    fn test_a_folder_with_no_scrybe_artifact_at_all_is_not_a_session() {
        let classified = classify(
            session_ref("stray"),
            &Fixture::default().with("README.md", ""),
        );

        assert!(matches!(classified, Classified::NotASession));
    }

    #[test]
    fn test_a_surviving_transcript_without_audio_or_journal_is_unfinished() {
        let session = Fixture::default().with(TRANSCRIPT_FILE, "# t\n").classify();

        assert_eq!(session.state, SessionState::Unfinished);
        assert!(!session.eligibility().repair);
        assert!(!session.eligibility().regenerate_notes);
    }

    #[test]
    fn test_playback_is_withheld_from_audio_belonging_to_an_incomplete_session() {
        let session = Fixture::default().with(AUDIO_FILE, "").classify();

        assert!(session.artifacts.audio);
        assert!(!session.artifacts.playback);
    }

    #[test]
    fn test_capture_and_provider_metadata_come_from_the_session_metadata() {
        let session = Fixture::default().with(META_FILE, VALID_META).classify();

        assert_eq!(session.capture().channels, Some(2));
        assert_eq!(
            session.capture().layout.as_deref(),
            Some("stereo:mic-l,system-r")
        );
        assert_eq!(session.providers().stt.as_deref(), Some("whisper-local"));
    }

    #[test]
    fn test_metadata_with_an_unknown_future_key_still_parses() {
        let session = Fixture::default()
            .with(
                META_FILE,
                &format!("{VALID_META}\n[future_block]\nadded_later = true\n"),
            )
            .classify();

        assert_eq!(session.state, SessionState::Complete);
    }
}
