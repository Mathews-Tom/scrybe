// Copyright 2026 Mathews Tom
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     https://www.apache.org/licenses/LICENSE-2.0

//! `scrybe list` — folder listing of the configured root.
//!
//! The scan, classification, and metadata reading are the shared
//! application service's. What stays here is presentation: column
//! layout, oldest-first ordering, and the recovery hint each
//! incomplete session gets.

use std::path::PathBuf;

use anyhow::Result;
use clap::Args as ClapArgs;
use scrybe_application::sessions::{SessionState, SessionSummary};
use scrybe_application::{ErrorCode, PageRequest, SessionRepository, MAX_PAGE_LIMIT};

use crate::runtime::application;

#[derive(ClapArgs, Debug)]
pub struct Args {
    /// Override the storage root from config.
    #[arg(long)]
    pub root: Option<PathBuf>,
}

#[allow(clippy::unused_async)]
pub async fn run(args: Args) -> Result<()> {
    let app = application(args.root.as_deref())?;
    let repository = app.sessions();
    let root = repository.root().path().display().to_string();

    let sessions = match collect(repository) {
        Ok(sessions) => sessions,
        Err(error) if error.code() == ErrorCode::StorageRootMissing => {
            println!("scrybe list: no sessions found (root {root} does not exist)");
            return Ok(());
        }
        Err(error) => return Err(error.into()),
    };

    if sessions.is_empty() {
        println!("scrybe list: no sessions in {root}");
        return Ok(());
    }

    println!("{:<48} {:<28} duration  title", "folder", "session_id");
    for session in sessions {
        render(&session);
    }
    Ok(())
}

/// Every session under the root, oldest first.
///
/// The repository orders most-recent-first, which is what a detail view
/// wants; this command has always printed oldest-first, so the order is
/// reversed here rather than in the service.
fn collect(
    repository: &SessionRepository,
) -> std::result::Result<Vec<SessionSummary>, scrybe_application::ApplicationError> {
    let mut sessions = Vec::new();
    let mut offset = 0;
    loop {
        let page = repository.list_sessions(PageRequest::new(offset, MAX_PAGE_LIMIT))?;
        let more = page.has_more;
        offset += page.items.len();
        sessions.extend(page.items);
        if !more {
            break;
        }
    }
    sessions.reverse();
    Ok(sessions)
}

fn render(session: &SessionSummary) {
    println!("{}", render_line(session));
}

fn render_line(session: &SessionSummary) -> String {
    let folder = session.id.as_str();
    match session.state {
        SessionState::Complete => {
            let duration = session
                .duration_secs
                .map_or_else(|| "?".to_string(), format_duration);
            let title = session.title.clone().unwrap_or_else(|| "(untitled)".into());
            let id = session.session_id.as_deref().unwrap_or("?");
            format!("{folder:<48} {id:<28} {duration:<9} {title}")
        }
        // A session is repairable by either of two routes — a journal
        // with its manifest, or already-merged `audio.opus` missing
        // only its metadata — and a summary does not say which. The
        // hint names both rather than asserting one, because claiming
        // "no audio.opus" is precisely wrong for the audio route.
        SessionState::Repairable => format!(
            "{:<48} {:<28} {:<9} UNFINISHED — a journal or merged audio survives; run `scrybe repair {folder}`",
            folder, "?", "?"
        ),
        SessionState::Unfinished => format!(
            "{:<48} {:<28} {:<9} UNFINISHED — nothing durable to recover; run `scrybe doctor`",
            folder, "?", "?"
        ),
        SessionState::Failed => format!(
            "{:<48} {:<28} {:<9} FAILED — durable state is unreadable; inspect {folder}",
            folder, "?", "?"
        ),
    }
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
    fn test_format_duration_renders_minutes_seconds_when_under_an_hour() {
        assert_eq!(format_duration(61), "01:01");
    }

    #[tokio::test]
    async fn test_run_reports_a_missing_root_without_failing() {
        let dir = tempfile::tempdir().unwrap();

        run(Args {
            root: Some(dir.path().join("absent")),
        })
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn test_run_lists_a_fixture_tree_oldest_first() {
        let dir = tempfile::tempdir().unwrap();
        for folder in ["2026-04-29-1430-beta-01BBB", "2026-04-01-0900-alpha-01AAA"] {
            let path = dir.path().join(folder);
            std::fs::create_dir_all(&path).unwrap();
            std::fs::write(
                path.join("meta.toml"),
                "session_id = \"01AAA\"\n\
                 title = \"Fixture\"\n\
                 started_at = \"2026-04-29T14:30:00Z\"\n\
                 ended_at = \"2026-04-29T15:00:00Z\"\n\
                 duration_secs = 1800\n",
            )
            .unwrap();
        }

        let app = application(Some(dir.path())).unwrap();
        let sessions = collect(app.sessions()).unwrap();

        assert_eq!(
            sessions
                .iter()
                .map(|session| session.id.as_str())
                .collect::<Vec<_>>(),
            vec!["2026-04-01-0900-alpha-01AAA", "2026-04-29-1430-beta-01BBB"]
        );
    }

    fn only_session(root: &std::path::Path) -> SessionSummary {
        let repository = session_repository(Some(root)).unwrap();
        let mut sessions = collect(&repository).unwrap();

        assert_eq!(sessions.len(), 1);
        sessions.remove(0)
    }

    #[test]
    fn test_a_session_repairable_from_its_journal_is_offered_repair() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("2026-04-29-1430-acme-01HXYZ");
        std::fs::create_dir_all(path.join("journal")).unwrap();
        std::fs::write(path.join("journal/manifest.toml"), "").unwrap();

        let line = render_line(&only_session(dir.path()));

        assert!(
            line.contains("a journal or merged audio survives"),
            "{line}"
        );
        assert!(
            line.contains("scrybe repair 2026-04-29-1430-acme-01HXYZ"),
            "{line}"
        );
    }

    #[test]
    fn test_a_session_repairable_from_its_audio_is_not_told_audio_is_missing() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("2026-04-29-1430-acme-01HXYZ");
        std::fs::create_dir_all(&path).unwrap();
        std::fs::write(path.join("audio.opus"), "").unwrap();

        let session = only_session(dir.path());
        let line = render_line(&session);

        assert_eq!(session.state, SessionState::Repairable);
        assert!(!line.contains("no audio.opus"), "{line}");
        assert!(
            line.contains("a journal or merged audio survives"),
            "{line}"
        );
    }
}
