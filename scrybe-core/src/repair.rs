// Copyright 2026 Mathews Tom
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     https://www.apache.org/licenses/LICENSE-2.0

//! Session repair: recovers `audio.opus` from a journal left behind
//! by a session that never reached `drive_session`'s normal completion.
//!
//! The trigger is a crash, a `SIGKILL`, or a merge that failed its
//! duration assertion and was never retried.
//!
//! `journal/manifest.toml` is written incrementally during capture
//! (`session::SessionJournals::push`, on each source's first frame),
//! not just at session end, so even a process killed mid-recording
//! leaves the anchors `repair_session` needs. Running the identical
//! `pipeline::merge::merge_journal` used by a normal session keeps
//! recovery and the live path on one code path — there is no
//! separate "repair encoder."
//!
//! `transcript.md` is durable independently (`storage::append_durable`
//! per completed chunk during capture), so a repaired session already
//! carries whatever was transcribed up to its last completed chunk
//! boundary before the crash; `repair_session` does not re-run STT.
//! `notes.md` is never durable before a normal session's final write,
//! so a repaired session has no notes — `scrybe repair`'s job is
//! audio recovery, not notes regeneration.

use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};

use crate::error::CoreError;
use crate::pipeline::encoder::EncoderConfig;
use crate::pipeline::journal::JournalManifest;
use crate::pipeline::merge::merge_journal;
use crate::session::{audio_layout, build_meta_toml, AudioMeta, MetaArgs};
use crate::storage::atomic_replace;
use crate::types::{ConsentAttestation, ConsentMode, SessionId};

/// What `repair_session` found and did.
#[derive(Debug)]
pub enum RepairOutcome {
    /// `journal/` recovered into `audio.opus`.
    Repaired(RepairReport),
    /// No `journal/` directory present, or the session already has a
    /// complete `audio.opus` — nothing for repair to do.
    NothingToRepair,
}

/// Facts about a completed repair, for the caller to report to the
/// user.
#[derive(Debug)]
pub struct RepairReport {
    pub audio_path: PathBuf,
    pub encoded_secs: f64,
    pub channels: u16,
    /// `true` when `meta.toml` did not already exist and repair wrote
    /// a reconstructed one so the session stays visible to `scrybe
    /// list` / `scrybe show`.
    pub wrote_meta: bool,
}

/// Recovers `<folder>/audio.opus` from `<folder>/journal/`.
///
/// Uses the same offline merge a normal session runs, then — only if
/// `meta.toml` is not already present — writes a reconstructed one so
/// the session remains visible to `scrybe list` and `scrybe show`.
///
/// The duration assertion `merge_journal` normally applies against
/// real wall-clock elapsed time is not meaningful here: nothing
/// tracked how long the interrupted session actually ran. Repair
/// passes `0.0` (skip) and reports the merge's own recovered duration
/// instead — the honest answer to "how much did we get back", not a
/// claim that it matches some other measurement.
///
/// # Errors
///
/// `CoreError::Storage` if `journal/manifest.toml` cannot be read or
/// parsed (a session killed before its first frame ever landed has no
/// manifest to recover from — nothing `repair_session` can do).
/// Other `CoreError` variants propagate from `merge_journal` (e.g.
/// `PipelineError::EmptyJournal` if `manifest.toml` names a source
/// with no segment bytes on disk) or from writing the reconstructed
/// `meta.toml`.
pub fn repair_session(folder: &Path) -> Result<RepairOutcome, CoreError> {
    let journal_dir = folder.join("journal");
    let audio_path = folder.join("audio.opus");
    if !journal_dir.exists() || audio_path.exists() {
        return Ok(RepairOutcome::NothingToRepair);
    }

    let manifest = crate::pipeline::journal::read_manifest(&journal_dir)?;
    let report = merge_journal(
        &journal_dir,
        &audio_path,
        &manifest,
        EncoderConfig::default(),
        0.0,
    )?;

    let meta_path = folder.join("meta.toml");
    let wrote_meta = if meta_path.exists() {
        false
    } else {
        write_reconstructed_meta(
            folder,
            &meta_path,
            &manifest,
            report.channels,
            report.encoded_secs,
        )?;
        true
    };

    Ok(RepairOutcome::Repaired(RepairReport {
        audio_path,
        encoded_secs: report.encoded_secs,
        channels: report.channels,
        wrote_meta,
    }))
}

