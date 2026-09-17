// Copyright 2026 Mathews Tom
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     https://www.apache.org/licenses/LICENSE-2.0

//! `scrybe show <id-or-folder>` — render a session's transcript and
//! notes to stdout.
//!
//! Session resolution, artifact availability, and paged transcript
//! reading belong to the shared application service. This module owns
//! the section headers, the ordering, and the recovery hint.

use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::Args as ClapArgs;
use scrybe_application::{PageRequest, SessionRef, MAX_PAGE_LIMIT};

use crate::runtime::application;

#[derive(ClapArgs, Debug)]
pub struct Args {
    /// Either a session-folder name relative to the storage root or the
    /// session's ULID/short prefix. Paths are not accepted: every
    /// session resolves beneath the configured storage root.
    pub id_or_folder: String,

    /// Override the storage root from config.
    #[arg(long)]
    pub root: Option<PathBuf>,

    /// Skip the transcript section. Useful when only notes are wanted.
    #[arg(long, default_value_t = false)]
    pub no_transcript: bool,
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

    if !args.no_transcript && detail.artifacts.transcript {
        println!(
            "=== transcript ({}): ===",
            folder.join("transcript.md").display()
        );
        let mut cursor = 0;
        loop {
            let page = repository
                .read_transcript_page(&detail.id, PageRequest::new(cursor, MAX_PAGE_LIMIT))?;
            for line in &page.lines {
                println!("{line}");
            }
            match page.next {
                Some(next) => cursor = next.line,
                None => break,
            }
        }
    }

    let notes = repository.read_notes(&detail.id)?;
    if let Some(markdown) = notes.markdown {
        println!("\n=== notes ({}): ===", folder.join("notes.md").display());
        print!("{markdown}");
        if !markdown.ends_with('\n') {
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
    async fn test_run_resolves_a_session_by_unambiguous_ulid_fragment() {
        let dir = tempfile::tempdir().unwrap();
        write_session(dir.path(), "2026-04-29-1430-acme-01HXYZ");

        run(Args {
            id_or_folder: "01HXYZ".into(),
            root: Some(dir.path().to_path_buf()),
            no_transcript: false,
        })
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn test_run_refuses_an_absolute_path_instead_of_reading_outside_the_root() {
        let dir = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        write_session(outside.path(), "2026-04-29-1430-secret-01HXYZ");

        let error = run(Args {
            id_or_folder: outside
                .path()
                .join("2026-04-29-1430-secret-01HXYZ")
                .display()
                .to_string(),
            root: Some(dir.path().to_path_buf()),
            no_transcript: false,
        })
        .await
        .unwrap_err();

        assert!(error.to_string().contains("resolving session"));
    }
}
