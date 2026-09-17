// Copyright 2026 Mathews Tom
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     https://www.apache.org/licenses/LICENSE-2.0

//! The filesystem-backed session repository.
//!
//! Every read and every explicit mutation a frontend can perform on a
//! session goes through here. Three properties are structural rather
//! than conventional:
//!
//! - **Confinement.** A caller supplies a [`SessionRef`], never a path.
//!   Resolution only ever selects among folder names the repository
//!   itself read from the configured root, so a resolved session is a
//!   direct child of that root by construction.
//! - **Cancellation.** Search reads every complete session's notes and
//!   transcript. The token is checked between sessions, so an
//!   abandoned search stops within one session's worth of I/O.
//! - **Coalesced invalidation.** One cheap fingerprint pass over the
//!   root — the set of folder names and their modification times —
//!   decides whether the whole cached scan is still valid. External
//!   mutation by any other tool therefore invalidates the cache
//!   without a watcher, and a burst of reads between two mutations
//!   shares one scan instead of re-parsing every `meta.toml`.
//!
//! No lock is held across filesystem work. The cache mutex is taken to
//! read a snapshot and released before any I/O, then taken again to
//! store the result.

use std::error::Error as StdError;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use chrono::{DateTime, Utc};
use scrybe_core::repair::{repair_session, RepairOutcome};
use scrybe_core::storage::atomic_replace;

use crate::cancellation::CancellationToken;
use crate::error::{ApplicationError, ErrorCode};
use crate::identity::{SessionRef, StorageRoot};
use crate::paging::{Page, PageRequest};
use crate::sessions::contract::{
    NotesDocument, NotesGenerationOutcome, NotesGenerationResult, RepairOutcomeKind, RepairResult,
    SearchPage, SearchRequest, SessionDetail, SessionPage, SessionState, SessionSummary,
    TranscriptCursor, TranscriptPage,
};
use crate::sessions::scan::{
    classify, Classified, FolderView, ScannedSession, NOTES_FILE, TRANSCRIPT_FILE,
};
use crate::Result;

/// What a caller-supplied notes generator receives.
///
/// The repository owns resolution, eligibility, the atomic write, and
/// cache invalidation. Which provider produces the text, and under what
/// configuration, stays with the caller that already owns provider
/// construction.
#[derive(Debug)]
pub struct NotesGenerationRequest<'a> {
    /// The session's durable canonical transcript.
    pub transcript: &'a str,
    pub title: Option<&'a str>,
    pub started_at: Option<DateTime<Utc>>,
}

/// Produces a notes document from a durable transcript.
pub trait NotesGenerator {
    /// Renders notes markdown for `request`.
    ///
    /// # Errors
    ///
    /// Any generator failure. The repository surfaces it as
    /// [`ErrorCode::NotesGenerationFailed`] and retains this error as
    /// the internal source.
    fn generate(
        &self,
        request: &NotesGenerationRequest<'_>,
    ) -> std::result::Result<String, Box<dyn StdError + Send + Sync>>;
}

/// Identity of the storage root's observable state.
///
/// Folder names catch sessions appearing and disappearing;
/// modification times catch an artifact being created, replaced, or
/// removed inside one. Both are what any external write to a session
/// changes, including the atomic replaces the pipeline itself performs.
type RootFingerprint = Vec<(String, Option<SystemTime>)>;

struct CachedScan {
    fingerprint: RootFingerprint,
    sessions: Arc<Vec<ScannedSession>>,
}

/// Filesystem-backed access to the sessions under one storage root.
pub struct SessionRepository {
    root: StorageRoot,
    cache: Mutex<Option<CachedScan>>,
}

impl SessionRepository {
    /// A repository over `root`.
    #[must_use]
    pub const fn new(root: StorageRoot) -> Self {
        Self {
            root,
            cache: Mutex::new(None),
        }
    }

    /// The configured storage root.
    #[must_use]
    pub const fn root(&self) -> &StorageRoot {
        &self.root
    }

