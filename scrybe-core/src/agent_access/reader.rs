// Copyright 2026 Mathews Tom
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     https://www.apache.org/licenses/LICENSE-2.0

//! Read-only directory walk over `~/scrybe/<session>/` folders.
//!
//! Reuses the session-directory semantics `scrybe-cli`'s `list` and
//! `show` commands already implement (`docs/system-design.md` §8.1,
//! "Storage layout invariants"): a folder is a *finished* meeting once
//! `meta.toml` exists, and *unfinished* when `journal/` exists with no
//! `audio.opus` and no `meta.toml` — the crash/`SIGKILL` case `scrybe
//! repair` recovers. A folder matching neither shape is not a meeting
//! at all and is omitted from listings / reported as not found.
//!
//! [`MetaSnapshot`] parses only the fields this surface needs and does
//! not `#[serde(deny_unknown_fields)]`, so a future minor-version field
//! addition to `meta.toml` (permitted by the Tier-2 contract) never
//! breaks this reader — unrecognized keys are ignored rather than
//! rejected. `tests::test_meta_snapshot_parses_real_build_meta_toml_output`
//! exercises this against the actual production writer.

use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use super::fs::ReadOnlyFs;

const META_FILE: &str = "meta.toml";
const NOTES_FILE: &str = "notes.md";
const TRANSCRIPT_FILE: &str = "transcript.md";
const JOURNAL_DIR: &str = "journal";
const AUDIO_FILE: &str = "audio.opus";

/// Default row count for `list_recent_meetings` / `search_meetings`
/// when the caller does not supply `limit`.
pub const DEFAULT_LIST_LIMIT: usize = 20;

/// Upper bound on `limit`, regardless of what a caller requests.
///
/// Keeps a single tool call from forcing a read of every
/// notes/transcript file under a very large storage root.
pub const MAX_LIST_LIMIT: usize = 200;

/// Forward-compatible subset of `meta.toml`'s `MetaTomlV1` schema
/// (`session.rs`).
///
/// See the module docs for why this intentionally omits
/// `deny_unknown_fields`.
#[derive(Debug, Clone, Deserialize)]
struct MetaSnapshot {
    session_id: String,
    #[serde(default)]
    title: Option<String>,
    started_at: DateTime<Utc>,
    #[serde(default)]
    ended_at: Option<DateTime<Utc>>,
    #[serde(default)]
    duration_secs: Option<u64>,
}

/// Whether a session folder reached the end of `drive_session`'s
/// normal completion.
///
/// An unfinished session is always reported as such — never silently
/// served as though it were complete.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionStatus {
    Finished,
    Unfinished,
}

/// One row of `list_recent_meetings` / `search_meetings`, and the
/// payload of `get_meeting`.
///
/// `Unfinished` rows carry no metadata fields — there is nothing
/// durable to report yet.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MeetingSummary {
    pub folder: String,
    pub status: SessionStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_secs: Option<u64>,
}

impl MeetingSummary {
    const fn unfinished(folder: String) -> Self {
        Self {
            folder,
            status: SessionStatus::Unfinished,
            session_id: None,
            title: None,
            started_at: None,
            ended_at: None,
            duration_secs: None,
        }
    }

    fn finished(folder: String, meta: MetaSnapshot) -> Self {
        Self {
            folder,
            status: SessionStatus::Finished,
            session_id: Some(meta.session_id),
            title: meta.title,
            started_at: Some(meta.started_at),
            ended_at: meta.ended_at,
            duration_secs: meta.duration_secs,
        }
    }
}

/// Payload of `get_meeting_notes` / `get_meeting_transcript`.
///
/// `content` is `None` both when the meeting is unfinished (nothing
/// durable exists yet) and, defensively, when a finished session is
/// missing the requested file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MeetingContent {
    pub folder: String,
    pub status: SessionStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
}

#[derive(thiserror::Error, Debug)]
pub enum AgentAccessError {
    #[error("storage root does not exist: {}", root.display())]
    RootMissing { root: PathBuf },

    #[error("no meeting matches {id:?}")]
    NotFound { id: String },

