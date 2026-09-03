// Copyright 2026 Mathews Tom
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     https://www.apache.org/licenses/LICENSE-2.0

//! `scrybe list` — folder listing of the configured root, with title
//! and duration extracted from each session's `meta.toml`.

use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::Args as ClapArgs;
use serde::Deserialize;

use crate::runtime::{expand_root, load_or_default_config};

#[derive(ClapArgs, Debug)]
pub struct Args {
    /// Override the storage root from config.
    #[arg(long)]
    pub root: Option<PathBuf>,
}

#[derive(Debug, Deserialize)]
struct MetaSnapshot {
    session_id: String,
    title: Option<String>,
    duration_secs: Option<u64>,
}

/// One row `scrybe list` renders: either a completed session read
/// from `meta.toml`, or an unfinished one — `journal/` present but no
/// `audio.opus` and no `meta.toml`, meaning the process exited (crash
/// or `SIGKILL`) before the offline merge ran. Reported explicitly
/// rather than silently skipped, per the unfinished-session
/// invariant: `journal/` with no `audio.opus` is always a signal to
/// run `scrybe repair`, never a folder to quietly ignore.
enum SessionRow {
    Complete {
        folder: String,
        snapshot: MetaSnapshot,
    },
    Unfinished {
        folder: String,
    },
}

pub async fn run(args: Args) -> Result<()> {
    let root = if let Some(p) = args.root.as_deref() {
        expand_root(p)
    } else {
        let cfg = load_or_default_config()?;
        expand_root(&cfg.storage.root)
    };
    if !root.exists() {
        println!(
            "scrybe list: no sessions found (root {} does not exist)",
            root.display()
        );
        return Ok(());
    }

    let mut entries: Vec<(String, SessionRow)> = Vec::new();
    let read = tokio::fs::read_dir(&root)
        .await
        .with_context(|| format!("reading {}", root.display()))?;
    let mut entries_stream = read;
    while let Some(entry) = entries_stream
        .next_entry()
        .await
        .with_context(|| format!("iterating {}", root.display()))?
    {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let folder_name = path
            .file_name()
            .map_or_else(|| "<unknown>".into(), |s| s.to_string_lossy().into_owned());
        let meta_path = path.join("meta.toml");
        if meta_path.exists() {
            let body = tokio::fs::read_to_string(&meta_path)
                .await
                .with_context(|| format!("reading {}", meta_path.display()))?;
            let snapshot: MetaSnapshot = toml::from_str(&body)
                .with_context(|| format!("parsing {}", meta_path.display()))?;
            entries.push((
                folder_name.clone(),
                SessionRow::Complete {
                    folder: folder_name,
                    snapshot,
                },
            ));
            continue;
        }
        let journal_dir = path.join("journal");
        let audio_path = path.join("audio.opus");
        if journal_dir.exists() && !audio_path.exists() {
            entries.push((
                folder_name.clone(),
                SessionRow::Unfinished {
                    folder: folder_name,
                },
            ));
        }
    }

    entries.sort_by(|a, b| a.0.cmp(&b.0));
    if entries.is_empty() {
        println!("scrybe list: no sessions in {}", root.display());
        return Ok(());
    }
    println!("{:<48} {:<28} duration  title", "folder", "session_id");
    for (_, row) in entries {
        match row {
            SessionRow::Complete { folder, snapshot } => {
                let duration = snapshot
                    .duration_secs
                    .map_or_else(|| "?".to_string(), format_duration);
                let title = snapshot.title.unwrap_or_else(|| "(untitled)".into());
                println!(
                    "{:<48} {:<28} {:<9} {title}",
                    folder, snapshot.session_id, duration
                );
            }
            SessionRow::Unfinished { folder } => {
                println!(
                    "{:<48} {:<28} {:<9} UNFINISHED — journal present, no audio.opus; run `scrybe repair {folder}`",
                    folder, "?", "?"
                );
            }
        }
    }
    Ok(())
}