    /// One page of sessions, most recently started first.
    ///
    /// Session folder names begin `YYYY-MM-DD-HHMM`, so
    /// reverse-lexicographic folder order is chronological order. This
    /// holds for sessions with no metadata too, which have no timestamp
    /// to sort by otherwise.
    ///
    /// # Errors
    ///
    /// [`ErrorCode::StorageRootMissing`] when the root does not exist,
    /// [`ErrorCode::StorageUnavailable`] when it cannot be read.
    pub fn list_sessions(&self, page: PageRequest) -> Result<SessionPage> {
        let sessions = self.scan()?;
        let rows = sessions.iter().map(summarize).collect();
        Ok(Page::paginate(rows, page))
    }

    /// One page of sessions matching `request`, most recent first.
    ///
    /// Matches folder name and title for every session, and notes and
    /// transcript text for complete ones. Content belonging to a
    /// session that never completed is never scanned or served.
    ///
    /// # Errors
    ///
    /// [`ErrorCode::Cancelled`] if `cancel` fires before the search
    /// finishes; otherwise as [`Self::list_sessions`].
    pub fn search_sessions(
        &self,
        request: &SearchRequest,
        cancel: &CancellationToken,
    ) -> Result<SearchPage> {
        let sessions = self.scan()?;
        let needle = request.query.to_lowercase();
        let mut hits = Vec::new();
        for session in sessions.iter() {
            if cancel.is_cancelled() {
                return Err(ApplicationError::new(
                    ErrorCode::Cancelled,
                    "search was cancelled before it completed",
                ));
            }
            if self.matches(session, &needle) {
                hits.push(summarize(session));
            }
        }
        Ok(Page::paginate(hits, request.page))
    }

    /// Everything a detail view needs about one session.
    ///
    /// # Errors
    ///
    /// [`ErrorCode::SessionNotFound`] or
    /// [`ErrorCode::AmbiguousSessionId`] from resolution; otherwise as
    /// [`Self::list_sessions`].
    pub fn get_session(&self, id: &SessionRef) -> Result<SessionDetail> {
        let session = self.resolve(id)?;
        let meta = session.meta.as_ref();
        Ok(SessionDetail {
            id: session.id.clone(),
            state: session.state,
            session_id: meta.map(|meta| meta.session_id.clone()),
            title: meta.and_then(|meta| meta.title.clone()),
            started_at: meta.map(|meta| meta.started_at),
            ended_at: meta.and_then(|meta| meta.ended_at),
            duration_secs: meta.and_then(|meta| meta.duration_secs),
            artifacts: session.artifacts,
            capture: session.capture(),
            providers: session.providers(),
            eligibility: session.eligibility(),
        })
    }

    /// A session's durable notes.
    ///
    /// # Errors
    ///
    /// As [`Self::get_session`], plus
    /// [`ErrorCode::StorageUnavailable`] if the file exists but cannot
    /// be read.
    pub fn read_notes(&self, id: &SessionRef) -> Result<NotesDocument> {
        let session = self.resolve(id)?;
        let markdown = if session.artifacts.notes {
            Some(self.read_artifact(&session, NOTES_FILE)?)
        } else {
            None
        };
        Ok(NotesDocument {
            id: session.id.clone(),
            state: session.state,
            markdown,
        })
    }

    /// One window of a session's durable transcript.
    ///
    /// `page` is the cursor: its offset is the first transcript line
    /// and its limit is the window size. The response carries the
    /// cursor it started from and the cursor for the next window.
    ///
    /// # Errors
    ///
    /// As [`Self::read_notes`].
    pub fn read_transcript_page(
        &self,
        id: &SessionRef,
        page: PageRequest,
    ) -> Result<TranscriptPage> {
        let session = self.resolve(id)?;
        let body = if session.artifacts.transcript {
            self.read_artifact(&session, TRANSCRIPT_FILE)?
        } else {
            String::new()
        };
        let all: Vec<&str> = body.lines().collect();
        let total_lines = all.len();
        let start = page.offset().min(total_lines);
        let end = start.saturating_add(page.limit()).min(total_lines);
        Ok(TranscriptPage {
            id: session.id.clone(),
            state: session.state,
            cursor: TranscriptCursor::at(start),
            lines: all[start..end]
                .iter()
                .map(|line| (*line).to_string())
                .collect(),
            total_lines,
            next: if end < total_lines {
                Some(TranscriptCursor::at(end))
            } else {
                None
            },
        })
    }