    #[error("{id:?} matches {count} meetings; use the full folder name or ULID")]
    Ambiguous { id: String, count: usize },

    #[error("meeting identifier must not contain a path separator: {id:?}")]
    InvalidId { id: String },

    #[error("reading {}: {source}", path.display())]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("parsing {}: {source}", path.display())]
    Meta {
        path: PathBuf,
        #[source]
        source: Box<toml::de::Error>,
    },
}

enum Classification {
    Finished(MetaSnapshot),
    Unfinished,
}

/// Classifies `folder` per the invariant documented at module level.
///
/// `Ok(None)` means `folder` is neither a finished nor an unfinished
/// meeting (e.g. an empty or foreign directory) and should be treated
/// as though it does not exist.
fn classify(
    fs: &dyn ReadOnlyFs,
    folder: &Path,
) -> Result<Option<Classification>, AgentAccessError> {
    let meta_path = folder.join(META_FILE);
    if fs.exists(&meta_path) {
        let body = fs
            .read_to_string(&meta_path)
            .map_err(|source| AgentAccessError::Io {
                path: meta_path.clone(),
                source,
            })?;
        let snapshot: MetaSnapshot =
            toml::from_str(&body).map_err(|source| AgentAccessError::Meta {
                path: meta_path,
                source: Box::new(source),
            })?;
        return Ok(Some(Classification::Finished(snapshot)));
    }
    let journal_dir = folder.join(JOURNAL_DIR);
    let audio_path = folder.join(AUDIO_FILE);
    if fs.exists(&journal_dir) && !fs.exists(&audio_path) {
        return Ok(Some(Classification::Unfinished));
    }
    Ok(None)
}

/// Resolves `id` to a session folder directly under `root`.
///
/// Accepts a bare folder name or an unambiguous substring of one
/// (typically the trailing ULID or a prefix of it) — never an
/// absolute path or one containing a path separator, so an agent
/// cannot use this surface to read anything outside `root`.
///
/// # Errors
///
/// - [`AgentAccessError::InvalidId`] if `id` contains a path separator
///   or is `.`/`..`.
/// - [`AgentAccessError::RootMissing`] if `root` does not exist.
/// - [`AgentAccessError::NotFound`] if no subdirectory matches.
/// - [`AgentAccessError::Ambiguous`] if more than one subdirectory
///   matches.
fn resolve_meeting_folder(
    fs: &dyn ReadOnlyFs,
    root: &Path,
    id: &str,
) -> Result<PathBuf, AgentAccessError> {
    if id.is_empty() || id.contains('/') || id.contains('\\') || id == "." || id == ".." {
        return Err(AgentAccessError::InvalidId { id: id.to_string() });
    }
    if !fs.exists(root) {
        return Err(AgentAccessError::RootMissing {
            root: root.to_path_buf(),
        });
    }
    let names = fs
        .subdirectories(root)
        .map_err(|source| AgentAccessError::Io {
            path: root.to_path_buf(),
            source,
        })?;
    if names.iter().any(|name| name == id) {
        return Ok(root.join(id));
    }
    let hits: Vec<&str> = names
        .iter()
        .filter(|name| name.contains(id))
        .map(String::as_str)
        .collect();
    match hits.len() {
        0 => Err(AgentAccessError::NotFound { id: id.to_string() }),
        1 => Ok(root.join(hits[0])),
        count => Err(AgentAccessError::Ambiguous {
            id: id.to_string(),
            count,
        }),
    }
}

/// Every session folder under `root` that is either finished or
/// unfinished, in filesystem-reported order (not sorted).
fn list_sessions(
    fs: &dyn ReadOnlyFs,
    root: &Path,
) -> Result<Vec<MeetingSummary>, AgentAccessError> {
    if !fs.exists(root) {
        return Err(AgentAccessError::RootMissing {
            root: root.to_path_buf(),
        });
    }
    let names = fs
        .subdirectories(root)
        .map_err(|source| AgentAccessError::Io {
            path: root.to_path_buf(),
            source,
        })?;
    let mut out = Vec::new();
    for name in names {
        let folder = root.join(&name);
        match classify(fs, &folder)? {
            Some(Classification::Finished(meta)) => out.push(MeetingSummary::finished(name, meta)),
            Some(Classification::Unfinished) => out.push(MeetingSummary::unfinished(name)),
            None => {}
        }
    }
    Ok(out)
}

