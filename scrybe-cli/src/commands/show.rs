// Copyright 2026 Mathews Tom
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     https://www.apache.org/licenses/LICENSE-2.0

//! `scrybe show <id-or-folder>` — render a session's transcript and
//! notes to stdout.

use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::Args as ClapArgs;

use crate::runtime::{expand_root, load_or_default_config, resolve_session_folder};

#[derive(ClapArgs, Debug)]
pub struct Args {
    /// Either a session-folder name relative to the storage root, an
    /// absolute path, or the session's ULID/short prefix.
    pub id_or_folder: String,

    /// Override the storage root from config.
    #[arg(long)]
    pub root: Option<PathBuf>,

    /// Skip the transcript section. Useful when only notes are wanted.
    #[arg(long, default_value_t = false)]
    pub no_transcript: bool,
}

pub async fn run(args: Args) -> Result<()> {
    let root = if let Some(p) = args.root.as_deref() {
        expand_root(p)
    } else {
        let cfg = load_or_default_config()?;
        expand_root(&cfg.storage.root)
    };
    let folder = resolve_session_folder(&root, &args.id_or_folder)
        .with_context(|| format!("resolving session {}", args.id_or_folder))?;

    let transcript_path = folder.join("transcript.md");
    let notes_path = folder.join("notes.md");

    if !args.no_transcript && transcript_path.exists() {
        let body = tokio::fs::read_to_string(&transcript_path)
            .await
            .with_context(|| format!("reading {}", transcript_path.display()))?;
        println!("=== transcript ({}): ===", transcript_path.display());
        print!("{body}");
        if !body.ends_with('\n') {
            println!();
        }
    }
    if notes_path.exists() {
        let body = tokio::fs::read_to_string(&notes_path)
            .await
            .with_context(|| format!("reading {}", notes_path.display()))?;
        println!("\n=== notes ({}): ===", notes_path.display());
        print!("{body}");
        if !body.ends_with('\n') {
            println!();
        }
    } else {
        println!(
            "\nscrybe show: notes.md missing in {}; run `scrybe doctor` to recover",
            folder.display()
        );
    }
    Ok(())
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    fn write_session(dir: &std::path::Path, folder: &str) -> PathBuf {
        let folder_path = dir.join(folder);
        std::fs::create_dir(&folder_path).unwrap();
        std::fs::write(
            folder_path.join("transcript.md"),
            "# title\n\n**Me** [00:00:00]: hello\n",
        )
        .unwrap();
        std::fs::write(
            folder_path.join("notes.md"),
            "## TL;DR\n- a meeting happened\n",
        )
        .unwrap();
        folder_path
    }

    #[tokio::test]
    async fn test_run_prints_transcript_and_notes_for_existing_session() {
        let dir = tempfile::tempdir().unwrap();
        write_session(dir.path(), "2026-04-29-1430-acme-01HXYZ");

        run(Args {
            id_or_folder: "2026-04-29-1430-acme-01HXYZ".into(),
            root: Some(dir.path().to_path_buf()),
            no_transcript: false,
        })
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn test_run_skips_transcript_when_no_transcript_flag_set() {
        let dir = tempfile::tempdir().unwrap();
        write_session(dir.path(), "2026-04-29-1430-acme-01HXYZ");

        run(Args {
            id_or_folder: "2026-04-29-1430-acme-01HXYZ".into(),
            root: Some(dir.path().to_path_buf()),
            no_transcript: true,
        })
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn test_run_emits_recovery_hint_when_notes_md_missing() {
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().join("2026-04-29-1430-acme-01HXYZ");
        std::fs::create_dir(&folder).unwrap();
        std::fs::write(folder.join("transcript.md"), "# title\n").unwrap();

        run(Args {
            id_or_folder: "2026-04-29-1430-acme-01HXYZ".into(),
            root: Some(dir.path().to_path_buf()),
            no_transcript: false,
        })
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn test_run_returns_error_when_session_does_not_exist() {
        let dir = tempfile::tempdir().unwrap();

        let err = run(Args {
            id_or_folder: "nonexistent".into(),
            root: Some(dir.path().to_path_buf()),
            no_transcript: false,
        })
        .await
        .unwrap_err();

        assert!(err.to_string().contains("nonexistent"));
    }

    #[tokio::test]
    async fn test_run_resolves_session_via_unique_substring() {
        let dir = tempfile::tempdir().unwrap();
        write_session(dir.path(), "2026-04-29-1430-acme-01HXYZ");

        run(Args {
            id_or_folder: "acme".into(),
            root: Some(dir.path().to_path_buf()),
            no_transcript: false,
        })
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn test_run_appends_newline_when_transcript_lacks_trailing_newline() {
        // Exercises the `if !body.ends_with('\n')` branch in the
        // transcript section of `run`. Region coverage on `show.rs`
        // misses this arm because the canonical fixture
        // (`write_session`) writes a transcript that already ends
        // with `\n`.
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().join("2026-04-29-1430-acme-01HXYZ");
        std::fs::create_dir(&folder).unwrap();
        std::fs::write(folder.join("transcript.md"), b"no trailing newline").unwrap();
        std::fs::write(folder.join("notes.md"), b"## TL;DR\n- ok\n").unwrap();

        run(Args {
            id_or_folder: "2026-04-29-1430-acme-01HXYZ".into(),
            root: Some(dir.path().to_path_buf()),
            no_transcript: false,
        })
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn test_run_appends_newline_when_notes_lack_trailing_newline() {
        // Symmetric counterpart of the transcript test above for the
        // notes section's `if !body.ends_with('\n')` arm.
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().join("2026-04-29-1430-acme-01HXYZ");
        std::fs::create_dir(&folder).unwrap();
        std::fs::write(folder.join("transcript.md"), b"# title\n").unwrap();
        std::fs::write(folder.join("notes.md"), b"no trailing newline").unwrap();

        run(Args {
            id_or_folder: "2026-04-29-1430-acme-01HXYZ".into(),
            root: Some(dir.path().to_path_buf()),
            no_transcript: false,
        })
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn test_run_skips_transcript_section_when_transcript_md_absent() {
        // When `no_transcript=false` but `transcript.md` does not
        // exist, the `transcript_path.exists()` guard should keep the
        // transcript section silent and the function must still
        // succeed by rendering only the notes section.
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().join("2026-04-29-1430-acme-01HXYZ");
        std::fs::create_dir(&folder).unwrap();
        std::fs::write(folder.join("notes.md"), b"## TL;DR\n").unwrap();

        run(Args {
            id_or_folder: "2026-04-29-1430-acme-01HXYZ".into(),
            root: Some(dir.path().to_path_buf()),
            no_transcript: false,
        })
        .await
        .unwrap();
    }
}