    /// Recovers an interrupted session. An explicit mutation.
    ///
    /// # Errors
    ///
    /// [`ErrorCode::NotApplicable`] when the session is not repairable,
    /// [`ErrorCode::RepairFailed`] when recovery itself fails;
    /// otherwise as [`Self::get_session`].
    pub fn repair_session(&self, id: &SessionRef) -> Result<RepairResult> {
        let session = self.resolve(id)?;
        let canonical = session.id.clone();
        if !matches!(
            session.state,
            SessionState::Repairable | SessionState::Complete
        ) {
            return Err(ApplicationError::new(
                ErrorCode::NotApplicable,
                format!("session {canonical} has no durable state repair can recover"),
            ));
        }
        let folder = self.root.resolve(&canonical);
        let outcome = repair_session(&folder).map_err(|source| {
            ApplicationError::new(
                ErrorCode::RepairFailed,
                format!("recovering session {canonical} failed"),
            )
            .with_source(source)
        })?;
        self.invalidate();
        let report = match &outcome {
            RepairOutcome::Repaired(report) | RepairOutcome::MetadataReconstructed(report) => {
                Some(report)
            }
            RepairOutcome::NothingToRepair => None,
        };
        let refreshed = self.resolve(&canonical)?;
        Ok(RepairResult {
            id: canonical,
            outcome: RepairOutcomeKind::from(&outcome),
            state: refreshed.state,
            recovered_secs: report.map(|report| report.encoded_secs),
            channels: report.map(|report| report.channels),
            wrote_metadata: report.is_some_and(|report| report.wrote_meta),
        })
    }

    /// Replaces a session's notes from its durable transcript. An
    /// explicit mutation.
    ///
    /// # Errors
    ///
    /// [`ErrorCode::NotApplicable`] when the session has no durable
    /// transcript to regenerate from,
    /// [`ErrorCode::NotesGenerationFailed`] when `generator` fails, and
    /// [`ErrorCode::StorageUnavailable`] when the replacement cannot be
    /// written.
    pub fn regenerate_notes(
        &self,
        id: &SessionRef,
        generator: &dyn NotesGenerator,
    ) -> Result<NotesGenerationResult> {
        let session = self.resolve(id)?;
        let canonical = session.id.clone();
        if !session.eligibility().regenerate_notes {
            return Err(ApplicationError::new(
                ErrorCode::NotApplicable,
                format!("session {canonical} has no durable transcript to regenerate notes from"),
            ));
        }
        let transcript = self.read_artifact(&session, TRANSCRIPT_FILE)?;
        let meta = session.meta.as_ref();
        let markdown = generator
            .generate(&NotesGenerationRequest {
                transcript: &transcript,
                title: meta.and_then(|meta| meta.title.as_deref()),
                started_at: meta.map(|meta| meta.started_at),
            })
            .map_err(|source| {
                ApplicationError::new(
                    ErrorCode::NotesGenerationFailed,
                    format!("regenerating notes for session {canonical} failed"),
                )
                .with_source(BoxedGeneratorError(source))
            })?;

        let path = self.root.resolve(&canonical).join(NOTES_FILE);
        let previous = std::fs::read_to_string(&path).ok();
        if previous.as_deref() == Some(markdown.as_str()) {
            return Ok(NotesGenerationResult {
                id: canonical,
                outcome: NotesGenerationOutcome::Unchanged,
                bytes: markdown.len(),
            });
        }
        atomic_replace(&path, markdown.as_bytes()).map_err(|source| {
            ApplicationError::new(
                ErrorCode::StorageUnavailable,
                format!("replacing notes for session {canonical} failed"),
            )
            .with_source(source)
        })?;
        self.invalidate();
        Ok(NotesGenerationResult {
            id: canonical,
            outcome: NotesGenerationOutcome::Replaced,
            bytes: markdown.len(),
        })
    }