fn clamp_limit(limit: usize) -> usize {
    limit.clamp(1, MAX_LIST_LIMIT)
}

/// The `limit` most recently started meetings under `root`, most
/// recent first.
///
/// Session folder names are `YYYY-MM-DD-HHMM-...`, so
/// reverse-lexicographic order on the folder name is chronological
/// order — this holds for unfinished sessions too, which carry no
/// `meta.toml` to sort by otherwise.
///
/// # Errors
///
/// See [`list_sessions`].
pub fn list_recent_meetings(
    fs: &dyn ReadOnlyFs,
    root: &Path,
    limit: usize,
) -> Result<Vec<MeetingSummary>, AgentAccessError> {
    let mut all = list_sessions(fs, root)?;
    all.sort_by(|a, b| b.folder.cmp(&a.folder));
    all.truncate(clamp_limit(limit));
    Ok(all)
}

/// Case-insensitive substring search over meeting title, folder name,
/// and — for finished meetings only — `notes.md` and `transcript.md`
/// content.
///
/// Unfinished meetings can still match by folder name (so
/// they remain discoverable, per the unfinished-session invariant)
/// but their content is never scanned or served through search.
///
/// # Errors
///
/// See [`list_sessions`].
pub fn search_sessions(
    fs: &dyn ReadOnlyFs,
    root: &Path,
    query: &str,
    limit: usize,
) -> Result<Vec<MeetingSummary>, AgentAccessError> {
    let needle = query.to_lowercase();
    let mut hits = Vec::new();
    for summary in list_sessions(fs, root)? {
        let folder_matches = summary.folder.to_lowercase().contains(&needle);
        let title_matches = summary
            .title
            .as_deref()
            .is_some_and(|t| t.to_lowercase().contains(&needle));
        let content_matches = summary.status == SessionStatus::Finished && {
            let folder = root.join(&summary.folder);
            file_contains(fs, &folder.join(NOTES_FILE), &needle)
                || file_contains(fs, &folder.join(TRANSCRIPT_FILE), &needle)
        };
        if folder_matches || title_matches || content_matches {
            hits.push(summary);
        }
    }
    hits.sort_by(|a, b| b.folder.cmp(&a.folder));
    hits.truncate(clamp_limit(limit));
    Ok(hits)
}

fn file_contains(fs: &dyn ReadOnlyFs, path: &Path, needle_lower: &str) -> bool {
    fs.read_to_string(path)
        .is_ok_and(|body| body.to_lowercase().contains(needle_lower))
}

/// Fetches metadata for one meeting by folder name or unambiguous
/// prefix.
///
/// # Errors
///
/// See [`resolve_meeting_folder`]; also [`AgentAccessError::NotFound`]
/// if the resolved folder is neither finished nor unfinished.
pub fn get_meeting(
    fs: &dyn ReadOnlyFs,
    root: &Path,
    id: &str,
) -> Result<MeetingSummary, AgentAccessError> {
    let folder = resolve_meeting_folder(fs, root, id)?;
    let name = folder_name(&folder, id);
    match classify(fs, &folder)? {
        Some(Classification::Finished(meta)) => Ok(MeetingSummary::finished(name, meta)),
        Some(Classification::Unfinished) => Ok(MeetingSummary::unfinished(name)),
        None => Err(AgentAccessError::NotFound { id: id.to_string() }),
    }
}

/// Fetches `notes.md` for one meeting. `content` is `None` when the
/// meeting is unfinished.
///
/// # Errors
///
/// See [`get_meeting`].
pub fn get_meeting_notes(
    fs: &dyn ReadOnlyFs,
    root: &Path,
    id: &str,
) -> Result<MeetingContent, AgentAccessError> {
    get_meeting_file(fs, root, id, NOTES_FILE)
}