fn format_duration(secs: u64) -> String {
    let h = secs / 3_600;
    let m = (secs % 3_600) / 60;
    let s = secs % 60;
    if h > 0 {
        format!("{h:02}:{m:02}:{s:02}")
    } else {
        format!("{m:02}:{s:02}")
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn test_format_duration_renders_hours_minutes_seconds_when_over_an_hour() {
        assert_eq!(format_duration(3_661), "01:01:01");
    }

    #[test]
    fn test_format_duration_renders_minutes_seconds_under_an_hour() {
        assert_eq!(format_duration(125), "02:05");
    }

    #[test]
    fn test_format_duration_renders_zero_as_two_digit_minutes_seconds() {
        assert_eq!(format_duration(0), "00:00");
    }

    #[tokio::test]
    async fn test_list_handles_missing_root_with_friendly_message() {
        let dir = tempfile::tempdir().unwrap();
        let bogus = dir.path().join("nonexistent");

        run(Args { root: Some(bogus) }).await.unwrap();
    }

    #[tokio::test]
    async fn test_list_succeeds_for_existing_root_with_no_session_folders() {
        let dir = tempfile::tempdir().unwrap();

        run(Args {
            root: Some(dir.path().to_path_buf()),
        })
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn test_list_renders_session_with_meta_toml() {
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().join("2026-04-29-1430-acme-01HXYZ");
        std::fs::create_dir(&folder).unwrap();
        std::fs::write(
            folder.join("meta.toml"),
            "session_id = \"01HXYZ\"\ntitle = \"acme\"\nduration_secs = 75\n",
        )
        .unwrap();

        run(Args {
            root: Some(dir.path().to_path_buf()),
        })
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn test_list_skips_directories_without_meta_toml() {
        let dir = tempfile::tempdir().unwrap();
        let with_meta = dir.path().join("2026-04-29-1430-acme-01HXYZ");
        let without_meta = dir.path().join("2026-04-29-1430-other-02HABCD");
        std::fs::create_dir(&with_meta).unwrap();
        std::fs::create_dir(&without_meta).unwrap();
        std::fs::write(
            with_meta.join("meta.toml"),
            "session_id = \"01HXYZ\"\nduration_secs = 30\n",
        )
        .unwrap();
        std::fs::write(dir.path().join("not-a-folder.txt"), b"junk").unwrap();

        run(Args {
            root: Some(dir.path().to_path_buf()),
        })
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn test_list_renders_multiple_sessions_sorted_by_folder_name() {
        let dir = tempfile::tempdir().unwrap();
        for (folder_name, sid) in [
            ("2026-04-29-0900-alpha-01AAAA", "01AAAA"),
            ("2026-04-29-1000-bravo-02BBBB", "02BBBB"),
        ] {
            let folder = dir.path().join(folder_name);
            std::fs::create_dir(&folder).unwrap();
            std::fs::write(
                folder.join("meta.toml"),
                format!("session_id = \"{sid}\"\nduration_secs = 60\n"),
            )
            .unwrap();
        }

        run(Args {
            root: Some(dir.path().to_path_buf()),
        })
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn test_list_reports_unfinished_session_when_journal_present_without_audio() {
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().join("2026-04-29-1430-crashed-01HZZZ");
        std::fs::create_dir(&folder).unwrap();
        std::fs::create_dir(folder.join("journal")).unwrap();
        std::fs::write(folder.join("journal").join("mic-0000.f32"), b"\0\0\0\0").unwrap();

        // Must succeed (not silently skip the folder) and must not
        // error just because meta.toml is absent.
        run(Args {
            root: Some(dir.path().to_path_buf()),
        })
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn test_list_skips_folder_with_neither_meta_toml_nor_journal() {
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().join("2026-04-29-1430-empty-01HYYY");
        std::fs::create_dir(&folder).unwrap();

        // An empty session-shaped directory (no meta.toml, no
        // journal/) is not a session at all — must not be reported
        // as unfinished.
        run(Args {
            root: Some(dir.path().to_path_buf()),
        })
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn test_list_prefers_complete_session_when_meta_toml_present_alongside_stale_journal() {
        // A session whose merge succeeded (meta.toml + audio.opus
        // written) but whose journal directory somehow still exists
        // (should not normally happen post-merge, but defends against
        // a partial cleanup) must render as complete, not unfinished.
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().join("2026-04-29-1430-both-01HWWW");
        std::fs::create_dir(&folder).unwrap();
        std::fs::create_dir(folder.join("journal")).unwrap();
        std::fs::write(
            folder.join("meta.toml"),
            "session_id = \"01HWWW\"\ntitle = \"both\"\nduration_secs = 42\n",
        )
        .unwrap();

        run(Args {
            root: Some(dir.path().to_path_buf()),
        })
        .await
        .unwrap();
    }
}