    /// Drops the cached scan, forcing the next read to rescan.
    fn invalidate(&self) {
        if let Ok(mut cache) = self.cache.lock() {
            *cache = None;
        }
    }

    /// The current scan, reusing the cached one when the root is
    /// unchanged.
    fn scan(&self) -> Result<Arc<Vec<ScannedSession>>> {
        let fingerprint = self.fingerprint()?;
        if let Ok(cache) = self.cache.lock() {
            if let Some(cached) = cache.as_ref() {
                if cached.fingerprint == fingerprint {
                    return Ok(Arc::clone(&cached.sessions));
                }
            }
        }

        let mut sessions = Vec::new();
        for (name, _) in &fingerprint {
            // A folder whose name cannot form a confined identity is
            // not addressable through this layer, so it is omitted
            // rather than listed as something no caller could open.
            let Ok(id) = SessionRef::parse(name) else {
                continue;
            };
            let folder = self.root.resolve(&id);
            if let Classified::Session(session) = classify(id, &DirectoryView { folder }) {
                sessions.push(*session);
            }
        }
        sessions.sort_by(|left, right| right.id.as_str().cmp(left.id.as_str()));
        let sessions = Arc::new(sessions);

        if let Ok(mut cache) = self.cache.lock() {
            *cache = Some(CachedScan {
                fingerprint,
                sessions: Arc::clone(&sessions),
            });
        }
        Ok(sessions)
    }

    /// The root's observable state, as folder names paired with their
    /// modification times.
    fn fingerprint(&self) -> Result<RootFingerprint> {
        let root = self.root.path();
        let entries = std::fs::read_dir(root).map_err(|source| {
            let code = if source.kind() == std::io::ErrorKind::NotFound {
                ErrorCode::StorageRootMissing
            } else {
                ErrorCode::StorageUnavailable
            };
            ApplicationError::new(
                code,
                format!("storage root {} could not be read", root.display()),
            )
            .with_source(source)
        })?;

        let mut fingerprint = RootFingerprint::new();
        for entry in entries.flatten() {
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            if !file_type.is_dir() {
                continue;
            }
            let Some(name) = entry.file_name().to_str().map(ToString::to_string) else {
                continue;
            };
            let modified = entry.metadata().and_then(|meta| meta.modified()).ok();
            fingerprint.push((name, modified));
        }
        fingerprint.sort();
        Ok(fingerprint)
    }

    /// Resolves an identity to exactly one scanned session.
    ///
    /// Accepts the complete folder name, or an unambiguous fragment of
    /// one — the same identifiers the CLI has always accepted. Because
    /// candidates come only from the scan, a resolved session is always
    /// under the configured root.
    fn resolve(&self, id: &SessionRef) -> Result<ScannedSession> {
        let sessions = self.scan()?;
        if let Some(exact) = sessions.iter().find(|session| session.id == *id) {
            return Ok(exact.clone());
        }
        let mut hits = sessions
            .iter()
            .filter(|session| session.id.as_str().contains(id.as_str()));
        match (hits.next(), hits.next()) {
            (None, _) => Err(ApplicationError::new(
                ErrorCode::SessionNotFound,
                format!("no session matches {id}"),
            )),
            (Some(only), None) => Ok(only.clone()),
            (Some(_), Some(_)) => {
                let count = sessions
                    .iter()
                    .filter(|session| session.id.as_str().contains(id.as_str()))
                    .count();
                Err(ApplicationError::new(
                    ErrorCode::AmbiguousSessionId,
                    format!("{id} matches {count} sessions; use the full session name"),
                ))
            }
        }
    }

