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
use scrybe_core::{repair_session, RepairOutcome};

use crate::runtime::{expand_root, load_or_default_config, resolve_session_folder};

#[derive(ClapArgs, Debug)]
pub struct Args {
    /// Either a session-folder name relative to the storage root, an
    /// absolute path, or the session's ULID/short prefix.
    pub id_or_folder: String,

    /// Override the storage root from config.
    #[arg(long)]
    pub root: Option<PathBuf>,
}

#[allow(clippy::unused_async)]
pub async fn run(args: Args) -> Result<()> {
    let root = if let Some(p) = args.root.as_deref() {
        expand_root(p)
    } else {
        let cfg = load_or_default_config()?;
        expand_root(&cfg.storage.root)
    };
    let folder = resolve_session_folder(&root, &args.id_or_folder)
        .with_context(|| format!("resolving session {}", args.id_or_folder))?;

    match repair_session(&folder)
        .with_context(|| format!("repairing session at {}", folder.display()))?
    {
        RepairOutcome::Repaired(report) => {
            println!(
                "scrybe repair: recovered {:.1}s of {}-channel audio to {}",
                report.encoded_secs,
                report.channels,
                report.audio_path.display()
            );
            if report.wrote_meta {
                println!(
                    "scrybe repair: wrote a reconstructed meta.toml (title, STT/LLM/diarizer \
                     names, and consent details were never durably recorded before the crash)"
                );
            }
        }
        RepairOutcome::NothingToRepair => {
            println!(
                "scrybe repair: nothing to repair in {} (no journal/, or already merged)",
                folder.display()
            );
        }
    }
    Ok(())
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
    async fn test_run_reports_nothing_to_repair_when_no_journal_present() {
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().join("2026-04-29-1430-clean-01HXYZ");
        std::fs::create_dir(&folder).unwrap();

        run(Args {
            id_or_folder: "2026-04-29-1430-clean-01HXYZ".into(),
            root: Some(dir.path().to_path_buf()),
        })
        .await
        .unwrap();
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
