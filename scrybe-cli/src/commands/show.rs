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

use crate::runtime::{expand_root, load_or_default_config};

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
    let folder = resolve_folder(&root, &args.id_or_folder)
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

fn resolve_folder(root: &std::path::Path, id_or_folder: &str) -> Result<PathBuf> {
    let direct = std::path::Path::new(id_or_folder);
    if direct.is_absolute() && direct.is_dir() {
        return Ok(direct.to_path_buf());
    }
    let direct_in_root = root.join(id_or_folder);
    if direct_in_root.is_dir() {
        return Ok(direct_in_root);
    }
    if !root.exists() {
        anyhow::bail!("storage root {} does not exist", root.display());
    }
    let entries = std::fs::read_dir(root).with_context(|| format!("reading {}", root.display()))?;
    let mut hits: Vec<PathBuf> = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let name = path
            .file_name()
            .map_or_else(String::new, |s| s.to_string_lossy().into_owned());
        if name.contains(id_or_folder) {
            hits.push(path);
        }
    }
    match hits.len() {
        0 => anyhow::bail!("no session matches {id_or_folder}"),
        1 => Ok(hits.remove(0)),
        n => {
            anyhow::bail!("session prefix {id_or_folder} matches {n} folders; please disambiguate")
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn test_resolve_folder_returns_path_when_folder_exists_in_root() {
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().join("2026-04-29-1430-test-01HXYZ");
        std::fs::create_dir(&folder).unwrap();

        let resolved = resolve_folder(dir.path(), "2026-04-29-1430-test-01HXYZ").unwrap();

        assert_eq!(resolved, folder);
    }

    #[test]
    fn test_resolve_folder_resolves_partial_prefix_unambiguously() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("2026-04-29-1430-acme-01HXYZ")).unwrap();
        std::fs::create_dir(dir.path().join("2026-04-30-0900-other-02HABCD")).unwrap();

        let resolved = resolve_folder(dir.path(), "acme").unwrap();

        assert!(resolved.to_string_lossy().contains("acme"));
    }

    #[test]
    fn test_resolve_folder_returns_error_for_no_matching_session() {
        let dir = tempfile::tempdir().unwrap();

        let err = resolve_folder(dir.path(), "missing").unwrap_err();

        assert!(err.to_string().contains("no session matches"));
    }

    #[test]
    fn test_resolve_folder_returns_error_when_prefix_is_ambiguous() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("2026-04-29-1430-acme-01HXYZ")).unwrap();
        std::fs::create_dir(dir.path().join("2026-04-29-1500-acme-02HABCD")).unwrap();

        let err = resolve_folder(dir.path(), "acme").unwrap_err();

        assert!(err.to_string().contains("matches 2 folders"));
    }

    #[test]
    fn test_resolve_folder_returns_path_when_absolute_dir_supplied() {
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().join("session-abs");
        std::fs::create_dir(&folder).unwrap();
        let unrelated = tempfile::tempdir().unwrap();

        let resolved = resolve_folder(unrelated.path(), folder.to_str().unwrap()).unwrap();

        assert_eq!(resolved, folder);
    }

    #[test]
    fn test_resolve_folder_returns_error_when_root_is_missing() {
        let dir = tempfile::tempdir().unwrap();
        let bogus = dir.path().join("nonexistent");

        let err = resolve_folder(&bogus, "anything").unwrap_err();

        assert!(err.to_string().contains("storage root"));
    }

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
}