    fn matches(&self, session: &ScannedSession, needle: &str) -> bool {
        if session.id.as_str().to_lowercase().contains(needle) {
            return true;
        }
        let title_matches = session
            .meta
            .as_ref()
            .and_then(|meta| meta.title.as_deref())
            .is_some_and(|title| title.to_lowercase().contains(needle));
        if title_matches {
            return true;
        }
        if !session.state.is_complete() {
            return false;
        }
        let folder = self.root.resolve(&session.id);
        contains(&folder.join(NOTES_FILE), needle)
            || contains(&folder.join(TRANSCRIPT_FILE), needle)
    }

    fn read_artifact(&self, session: &ScannedSession, file: &str) -> Result<String> {
        let path = self.root.resolve(&session.id).join(file);
        std::fs::read_to_string(&path).map_err(|source| {
            ApplicationError::new(
                ErrorCode::StorageUnavailable,
                format!("reading {file} for session {} failed", session.id),
            )
            .with_source(source)
        })
    }
}

/// Wrapper giving a boxed generator error the concrete `Error` type
/// `ApplicationError::with_source` needs.
#[derive(Debug)]
struct BoxedGeneratorError(Box<dyn StdError + Send + Sync>);

impl std::fmt::Display for BoxedGeneratorError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

impl StdError for BoxedGeneratorError {
    fn source(&self) -> Option<&(dyn StdError + 'static)> {
        self.0.source()
    }
}

struct DirectoryView {
    folder: PathBuf,
}

impl FolderView for DirectoryView {
    fn exists(&self, relative: &str) -> bool {
        self.folder.join(relative).exists()
    }

    fn read(&self, relative: &str) -> Option<String> {
        std::fs::read_to_string(self.folder.join(relative)).ok()
    }
}

fn contains(path: &Path, needle: &str) -> bool {
    std::fs::read_to_string(path).is_ok_and(|body| body.to_lowercase().contains(needle))
}