/// Best-effort session start time: the earliest `first_frame_epoch_ms`
/// across whichever sources the manifest names. Every journaled
/// session has at least one source by the time `repair_session` gets
/// this far (an empty manifest would already have failed inside
/// `merge_journal` for lacking segments).
fn earliest_epoch(manifest: &JournalManifest) -> Option<DateTime<Utc>> {
    [
        manifest.mic.map(|a| a.first_frame_epoch_ms),
        manifest.system.map(|a| a.first_frame_epoch_ms),
    ]
    .into_iter()
    .flatten()
    .min()
    .and_then(DateTime::from_timestamp_millis)
}

#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn write_reconstructed_meta(
    folder: &Path,
    meta_path: &Path,
    manifest: &JournalManifest,
    channels: u16,
    encoded_secs: f64,
) -> Result<(), CoreError> {
    let id = folder
        .file_name()
        .and_then(|s| s.to_str())
        .and_then(|name| name.rsplit('-').next())
        .and_then(|suffix| suffix.parse::<SessionId>().ok())
        .unwrap_or_default();
    let started_at = earliest_epoch(manifest).unwrap_or_else(Utc::now);
    // `ended_at` only exists so `build_meta_toml`'s existing
    // `duration_secs = ended_at - started_at` computation reports the
    // real recovered duration; it is not a claim about when the
    // interrupted session actually stopped.
    let ended_at = started_at + chrono::Duration::milliseconds((encoded_secs * 1000.0) as i64);
    let attestation = ConsentAttestation::new(
        ConsentMode::Quick,
        "unknown (reconstructed by scrybe repair; the interrupted session's own \
         consent attestation was never durably written)",
    );
    let audio_meta = Some(AudioMeta {
        channels,
        layout: audio_layout(channels == 2, channels).to_string(),
        sample_rate: EncoderConfig::default().sample_rate,
        bitrate_bps: EncoderConfig::default().bitrate_bps,
        mic_epoch_ms: manifest.mic.map(|a| a.first_frame_epoch_ms),
        system_epoch_ms: manifest.system.map(|a| a.first_frame_epoch_ms),
    });
    let meta = build_meta_toml(MetaArgs {
        id,
        title: None,
        started_at,
        ended_at,
        attestation: &attestation,
        stt_name: "unknown (scrybe repair)",
        llm_name: "unknown (scrybe repair)",
        diarizer_name: "unknown (scrybe repair)",
        audio: audio_meta,
    })?;
    atomic_replace(meta_path, meta.as_bytes()).map_err(CoreError::Storage)
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::cast_precision_loss
)]
mod tests {
    use super::*;
    use crate::pipeline::journal::{segment_path, JournalAnchor, JournalWriter};
    use crate::types::FrameSource;
    use tempfile::tempdir;

    fn sine(n: usize, freq_scale: f32) -> Vec<f32> {
        (0..n)
            .map(|i| (i as f32 * freq_scale).sin() * 0.5)
            .collect()
    }

    #[test]
    fn test_repair_session_returns_nothing_to_repair_when_no_journal_present() {
        let dir = tempdir().unwrap();
        let folder = dir.path().join("2026-04-29-1430-nojournal-01HAAA");
        std::fs::create_dir_all(&folder).unwrap();

        let outcome = repair_session(&folder).unwrap();

        assert!(matches!(outcome, RepairOutcome::NothingToRepair));
    }

    #[test]
    fn test_repair_session_returns_nothing_to_repair_when_audio_already_exists() {
        let dir = tempdir().unwrap();
        let folder = dir.path().join("2026-04-29-1430-already-01HBBB");
        std::fs::create_dir_all(folder.join("journal")).unwrap();
        std::fs::write(folder.join("audio.opus"), b"already merged").unwrap();

        let outcome = repair_session(&folder).unwrap();

        assert!(matches!(outcome, RepairOutcome::NothingToRepair));
    }