/// Fetches `transcript.md` for one meeting. `content` is `None` when
/// the meeting is unfinished.
///
/// # Errors
///
/// See [`get_meeting`].
pub fn get_meeting_transcript(
    fs: &dyn ReadOnlyFs,
    root: &Path,
    id: &str,
) -> Result<MeetingContent, AgentAccessError> {
    get_meeting_file(fs, root, id, TRANSCRIPT_FILE)
}

fn get_meeting_file(
    fs: &dyn ReadOnlyFs,
    root: &Path,
    id: &str,
    file_name: &str,
) -> Result<MeetingContent, AgentAccessError> {
    let folder = resolve_meeting_folder(fs, root, id)?;
    let name = folder_name(&folder, id);
    match classify(fs, &folder)? {
        None => Err(AgentAccessError::NotFound { id: id.to_string() }),
        Some(Classification::Unfinished) => Ok(MeetingContent {
            folder: name,
            status: SessionStatus::Unfinished,
            content: None,
        }),
        Some(Classification::Finished(_)) => {
            let path = folder.join(file_name);
            let content = if fs.exists(&path) {
                Some(
                    fs.read_to_string(&path)
                        .map_err(|source| AgentAccessError::Io { path, source })?,
                )
            } else {
                None
            };
            Ok(MeetingContent {
                folder: name,
                status: SessionStatus::Finished,
                content,
            })
        }
    }
}