fn summarize(session: &ScannedSession) -> SessionSummary {
    let meta = session.meta.as_ref();
    SessionSummary {
        id: session.id.clone(),
        state: session.state,
        session_id: meta.map(|meta| meta.session_id.clone()),
        title: meta.and_then(|meta| meta.title.clone()),
        started_at: meta.map(|meta| meta.started_at),
        ended_at: meta.and_then(|meta| meta.ended_at),
        duration_secs: meta.and_then(|meta| meta.duration_secs),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    struct Tree {
        dir: tempfile::TempDir,
    }

    impl Tree {
        fn new() -> Self {
            Self {
                dir: tempfile::tempdir().unwrap(),
            }
        }

        fn root(&self) -> StorageRoot {
            StorageRoot::new(self.dir.path())
        }

        fn repository(&self) -> SessionRepository {
            SessionRepository::new(self.root())
        }

        fn complete(&self, folder: &str, title: &str) -> &Self {
            let path = self.dir.path().join(folder);
            std::fs::create_dir_all(&path).unwrap();
            std::fs::write(
                path.join("meta.toml"),
                format!(
                    "session_id = \"01AAA\"\n\
                     title = \"{title}\"\n\
                     started_at = \"2026-04-29T14:30:00Z\"\n\
                     ended_at = \"2026-04-29T15:00:00Z\"\n\
                     duration_secs = 1800\n"
                ),
            )
            .unwrap();
            std::fs::write(path.join("audio.opus"), b"").unwrap();
            std::fs::write(
                path.join("transcript.md"),
                "# t\nhello world\nsecond line\n",
            )
            .unwrap();
            std::fs::write(path.join("notes.md"), "## TL;DR\n- covered widgets\n").unwrap();
            self
        }

        fn journal_only(&self, folder: &str) -> &Self {
            let path = self.dir.path().join(folder).join("journal");
            std::fs::create_dir_all(&path).unwrap();
            std::fs::write(path.join("mic.pcm"), b"").unwrap();
            self
        }

        fn repairable(&self, folder: &str) -> &Self {
            let path = self.dir.path().join(folder).join("journal");
            std::fs::create_dir_all(&path).unwrap();
            std::fs::write(path.join("manifest.toml"), b"").unwrap();
            self
        }
    }

    fn id(text: &str) -> SessionRef {
        SessionRef::parse(text).unwrap()
    }

    #[test]
    fn test_listing_orders_most_recently_started_first() {
        let tree = Tree::new();
        tree.complete("2026-04-01-0900-alpha-01AAA", "Alpha")
            .complete("2026-04-29-1430-beta-01BBB", "Beta");

        let page = tree
            .repository()
            .list_sessions(PageRequest::first())
            .unwrap();

        assert_eq!(
            page.items
                .iter()
                .map(|row| row.id.as_str())
                .collect::<Vec<_>>(),
            vec!["2026-04-29-1430-beta-01BBB", "2026-04-01-0900-alpha-01AAA"]
        );
    }

    #[test]
    fn test_listing_pages_without_losing_or_repeating_a_session() {
        let tree = Tree::new();
        for index in 0..7 {
            tree.complete(&format!("2026-04-0{index}-0900-session-01AA{index}"), "S");
        }
        let repository = tree.repository();

        let first = repository.list_sessions(PageRequest::new(0, 3)).unwrap();
        let second = repository.list_sessions(PageRequest::new(3, 3)).unwrap();
        let third = repository.list_sessions(PageRequest::new(6, 3)).unwrap();

        assert_eq!(first.total, 7);
        assert!(first.has_more);
        assert!(!third.has_more);
        let mut seen: Vec<&str> = first
            .items
            .iter()
            .chain(&second.items)
            .chain(&third.items)
            .map(|row| row.id.as_str())
            .collect();
        seen.sort_unstable();
        seen.dedup();
        assert_eq!(seen.len(), 7);
    }

    #[test]
    fn test_a_missing_storage_root_is_reported_as_missing_not_empty() {
        let repository = SessionRepository::new(StorageRoot::new("/nonexistent/scrybe-root"));

        let error = repository.list_sessions(PageRequest::first()).unwrap_err();

        assert_eq!(error.code(), ErrorCode::StorageRootMissing);
    }

    #[test]
    fn test_an_identity_outside_the_root_never_resolves() {
        let tree = Tree::new();
        tree.complete("2026-04-29-1430-acme-01HXYZ", "Acme");
        let repository = tree.repository();

        // Every shape that could address something outside the root is
        // refused before it reaches the repository at all.
        for escape in [
            "/etc/passwd",
            "../../etc",
            "nested/session",
            "nested\\session",
            "~/scrybe",
            "C:sessions",
        ] {
            assert!(
                SessionRef::parse(escape).is_err(),
                "{escape} must not construct an identity"
            );
        }

        // A syntactically valid identity that names nothing under the
        // root is a miss, not a traversal.
        let error = repository.get_session(&id("elsewhere")).unwrap_err();

        assert_eq!(error.code(), ErrorCode::SessionNotFound);
    }

    #[test]
    fn test_an_ambiguous_fragment_is_refused_rather_than_guessed() {
        let tree = Tree::new();
        tree.complete("2026-04-01-0900-standup-01AAA", "One")
            .complete("2026-04-02-0900-standup-01BBB", "Two");

        let error = tree.repository().get_session(&id("standup")).unwrap_err();

        assert_eq!(error.code(), ErrorCode::AmbiguousSessionId);
    }

    #[test]
    fn test_an_unambiguous_fragment_resolves_to_the_canonical_identity() {
        let tree = Tree::new();
        tree.complete("2026-04-29-1430-acme-01HXYZ", "Acme");

        let detail = tree.repository().get_session(&id("01HXYZ")).unwrap();

        assert_eq!(detail.id.as_str(), "2026-04-29-1430-acme-01HXYZ");
    }

    #[test]
    fn test_search_matches_transcript_text_of_complete_sessions_only() {
        let tree = Tree::new();
        tree.complete("2026-04-29-1430-acme-01HXYZ", "Acme");
        let draft = tree.dir.path().join("2026-04-28-0900-draft-01DDD");
        std::fs::create_dir_all(&draft).unwrap();
        std::fs::write(draft.join("transcript.md"), "hello world\n").unwrap();

        let page = tree
            .repository()
            .search_sessions(&SearchRequest::new("hello"), &CancellationToken::new())
            .unwrap();

        assert_eq!(
            page.items
                .iter()
                .map(|row| row.id.as_str())
                .collect::<Vec<_>>(),
            vec!["2026-04-29-1430-acme-01HXYZ"]
        );
    }

    #[test]
    fn test_search_matches_a_folder_name_even_for_an_incomplete_session() {
        let tree = Tree::new();
        tree.journal_only("2026-04-29-1430-standup-01HXYZ");

        let page = tree
            .repository()
            .search_sessions(&SearchRequest::new("standup"), &CancellationToken::new())
            .unwrap();

        assert_eq!(page.items.len(), 1);
        assert_eq!(page.items[0].state, SessionState::Unfinished);
    }

    #[test]
    fn test_a_cancelled_search_reports_cancellation_instead_of_partial_results() {
        let tree = Tree::new();
        tree.complete("2026-04-29-1430-acme-01HXYZ", "Acme");
        let cancel = CancellationToken::new();
        cancel.cancel();

        let error = tree
            .repository()
            .search_sessions(&SearchRequest::new("acme"), &cancel)
            .unwrap_err();

        assert_eq!(error.code(), ErrorCode::Cancelled);
    }

    #[test]
    fn test_search_is_paged() {
        let tree = Tree::new();
        for index in 0..5 {
            tree.complete(&format!("2026-04-0{index}-0900-standup-01AA{index}"), "S");
        }

        let page = tree
            .repository()
            .search_sessions(
                &SearchRequest::new("standup").at(PageRequest::new(0, 2)),
                &CancellationToken::new(),
            )
            .unwrap();

        assert_eq!(page.items.len(), 2);
        assert_eq!(page.total, 5);
        assert!(page.has_more);
    }

    #[test]
    fn test_a_session_created_by_another_tool_appears_without_an_explicit_refresh() {
        let tree = Tree::new();
        tree.complete("2026-04-01-0900-alpha-01AAA", "Alpha");
        let repository = tree.repository();
        assert_eq!(
            repository
                .list_sessions(PageRequest::first())
                .unwrap()
                .total,
            1
        );

        tree.complete("2026-04-29-1430-beta-01BBB", "Beta");

        assert_eq!(
            repository
                .list_sessions(PageRequest::first())
                .unwrap()
                .total,
            2
        );
    }

    #[test]
    fn test_an_artifact_written_by_another_tool_invalidates_the_cached_classification() {
        let tree = Tree::new();
        tree.repairable("2026-04-29-1430-acme-01HXYZ");
        let repository = tree.repository();
        assert_eq!(
            repository.get_session(&id("01HXYZ")).unwrap().state,
            SessionState::Repairable
        );

        std::fs::write(
            tree.dir
                .path()
                .join("2026-04-29-1430-acme-01HXYZ")
                .join("meta.toml"),
            "session_id = \"01AAA\"\nstarted_at = \"2026-04-29T14:30:00Z\"\n",
        )
        .unwrap();

        assert_eq!(
            repository.get_session(&id("01HXYZ")).unwrap().state,
            SessionState::Complete
        );
    }

    #[test]
    fn test_transcript_paging_walks_the_whole_document_exactly_once() {
        let tree = Tree::new();
        tree.complete("2026-04-29-1430-acme-01HXYZ", "Acme");
        let repository = tree.repository();

        let first = repository
            .read_transcript_page(&id("01HXYZ"), PageRequest::new(0, 2))
            .unwrap();
        let next = first.next.unwrap();
        let second = repository
            .read_transcript_page(&id("01HXYZ"), PageRequest::new(next.line, 2))
            .unwrap();

        assert_eq!(first.total_lines, 3);
        assert_eq!(first.lines, vec!["# t", "hello world"]);
        assert_eq!(second.lines, vec!["second line"]);
        assert!(second.next.is_none());
    }

    #[test]
    fn test_reading_notes_of_a_session_without_notes_reports_absence_not_failure() {
        let tree = Tree::new();
        tree.journal_only("2026-04-29-1430-acme-01HXYZ");

        let notes = tree.repository().read_notes(&id("01HXYZ")).unwrap();

        assert!(notes.markdown.is_none());
        assert_eq!(notes.state, SessionState::Unfinished);
    }

    #[test]
    fn test_repair_is_refused_for_a_session_with_nothing_to_recover() {
        let tree = Tree::new();
        tree.journal_only("2026-04-29-1430-acme-01HXYZ");

        let error = tree.repository().repair_session(&id("01HXYZ")).unwrap_err();

        assert_eq!(error.code(), ErrorCode::NotApplicable);
    }

    #[test]
    fn test_repair_of_an_already_complete_session_changes_nothing() {
        let tree = Tree::new();
        tree.complete("2026-04-29-1430-acme-01HXYZ", "Acme");

        let result = tree.repository().repair_session(&id("01HXYZ")).unwrap();

        assert_eq!(result.outcome, RepairOutcomeKind::NothingToRepair);
        assert_eq!(result.state, SessionState::Complete);
        assert!(!result.wrote_metadata);
    }

    struct FixedNotes(&'static str);

    impl NotesGenerator for FixedNotes {
        fn generate(
            &self,
            _request: &NotesGenerationRequest<'_>,
        ) -> std::result::Result<String, Box<dyn StdError + Send + Sync>> {
            Ok(self.0.to_string())
        }
    }

    struct FailingNotes;

    impl NotesGenerator for FailingNotes {
        fn generate(
            &self,
            _request: &NotesGenerationRequest<'_>,
        ) -> std::result::Result<String, Box<dyn StdError + Send + Sync>> {
            Err("provider refused the request".into())
        }
    }

    #[test]
    fn test_regenerating_notes_replaces_the_durable_document() {
        let tree = Tree::new();
        tree.complete("2026-04-29-1430-acme-01HXYZ", "Acme");
        let repository = tree.repository();

        let result = repository
            .regenerate_notes(&id("01HXYZ"), &FixedNotes("## TL;DR\n- regenerated\n"))
            .unwrap();

        assert_eq!(result.outcome, NotesGenerationOutcome::Replaced);
        assert_eq!(
            repository
                .read_notes(&id("01HXYZ"))
                .unwrap()
                .markdown
                .as_deref(),
            Some("## TL;DR\n- regenerated\n")
        );
    }

    #[test]
    fn test_regenerating_identical_notes_leaves_the_document_untouched() {
        let tree = Tree::new();
        tree.complete("2026-04-29-1430-acme-01HXYZ", "Acme");

        let result = tree
            .repository()
            .regenerate_notes(&id("01HXYZ"), &FixedNotes("## TL;DR\n- covered widgets\n"))
            .unwrap();

        assert_eq!(result.outcome, NotesGenerationOutcome::Unchanged);
    }

    #[test]
    fn test_a_generator_failure_leaves_the_previous_notes_in_place() {
        let tree = Tree::new();
        tree.complete("2026-04-29-1430-acme-01HXYZ", "Acme");
        let repository = tree.repository();

        let error = repository
            .regenerate_notes(&id("01HXYZ"), &FailingNotes)
            .unwrap_err();

        assert_eq!(error.code(), ErrorCode::NotesGenerationFailed);
        assert_eq!(
            repository
                .read_notes(&id("01HXYZ"))
                .unwrap()
                .markdown
                .as_deref(),
            Some("## TL;DR\n- covered widgets\n")
        );
    }

    #[test]
    fn test_regeneration_is_refused_for_a_session_without_a_durable_transcript() {
        let tree = Tree::new();
        tree.repairable("2026-04-29-1430-acme-01HXYZ");

        let error = tree
            .repository()
            .regenerate_notes(&id("01HXYZ"), &FixedNotes("x"))
            .unwrap_err();

        assert_eq!(error.code(), ErrorCode::NotApplicable);
    }
}
