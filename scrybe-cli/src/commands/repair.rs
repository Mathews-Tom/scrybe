// Copyright 2026 Mathews Tom
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     https://www.apache.org/licenses/LICENSE-2.0

//! `scrybe repair <id-or-folder>` — recovers `audio.opus` from a
//! session's `journal/` after a crash or `SIGKILL` left the session
//! without a completed offline merge.

use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::Args as ClapArgs;
use scrybe_application::sessions::RepairOutcomeKind;
use scrybe_application::SessionRef;

use crate::runtime::application;

#[derive(ClapArgs, Debug)]
pub struct Args {
    /// Either a session-folder name relative to the storage root or
    /// the session's ULID/short prefix. Paths are not accepted: every
    /// session resolves beneath the configured storage root.
    pub id_or_folder: String,

    /// Override the storage root from config.
    #[arg(long)]
    pub root: Option<PathBuf>,
}

#[allow(clippy::unused_async)]
pub async fn run(args: Args) -> Result<()> {
    let app = application(args.root.as_deref())?;
    let repository = app.sessions();
    let id = SessionRef::parse(&args.id_or_folder)
        .map_err(scrybe_application::ApplicationError::from)
        .with_context(|| format!("resolving session {}", args.id_or_folder))?;
    let detail = repository
        .get_session(&id)
        .with_context(|| format!("resolving session {}", args.id_or_folder))?;
    let folder = repository.root().resolve(&detail.id);
    clear_stale_session_lock(&folder)?;

    let result = repository
        .repair_session(&detail.id)
        .with_context(|| format!("repairing session at {}", folder.display()))?;
    match result.outcome {
        RepairOutcomeKind::Recovered => {
            println!(
                "scrybe repair: recovered {:.1}s of {}-channel audio to {}",
                result.recovered_secs.unwrap_or_default(),
                result.channels.unwrap_or_default(),
                folder.join("audio.opus").display()
            );
            if result.wrote_metadata {
                println!(
                    "scrybe repair: wrote a reconstructed meta.toml (title, STT/LLM/diarizer \
                     names, and consent details were never durably recorded before the crash)"
                );
            }
        }
        RepairOutcomeKind::MetadataReconstructed => {
            println!(
                "scrybe repair: audio was already complete ({:.1}s, {} channels); wrote reconstructed meta.toml",
                result.recovered_secs.unwrap_or_default(),
                result.channels.unwrap_or_default()
            );
            println!(
                "scrybe repair: run `scrybe notes {}` to regenerate notes.md",
                result.id
            );
        }
        RepairOutcomeKind::NothingToRepair => {
            println!(
                "scrybe repair: nothing to repair in {} (no journal/, or already merged)",
                folder.display()
            );
        }
    }
    Ok(())
}

fn clear_stale_session_lock(folder: &std::path::Path) -> Result<()> {
    let lock_path = folder.join(scrybe_core::storage::PID_LOCK_NAME);
    if !lock_path.exists() {
        return Ok(());
    }
    if scrybe_application::diagnostics::lock_owner_alive(&lock_path) == Some(true) {
        anyhow::bail!(
            "session at {} is still owned by the process in {}",
            folder.display(),
            lock_path.display()
        );
    }
    std::fs::remove_file(&lock_path)
        .with_context(|| format!("removing stale session lock {}", lock_path.display()))
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

    #[tokio::test]
    async fn test_run_reports_nothing_to_repair_for_a_complete_session() {
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().join("2026-04-29-1430-clean-01HXYZ");
        std::fs::create_dir(&folder).unwrap();
        std::fs::write(
            folder.join("meta.toml"),
            "session_id = \"01HXYZ\"\nstarted_at = \"2026-04-29T14:30:00Z\"\n",
        )
        .unwrap();
        std::fs::write(folder.join("audio.opus"), b"").unwrap();

        run(Args {
            id_or_folder: "2026-04-29-1430-clean-01HXYZ".into(),
            root: Some(dir.path().to_path_buf()),
        })
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn test_run_reports_a_folder_that_is_not_a_session_as_not_found() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("2026-04-29-1430-empty-01HXYZ")).unwrap();

        let error = run(Args {
            id_or_folder: "2026-04-29-1430-empty-01HXYZ".into(),
            root: Some(dir.path().to_path_buf()),
        })
        .await
        .unwrap_err();

        assert!(error.to_string().contains("resolving session"));
    }
    #[cfg(unix)]
    #[test]
    fn test_clear_stale_session_lock_removes_dead_owner() {
        let mut exited = std::process::Command::new("true").spawn().unwrap();
        let dead_pid = exited.id();
        exited.wait().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let lock = dir.path().join(scrybe_core::storage::PID_LOCK_NAME);
        std::fs::write(&lock, dead_pid.to_string()).unwrap();

        clear_stale_session_lock(dir.path()).unwrap();

        assert!(!lock.exists());
    }

    #[tokio::test]
    async fn test_run_recovers_audio_from_crashed_session() {
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().join("2026-04-29-1430-crashed-01HZZZ");
        std::fs::create_dir(&folder).unwrap();
        let journal_dir = folder.join("journal");
        let writer = scrybe_core::pipeline::JournalWriter::spawn(
            &journal_dir,
            scrybe_core::FrameSource::Mic,
            16_000,
            1,
        )
        .unwrap();
        let samples: std::sync::Arc<[f32]> =
            (0..16_000).map(|i| (i as f32 * 0.01).sin() * 0.5).collect();
        writer.push(samples);
        let summary = writer.finish().unwrap();
        scrybe_core::pipeline::journal::write_manifest(
            &journal_dir,
            &scrybe_core::pipeline::JournalManifest {
                mic: Some(scrybe_core::pipeline::JournalAnchor {
                    first_frame_epoch_ms: 1_735_000_000_000,
                    sample_rate: summary.sample_rate,
                    channels: summary.channels,
                    frames_written: summary.frames_written,
                    first_frame_timestamp_ns: None,
                    last_frame_end_timestamp_ns: None,
                }),
                system: None,
            },
        )
        .unwrap();

        run(Args {
            id_or_folder: "2026-04-29-1430-crashed-01HZZZ".into(),
            root: Some(dir.path().to_path_buf()),
        })
        .await
        .unwrap();

        assert!(folder.join("audio.opus").exists());
        assert!(folder.join("meta.toml").exists());
        assert!(!journal_dir.exists());
    }

    #[tokio::test]
    async fn test_run_returns_error_when_session_does_not_exist() {
        let dir = tempfile::tempdir().unwrap();

        let err = run(Args {
            id_or_folder: "nonexistent".into(),
            root: Some(dir.path().to_path_buf()),
        })
        .await
        .unwrap_err();

        assert!(err.to_string().contains("nonexistent"));
    }
}