    #[test]
    fn test_repair_session_recovers_audio_and_writes_meta_when_absent() {
        let dir = tempdir().unwrap();
        let folder = dir.path().join("2026-04-29-1430-crashed-01M1KREPAIR01");
        std::fs::create_dir_all(&folder).unwrap();
        let journal_dir = folder.join("journal");
        let writer = JournalWriter::spawn(&journal_dir, FrameSource::Mic, 16_000, 1).unwrap();
        writer.push(sine(32_000, 0.01).into()); // 2s of audio at 16kHz.
        let summary = writer.finish().unwrap();
        crate::pipeline::journal::write_manifest(
            &journal_dir,
            &JournalManifest {
                mic: Some(JournalAnchor {
                    first_frame_epoch_ms: 1_735_000_000_000,
                    sample_rate: summary.sample_rate,
                    channels: summary.channels,
                    frames_written: summary.frames_written,
                }),
                system: None,
            },
        )
        .unwrap();

        let outcome = repair_session(&folder).unwrap();

        let RepairOutcome::Repaired(report) = outcome else {
            panic!("expected Repaired outcome");
        };
        assert!(report.wrote_meta);
        assert!((report.encoded_secs - 2.0).abs() < 0.01);
        assert_eq!(report.channels, 1);
        assert!(folder.join("audio.opus").exists());
        assert!(!journal_dir.exists(), "journal must be deleted on success");
        let meta = std::fs::read_to_string(folder.join("meta.toml")).unwrap();
        assert!(meta.contains("mic_epoch_ms"));
        assert!(meta.contains("channels = 1"));
    }

    #[test]
    fn test_repair_session_does_not_overwrite_existing_meta_toml() {
        let dir = tempdir().unwrap();
        let folder = dir.path().join("2026-04-29-1430-hasmeta-01M1KREPAIR02");
        std::fs::create_dir_all(&folder).unwrap();
        let journal_dir = folder.join("journal");
        let writer = JournalWriter::spawn(&journal_dir, FrameSource::Mic, 16_000, 1).unwrap();
        writer.push(sine(16_000, 0.01).into());
        let summary = writer.finish().unwrap();
        crate::pipeline::journal::write_manifest(
            &journal_dir,
            &JournalManifest {
                mic: Some(JournalAnchor {
                    first_frame_epoch_ms: 1_735_000_000_000,
                    sample_rate: summary.sample_rate,
                    channels: summary.channels,
                    frames_written: summary.frames_written,
                }),
                system: None,
            },
        )
        .unwrap();
        std::fs::write(folder.join("meta.toml"), "session_id = \"pre-existing\"\n").unwrap();

        let outcome = repair_session(&folder).unwrap();

        let RepairOutcome::Repaired(report) = outcome else {
            panic!("expected Repaired outcome");
        };
        assert!(!report.wrote_meta);
        let meta = std::fs::read_to_string(folder.join("meta.toml")).unwrap();
        assert!(
            meta.contains("pre-existing"),
            "existing meta.toml must survive untouched"
        );
    }

    #[test]
    fn test_repair_session_fails_when_manifest_missing_but_segments_exist() {
        // A crash before the incremental manifest write ever landed
        // (the first frame's spawn) leaves orphaned segments with no
        // way to know their native sample_rate/channels safely.
        let dir = tempdir().unwrap();
        let folder = dir.path().join("2026-04-29-1430-nomanifest-01HCCC");
        let journal_dir = folder.join("journal");
        std::fs::create_dir_all(&journal_dir).unwrap();
        let bytes: Vec<u8> = sine(1_000, 0.01)
            .iter()
            .flat_map(|s| s.to_le_bytes())
            .collect();
        std::fs::write(segment_path(&journal_dir, "mic", 0), bytes).unwrap();

        let err = repair_session(&folder).unwrap_err();

        assert!(matches!(err, CoreError::Storage(_)));
    }
}