fn folder_name(folder: &Path, fallback: &str) -> String {
    folder
        .file_name()
        .and_then(|s| s.to_str())
        .map_or_else(|| fallback.to_string(), ToString::to_string)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use crate::agent_access::fs::RealReadOnlyFs;
    use crate::agent_access::test_support::{write_finished_session, write_unfinished_session};
    use crate::types::{ConsentAttestation, ConsentMode};

    fn fs() -> RealReadOnlyFs {
        RealReadOnlyFs
    }

    #[test]
    fn test_list_sessions_reports_finished_and_unfinished_rows() {
        let dir = tempfile::tempdir().unwrap();
        write_finished_session(
            dir.path(),
            "2026-01-01-0900-standup-01AAA",
            "Standup",
            120,
            "notes body",
            "transcript body",
        );
        write_unfinished_session(dir.path(), "2026-01-02-0900-crashed-01BBB");

        let mut rows = list_sessions(&fs(), dir.path()).unwrap();
        rows.sort_by(|a, b| a.folder.cmp(&b.folder));

        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].status, SessionStatus::Finished);
        assert_eq!(rows[0].title.as_deref(), Some("Standup"));
        assert_eq!(rows[0].duration_secs, Some(120));
        assert_eq!(rows[1].status, SessionStatus::Unfinished);
        assert_eq!(rows[1].session_id, None);
    }

    #[test]
    fn test_list_sessions_skips_folder_with_neither_meta_nor_journal() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("stray-empty-dir")).unwrap();

        let rows = list_sessions(&fs(), dir.path()).unwrap();

        assert!(rows.is_empty());
    }

    #[test]
    fn test_list_sessions_errors_when_root_missing() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("does-not-exist");

        let err = list_sessions(&fs(), &root).unwrap_err();

        assert!(matches!(err, AgentAccessError::RootMissing { .. }));
    }

    #[test]
    fn test_list_recent_meetings_orders_most_recent_first_and_respects_limit() {
        let dir = tempfile::tempdir().unwrap();
        write_finished_session(
            dir.path(),
            "2026-01-01-0900-first-01AAA",
            "First",
            10,
            "",
            "",
        );
        write_finished_session(
            dir.path(),
            "2026-01-03-0900-third-01CCC",
            "Third",
            10,
            "",
            "",
        );
        write_finished_session(
            dir.path(),
            "2026-01-02-0900-second-01BBB",
            "Second",
            10,
            "",
            "",
        );

        let rows = list_recent_meetings(&fs(), dir.path(), 2).unwrap();

        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].title.as_deref(), Some("Third"));
        assert_eq!(rows[1].title.as_deref(), Some("Second"));
    }

    #[test]
    fn test_search_sessions_matches_title_case_insensitively() {
        let dir = tempfile::tempdir().unwrap();
        write_finished_session(
            dir.path(),
            "2026-01-01-0900-acme-01AAA",
            "Acme Discovery",
            10,
            "",
            "",
        );
        write_finished_session(
            dir.path(),
            "2026-01-02-0900-other-01BBB",
            "Unrelated",
            10,
            "",
            "",
        );

        let hits = search_sessions(&fs(), dir.path(), "ACME", 10).unwrap();

        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].title.as_deref(), Some("Acme Discovery"));
    }

    #[test]
    fn test_search_sessions_matches_notes_content() {
        let dir = tempfile::tempdir().unwrap();
        write_finished_session(
            dir.path(),
            "2026-01-01-0900-standup-01AAA",
            "Standup",
            10,
            "Action item: renew the widget contract.",
            "",
        );

        let hits = search_sessions(&fs(), dir.path(), "widget contract", 10).unwrap();

        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].folder, "2026-01-01-0900-standup-01AAA");
    }

    #[test]
    fn test_search_sessions_matches_unfinished_folder_name_without_content() {
        let dir = tempfile::tempdir().unwrap();
        write_unfinished_session(dir.path(), "2026-01-01-0900-crashed-call-01AAA");

        let hits = search_sessions(&fs(), dir.path(), "crashed-call", 10).unwrap();

        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].status, SessionStatus::Unfinished);
    }

    #[test]
    fn test_get_meeting_returns_finished_metadata() {
        let dir = tempfile::tempdir().unwrap();
        write_finished_session(
            dir.path(),
            "2026-01-01-0900-standup-01AAA",
            "Standup",
            60,
            "",
            "",
        );

        let meeting = get_meeting(&fs(), dir.path(), "2026-01-01-0900-standup-01AAA").unwrap();

        assert_eq!(meeting.status, SessionStatus::Finished);
        assert_eq!(
            meeting.session_id.as_deref(),
            Some("2026-01-01-0900-standup-01AAA")
        );
        assert_eq!(meeting.duration_secs, Some(60));
    }

    #[test]
    fn test_get_meeting_reports_unfinished_status_without_serving_meta_fields() {
        let dir = tempfile::tempdir().unwrap();
        write_unfinished_session(dir.path(), "2026-01-01-0900-crashed-01AAA");

        let meeting = get_meeting(&fs(), dir.path(), "2026-01-01-0900-crashed-01AAA").unwrap();

        assert_eq!(meeting.status, SessionStatus::Unfinished);
        assert_eq!(meeting.session_id, None);
        assert_eq!(meeting.title, None);
        assert_eq!(meeting.started_at, None);
        assert_eq!(meeting.duration_secs, None);
    }

    #[test]
    fn test_get_meeting_errors_not_found_for_unknown_id() {
        let dir = tempfile::tempdir().unwrap();

        let err = get_meeting(&fs(), dir.path(), "nothing-here").unwrap_err();

        assert!(matches!(err, AgentAccessError::NotFound { .. }));
    }

    #[test]
    fn test_get_meeting_errors_not_found_for_empty_directory() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("empty-dir")).unwrap();

        let err = get_meeting(&fs(), dir.path(), "empty-dir").unwrap_err();

        assert!(matches!(err, AgentAccessError::NotFound { .. }));
    }

    #[test]
    fn test_get_meeting_notes_returns_content_for_finished_session() {
        let dir = tempfile::tempdir().unwrap();
        write_finished_session(
            dir.path(),
            "2026-01-01-0900-standup-01AAA",
            "Standup",
            10,
            "# Notes\n\nDiscussed roadmap.",
            "",
        );

        let notes = get_meeting_notes(&fs(), dir.path(), "2026-01-01-0900-standup-01AAA").unwrap();

        assert_eq!(notes.status, SessionStatus::Finished);
        assert_eq!(
            notes.content.as_deref(),
            Some("# Notes\n\nDiscussed roadmap.")
        );
    }

    #[test]
    fn test_get_meeting_notes_returns_none_content_for_unfinished_session() {
        let dir = tempfile::tempdir().unwrap();
        write_unfinished_session(dir.path(), "2026-01-01-0900-crashed-01AAA");

        let notes = get_meeting_notes(&fs(), dir.path(), "2026-01-01-0900-crashed-01AAA").unwrap();

        assert_eq!(notes.status, SessionStatus::Unfinished);
        assert_eq!(notes.content, None);
    }

    #[test]
    fn test_get_meeting_notes_returns_none_when_file_missing_on_finished_session() {
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().join("2026-01-01-0900-standup-01AAA");
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::write(
            folder.join("meta.toml"),
            "session_id = \"01AAA\"\nstarted_at = \"2026-01-01T09:00:00Z\"\n",
        )
        .unwrap();

        let notes = get_meeting_notes(&fs(), dir.path(), "2026-01-01-0900-standup-01AAA").unwrap();

        assert_eq!(notes.status, SessionStatus::Finished);
        assert_eq!(notes.content, None);
    }

    #[test]
    fn test_get_meeting_transcript_returns_content_for_finished_session() {
        let dir = tempfile::tempdir().unwrap();
        write_finished_session(
            dir.path(),
            "2026-01-01-0900-standup-01AAA",
            "Standup",
            10,
            "",
            "**Me** [00:00:01]: hello\n",
        );

        let transcript =
            get_meeting_transcript(&fs(), dir.path(), "2026-01-01-0900-standup-01AAA").unwrap();

        assert_eq!(
            transcript.content.as_deref(),
            Some("**Me** [00:00:01]: hello\n")
        );
    }

    #[test]
    fn test_resolve_meeting_folder_rejects_path_separators() {
        let dir = tempfile::tempdir().unwrap();

        let err = resolve_meeting_folder(&fs(), dir.path(), "../etc/passwd").unwrap_err();

        assert!(matches!(err, AgentAccessError::InvalidId { .. }));
    }

    #[test]
    fn test_resolve_meeting_folder_rejects_dot_dot() {
        let dir = tempfile::tempdir().unwrap();

        let err = resolve_meeting_folder(&fs(), dir.path(), "..").unwrap_err();

        assert!(matches!(err, AgentAccessError::InvalidId { .. }));
    }

    #[test]
    fn test_resolve_meeting_folder_errors_on_ambiguous_prefix() {
        let dir = tempfile::tempdir().unwrap();
        write_finished_session(
            dir.path(),
            "2026-01-01-0900-alpha-01AAA",
            "Alpha",
            10,
            "",
            "",
        );
        write_finished_session(
            dir.path(),
            "2026-01-02-0900-alpha-01BBB",
            "Alpha 2",
            10,
            "",
            "",
        );

        let err = resolve_meeting_folder(&fs(), dir.path(), "alpha").unwrap_err();

        assert!(matches!(err, AgentAccessError::Ambiguous { count: 2, .. }));
    }

    #[test]
    fn test_resolve_meeting_folder_matches_ulid_suffix_uniquely() {
        let dir = tempfile::tempdir().unwrap();
        write_finished_session(
            dir.path(),
            "2026-01-01-0900-standup-01AAAUNIQUE",
            "Standup",
            10,
            "",
            "",
        );

        let folder = resolve_meeting_folder(&fs(), dir.path(), "01AAAUNIQUE").unwrap();

        assert_eq!(
            folder,
            dir.path().join("2026-01-01-0900-standup-01AAAUNIQUE")
        );
    }

    #[test]
    fn test_meta_snapshot_parses_real_build_meta_toml_output() {
        // Proves forward/backward compatibility against the actual
        // production writer, not just this test module's own fixture
        // format: `build_meta_toml` writes `consent`, `providers`, and
        // `scrybe` tables this reader does not declare — they must be
        // ignored, not rejected.
        let attestation = ConsentAttestation::new(ConsentMode::Quick, "tester");
        let now = Utc::now();
        let body = crate::session::build_meta_toml(crate::session::MetaArgs {
            id: crate::types::SessionId::new(),
            title: Some("Real Writer Session"),
            started_at: now,
            ended_at: now,
            attestation: &attestation,
            stt_name: "whisper-local",
            llm_name: "stub",
            diarizer_name: "binary-channel",
            audio: None,
        })
        .unwrap();

        let snapshot: MetaSnapshot = toml::from_str(&body).unwrap();

        assert_eq!(snapshot.title.as_deref(), Some("Real Writer Session"));
    }
}
