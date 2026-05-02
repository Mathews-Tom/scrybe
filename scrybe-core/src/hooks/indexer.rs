// Copyright 2026 Mathews Tom
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     https://www.apache.org/licenses/LICENSE-2.0

//! `TantivyIndexerHook` — incremental local full-text index over
//! recorded sessions.
//!
//! Behind the `hook-tantivy` cargo feature so the default workspace
//! build does not pull `tantivy`. The index lives at
//! `<storage.root>/.index/` and is **never the source of truth**:
//! `rebuild_all` walks `<storage.root>/<session>/` and regenerates the
//! index from `notes.md`, `transcript.md`, and `meta.toml`. A torched
//! index folder is recoverable; deleting a session folder makes the
//! corresponding index entries orphan-but-harmless until the next
//! `rebuild_all`.
//!
//! Failure mode: an indexing failure on a session emits `HookError::Hook`
//! to the dispatcher (which surfaces it as `LifecycleEvent::HookFailed`)
//! and **never** mutates the source-of-truth files under
//! `<storage.root>/<session>/`. This is what lets the index be a pure
//! cache.

use std::path::{Path, PathBuf};

use async_trait::async_trait;

use crate::error::HookError;
use crate::hooks::{Hook, LifecycleEvent};
use crate::types::SessionId;

/// File names this hook reads when indexing a session folder. Kept as
/// constants so `rebuild_all` and the live event handler agree on
/// what they read. Gated on the feature because the no-feature build
/// only carries the no-op stubs and never reads these.
#[cfg(feature = "hook-tantivy")]
const NOTES_FILE: &str = "notes.md";
#[cfg(feature = "hook-tantivy")]
const TRANSCRIPT_FILE: &str = "transcript.md";
#[cfg(feature = "hook-tantivy")]
const META_FILE: &str = "meta.toml";

/// Configuration for [`TantivyIndexerHook`].
#[derive(Clone, Debug)]
pub struct TantivyIndexerHookConfig {
    /// Storage root — the parent folder of every session directory
    /// (matches `Config::storage.root`).
    pub storage_root: PathBuf,
    /// Subdirectory under `storage_root` that holds the tantivy index.
    /// Defaults to `.index` per `docs/system-overview.md` §3.
    pub index_subdir: PathBuf,
    /// Stable hook identifier surfaced in `meta.toml`'s `[hooks]` table.
    pub display_name: String,
}

impl TantivyIndexerHookConfig {
    /// Construct with the documented `.index` subdirectory.
    #[must_use]
    pub fn new(storage_root: PathBuf) -> Self {
        Self {
            storage_root,
            index_subdir: PathBuf::from(".index"),
            display_name: "indexer".to_string(),
        }
    }

    /// Resolved absolute path to the index directory.
    #[must_use]
    pub fn index_path(&self) -> PathBuf {
        self.storage_root.join(&self.index_subdir)
    }
}

/// Hit returned by [`TantivyIndexerHook::search`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IndexHit {
    /// The session ULID (as 26-char string).
    pub session_id: String,
    /// Document kind: `"notes"` or `"transcript"`.
    pub kind: String,
    /// Title from `meta.toml`, or empty when not yet known.
    pub title: String,
    /// Snippet of the matching body.
    pub snippet: String,
}

/// Tantivy-backed lifecycle hook.
///
/// Subscribes to `NotesGenerated` (and, internally,
/// `SessionEnd` / `ChunkTranscribed` are ignored — re-indexing every
/// chunk would dominate the runtime cost of a session). Use
/// [`Self::rebuild_all`] to repopulate the index from on-disk session
/// folders after a manual edit, a corrupted index, or a fresh install
/// pointed at an existing `~/scrybe/`.
pub struct TantivyIndexerHook {
    config: TantivyIndexerHookConfig,
    #[cfg(feature = "hook-tantivy")]
    inner: backend::Inner,
}

impl TantivyIndexerHook {
    /// Construct a hook over `config.storage_root`. Opens or creates
    /// the on-disk index.
    ///
    /// # Errors
    ///
    /// `HookError::Hook` if the index directory cannot be created or
    /// the schema mismatches an existing index.
    #[cfg(feature = "hook-tantivy")]
    pub fn new(config: TantivyIndexerHookConfig) -> Result<Self, HookError> {
        let inner = backend::Inner::open(&config.index_path())?;
        Ok(Self { config, inner })
    }

    /// Construct without the tantivy feature; every call to
    /// [`Hook::on_event`] returns `HookError::Hook` to make the
    /// missing feature obvious instead of silently no-oping.
    ///
    /// # Errors
    ///
    /// Never fails today; signature mirrors the feature-on variant
    /// so call sites do not branch on feature flags.
    #[cfg(not(feature = "hook-tantivy"))]
    #[allow(clippy::missing_errors_doc, clippy::unnecessary_wraps)]
    pub const fn new(config: TantivyIndexerHookConfig) -> Result<Self, HookError> {
        Ok(Self { config })
    }

    /// Rebuild the index from on-disk session folders. Walks
    /// `<storage_root>/*/` and indexes every folder that carries a
    /// `meta.toml`. Idempotent: identical input produces identical
    /// index state.
    ///
    /// # Errors
    ///
    /// `HookError::Hook` for any unrecoverable index error. Per-
    /// session read failures are skipped and reported via the
    /// `tracing` event log, not raised — a corrupted single session
    /// must not block the whole rebuild.
    #[cfg(feature = "hook-tantivy")]
    pub fn rebuild_all(&self) -> Result<u64, HookError> {
        self.inner.rebuild_all(&self.config.storage_root)
    }

    /// Stub no-op when built without `hook-tantivy`.
    ///
    /// # Errors
    ///
    /// Always returns `HookError::Hook` describing the disabled feature.
    #[cfg(not(feature = "hook-tantivy"))]
    pub fn rebuild_all(&self) -> Result<u64, HookError> {
        Err(feature_disabled_error())
    }

    /// Search the index. Returns up to `limit` hits ordered by tantivy's
    /// default BM25 scoring.
    ///
    /// # Errors
    ///
    /// `HookError::Hook` on query parse error or index read failure.
    #[cfg(feature = "hook-tantivy")]
    pub fn search(&self, query: &str, limit: usize) -> Result<Vec<IndexHit>, HookError> {
        self.inner.search(query, limit)
    }

    /// Stub no-op when built without `hook-tantivy`.
    ///
    /// # Errors
    ///
    /// Always returns `HookError::Hook` describing the disabled feature.
    #[cfg(not(feature = "hook-tantivy"))]
    #[allow(clippy::unused_self)]
    pub fn search(&self, _query: &str, _limit: usize) -> Result<Vec<IndexHit>, HookError> {
        Err(feature_disabled_error())
    }
}

#[async_trait]
impl Hook for TantivyIndexerHook {
    async fn on_event(&self, event: &LifecycleEvent) -> Result<(), HookError> {
        let LifecycleEvent::NotesGenerated { id, notes_path } = event else {
            return Ok(());
        };
        let session_dir = match notes_path.parent() {
            Some(dir) => dir.to_owned(),
            None => return Ok(()),
        };
        index_one_session(self, *id, &session_dir).await
    }

    fn name(&self) -> &str {
        &self.config.display_name
    }
}

#[cfg(feature = "hook-tantivy")]
async fn index_one_session(
    hook: &TantivyIndexerHook,
    id: SessionId,
    session_dir: &Path,
) -> Result<(), HookError> {
    let storage_root = hook.config.storage_root.clone();
    let index_path = hook.config.index_path();
    let session_dir = session_dir.to_owned();
    tokio::task::spawn_blocking(move || {
        let inner = backend::Inner::open(&index_path)?;
        inner.index_session(&storage_root, &session_dir, id)
    })
    .await
    .map_err(|e| HookError::Hook(Box::new(e)))?
}

#[cfg(not(feature = "hook-tantivy"))]
#[allow(clippy::unused_async)]
async fn index_one_session(
    _hook: &TantivyIndexerHook,
    _id: SessionId,
    _session_dir: &Path,
) -> Result<(), HookError> {
    Err(feature_disabled_error())
}

#[cfg(not(feature = "hook-tantivy"))]
fn feature_disabled_error() -> HookError {
    HookError::Hook(Box::new(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "scrybe-core was built without the `hook-tantivy` cargo feature; \
         enable it to use TantivyIndexerHook",
    )))
}

/// Read a session-folder file (`notes.md`, `transcript.md`) when
/// present. `NotFound` returns `None` silently (the file may simply
/// not have been generated yet); any other IO error logs a warning
/// at `tracing::warn!` so a permission-denied or disk-corruption
/// failure surfaces in operator logs instead of looking identical to
/// "no notes yet."
#[cfg(feature = "hook-tantivy")]
fn read_session_file(session_dir: &Path, name: &str) -> Option<String> {
    let path = session_dir.join(name);
    match std::fs::read_to_string(&path) {
        Ok(body) => Some(body),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => {
            tracing::warn!(
                path = %path.display(),
                error = %e,
                "skipping unreadable session file during index"
            );
            None
        }
    }
}

/// Read `meta.toml`'s `title` field. Tolerant of missing or unparseable
/// files: a session whose `meta.toml` is in flight should still be
/// searchable by transcript text. Gated on the feature in production
/// builds; the test module re-uses it under `#[cfg(test)]` for the
/// parsing tests.
#[cfg(any(test, feature = "hook-tantivy"))]
fn read_title(meta_toml: &Path) -> String {
    let Ok(text) = std::fs::read_to_string(meta_toml) else {
        return String::new();
    };
    for line in text.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("title") {
            let rest = rest.trim_start();
            if let Some(rest) = rest.strip_prefix('=') {
                let rest = rest.trim();
                let stripped = rest
                    .strip_prefix('"')
                    .and_then(|s| s.strip_suffix('"'))
                    .unwrap_or(rest);
                return stripped.to_string();
            }
        }
    }
    String::new()
}

#[cfg(feature = "hook-tantivy")]
mod backend {
    use std::path::Path;

    use tantivy::collector::TopDocs;
    use tantivy::query::QueryParser;
    use tantivy::schema::{Field, Schema, STORED, STRING, TEXT};
    use tantivy::{doc, Index, IndexReader, IndexWriter, ReloadPolicy, TantivyDocument, Term};

    use super::{read_session_file, read_title, IndexHit, META_FILE, NOTES_FILE, TRANSCRIPT_FILE};
    use crate::error::HookError;
    use crate::types::SessionId;

    /// Schema fields owned together so the writer/searcher pair always
    /// refer to the same `Field` handles.
    pub(super) struct Fields {
        pub session_id: Field,
        pub kind: Field,
        pub title: Field,
        pub body: Field,
    }

    pub(super) struct Inner {
        pub(super) index: Index,
        pub(super) fields: Fields,
        pub(super) reader: IndexReader,
    }

    impl Inner {
        /// Heap budget for the writer. 50 MB is the tantivy minimum and
        /// is generous for a full-rebuild over a typical user's archive
        /// (a 90-min meeting transcript is ~50 KB).
        const WRITER_HEAP: usize = 50_000_000;

        pub(super) fn open(index_path: &Path) -> Result<Self, HookError> {
            let mut schema_builder = Schema::builder();
            let session_id = schema_builder.add_text_field("session_id", STRING | STORED);
            let kind = schema_builder.add_text_field("kind", STRING | STORED);
            let title = schema_builder.add_text_field("title", TEXT | STORED);
            let body = schema_builder.add_text_field("body", TEXT | STORED);
            let schema = schema_builder.build();

            std::fs::create_dir_all(index_path).map_err(|e| HookError::Hook(Box::new(e)))?;

            let index = Index::open_or_create(
                tantivy::directory::MmapDirectory::open(index_path)
                    .map_err(|e| HookError::Hook(Box::new(e)))?,
                schema,
            )
            .map_err(|e| HookError::Hook(Box::new(e)))?;

            let reader = index
                .reader_builder()
                .reload_policy(ReloadPolicy::OnCommitWithDelay)
                .try_into()
                .map_err(|e| HookError::Hook(Box::new(e)))?;

            let fields = Fields {
                session_id,
                kind,
                title,
                body,
            };
            Ok(Self {
                index,
                fields,
                reader,
            })
        }

        pub(super) fn index_session(
            &self,
            _storage_root: &Path,
            session_dir: &Path,
            id: SessionId,
        ) -> Result<(), HookError> {
            let session_id_str = id.to_string_26();
            let mut writer: IndexWriter = self
                .index
                .writer(Self::WRITER_HEAP)
                .map_err(|e| HookError::Hook(Box::new(e)))?;

            // Delete-then-add gives idempotency. Re-running the hook on
            // the same session replaces existing docs in place.
            writer.delete_term(Term::from_field_text(
                self.fields.session_id,
                &session_id_str,
            ));

            let title = read_title(&session_dir.join(META_FILE));

            if let Some(notes_body) = read_session_file(session_dir, NOTES_FILE) {
                writer
                    .add_document(doc!(
                        self.fields.session_id => session_id_str.clone(),
                        self.fields.kind => "notes",
                        self.fields.title => title.clone(),
                        self.fields.body => notes_body,
                    ))
                    .map_err(|e| HookError::Hook(Box::new(e)))?;
            }

            if let Some(transcript_body) = read_session_file(session_dir, TRANSCRIPT_FILE) {
                writer
                    .add_document(doc!(
                        self.fields.session_id => session_id_str,
                        self.fields.kind => "transcript",
                        self.fields.title => title,
                        self.fields.body => transcript_body,
                    ))
                    .map_err(|e| HookError::Hook(Box::new(e)))?;
            }

            writer.commit().map_err(|e| HookError::Hook(Box::new(e)))?;
            Ok(())
        }

        pub(super) fn rebuild_all(&self, storage_root: &Path) -> Result<u64, HookError> {
            let entries = match std::fs::read_dir(storage_root) {
                Ok(it) => it,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(0),
                Err(e) => return Err(HookError::Hook(Box::new(e))),
            };

            let mut writer: IndexWriter = self
                .index
                .writer(Self::WRITER_HEAP)
                .map_err(|e| HookError::Hook(Box::new(e)))?;
            // Truncate the index before re-adding so removed sessions
            // disappear in the same pass.
            writer
                .delete_all_documents()
                .map_err(|e| HookError::Hook(Box::new(e)))?;

            let mut indexed: u64 = 0;
            for entry_result in entries {
                let entry = match entry_result {
                    Ok(e) => e,
                    Err(e) => {
                        tracing::warn!(error = %e, "skipping unreadable directory entry during rebuild");
                        continue;
                    }
                };
                let path = entry.path();
                if !path.is_dir() {
                    continue;
                }
                if path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.starts_with('.'))
                {
                    // Skip dotfile children like `.index/` itself.
                    continue;
                }
                let Some(folder_name) = path.file_name().and_then(|n| n.to_str()) else {
                    continue;
                };
                let Some(id) = parse_id_from_folder(folder_name) else {
                    tracing::warn!(folder = %folder_name, "skipping folder with no parsable session id");
                    continue;
                };
                if let Err(e) = self.add_session_to_writer(&writer, &path, id) {
                    tracing::warn!(folder = %folder_name, error = %e, "skipping session that failed to index");
                    continue;
                }
                indexed += 1;
            }
            writer.commit().map_err(|e| HookError::Hook(Box::new(e)))?;
            Ok(indexed)
        }

        fn add_session_to_writer(
            &self,
            writer: &IndexWriter,
            session_dir: &Path,
            id: SessionId,
        ) -> Result<(), HookError> {
            let session_id_str = id.to_string_26();
            let title = read_title(&session_dir.join(META_FILE));
            if let Some(body) = read_session_file(session_dir, NOTES_FILE) {
                writer
                    .add_document(doc!(
                        self.fields.session_id => session_id_str.clone(),
                        self.fields.kind => "notes",
                        self.fields.title => title.clone(),
                        self.fields.body => body,
                    ))
                    .map_err(|e| HookError::Hook(Box::new(e)))?;
            }
            if let Some(body) = read_session_file(session_dir, TRANSCRIPT_FILE) {
                writer
                    .add_document(doc!(
                        self.fields.session_id => session_id_str,
                        self.fields.kind => "transcript",
                        self.fields.title => title,
                        self.fields.body => body,
                    ))
                    .map_err(|e| HookError::Hook(Box::new(e)))?;
            }
            Ok(())
        }

        pub(super) fn search(&self, query: &str, limit: usize) -> Result<Vec<IndexHit>, HookError> {
            self.reader
                .reload()
                .map_err(|e| HookError::Hook(Box::new(e)))?;
            let searcher = self.reader.searcher();
            let parser =
                QueryParser::for_index(&self.index, vec![self.fields.title, self.fields.body]);
            let parsed = parser
                .parse_query(query)
                .map_err(|e| HookError::Hook(Box::new(e)))?;
            let top = searcher
                .search(&parsed, &TopDocs::with_limit(limit))
                .map_err(|e| HookError::Hook(Box::new(e)))?;

            let mut hits = Vec::with_capacity(top.len());
            for (_score, address) in top {
                let retrieved: TantivyDocument = searcher
                    .doc(address)
                    .map_err(|e| HookError::Hook(Box::new(e)))?;
                hits.push(IndexHit {
                    session_id: stored_text(&retrieved, self.fields.session_id),
                    kind: stored_text(&retrieved, self.fields.kind),
                    title: stored_text(&retrieved, self.fields.title),
                    snippet: snippet_for(&stored_text(&retrieved, self.fields.body), query),
                });
            }
            Ok(hits)
        }
    }

    fn stored_text(doc: &TantivyDocument, field: Field) -> String {
        use tantivy::schema::Value;
        doc.get_first(field)
            .and_then(|v| v.as_str().map(str::to_owned))
            .unwrap_or_default()
    }

    /// Extract a 160-char window around the first occurrence of any
    /// query token. Falls back to the leading 160 chars when no token
    /// matches (e.g. the query hit the title field, not the body).
    fn snippet_for(body: &str, query: &str) -> String {
        const WINDOW: usize = 160;
        let lower_body = body.to_lowercase();
        for token in query.split_whitespace() {
            let needle = token
                .trim_matches(|c: char| !c.is_alphanumeric())
                .to_lowercase();
            if needle.is_empty() {
                continue;
            }
            if let Some(pos) = lower_body.find(&needle) {
                let start = body
                    .char_indices()
                    .map(|(i, _)| i)
                    .take_while(|&i| i <= pos.saturating_sub(WINDOW / 2))
                    .last()
                    .unwrap_or(0);
                let end = (start + WINDOW).min(body.len());
                let mut clipped_end = end;
                while !body.is_char_boundary(clipped_end) {
                    clipped_end -= 1;
                }
                return body[start..clipped_end].to_string();
            }
        }
        let mut end = WINDOW.min(body.len());
        while !body.is_char_boundary(end) {
            end -= 1;
        }
        body[..end].to_string()
    }

    /// Parse a `SessionId` out of a session folder name. Folder names
    /// follow `YYYY-MM-DD-HHMM-<title>-<26-char ULID>`; the ULID is
    /// always the last hyphen-separated segment.
    fn parse_id_from_folder(folder_name: &str) -> Option<SessionId> {
        let last = folder_name.rsplit('-').next()?;
        last.parse().ok()
    }
}

#[cfg(all(test, feature = "hook-tantivy"))]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod feature_tests {
    use std::path::PathBuf;
    use std::sync::Arc;

    use super::*;
    use crate::context::MeetingContext;

    fn write_session(
        root: &Path,
        folder: &str,
        title: &str,
        notes: &str,
        transcript: &str,
    ) -> PathBuf {
        let dir = root.join(folder);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join(META_FILE),
            format!("session_id = \"01HXY7K9RZNRC8WVCZ8K3J5T2A\"\ntitle = \"{title}\"\n"),
        )
        .unwrap();
        std::fs::write(dir.join(NOTES_FILE), notes).unwrap();
        std::fs::write(dir.join(TRANSCRIPT_FILE), transcript).unwrap();
        dir
    }

    fn fresh_hook(tmp: &Path) -> TantivyIndexerHook {
        let cfg = TantivyIndexerHookConfig::new(tmp.to_path_buf());
        TantivyIndexerHook::new(cfg).unwrap()
    }

    #[tokio::test]
    async fn test_indexer_hook_indexes_notes_when_notes_generated_event_fires() {
        let dir = tempfile::tempdir().unwrap();
        let session = write_session(
            dir.path(),
            "2026-04-29-1430-acme-discovery-01HXY7K9RZNRC8WVCZ8K3J5T2A",
            "Acme discovery call",
            "# Notes\n## TL;DR\n- pricing discussion\n",
            "**Me** [00:00:03]: pricing\n",
        );
        let hook = fresh_hook(dir.path());
        let event = LifecycleEvent::NotesGenerated {
            id: SessionId::new(),
            notes_path: session.join(NOTES_FILE),
        };

        hook.on_event(&event).await.unwrap();

        let hits = hook.search("pricing", 5).unwrap();
        assert!(!hits.is_empty(), "expected at least one hit on 'pricing'");
    }

    #[tokio::test]
    async fn test_indexer_hook_ignores_session_start_event() {
        let dir = tempfile::tempdir().unwrap();
        let hook = fresh_hook(dir.path());
        let event = LifecycleEvent::SessionStart {
            id: SessionId::new(),
            ctx: Arc::new(MeetingContext::default()),
        };

        hook.on_event(&event).await.unwrap();

        let hits = hook.search("anything", 5).unwrap();
        assert!(hits.is_empty());
    }

    #[test]
    fn test_indexer_rebuild_all_repopulates_from_session_folders() {
        let dir = tempfile::tempdir().unwrap();
        write_session(
            dir.path(),
            "2026-04-29-1430-acme-01HXY7K9RZNRC8WVCZ8K3J5T2A",
            "Acme",
            "## TL;DR\n- contract review\n",
            "**Me** [00:00:01]: contract\n",
        );
        write_session(
            dir.path(),
            "2026-04-30-1500-followup-01J0000000000000000000000A",
            "Followup",
            "## TL;DR\n- migration plan\n",
            "**Them** [00:00:02]: migration\n",
        );

        let hook = fresh_hook(dir.path());
        let count = hook.rebuild_all().unwrap();

        assert_eq!(count, 2);
        let hits = hook.search("contract", 5).unwrap();
        assert!(hits
            .iter()
            .any(|h| h.kind == "notes" || h.kind == "transcript"));
    }

    #[test]
    fn test_indexer_rebuild_all_is_idempotent_for_repeated_runs() {
        let dir = tempfile::tempdir().unwrap();
        write_session(
            dir.path(),
            "2026-04-29-1430-x-01HXY7K9RZNRC8WVCZ8K3J5T2A",
            "X",
            "## TL;DR\n- action\n",
            "**Me** [00:00:01]: action\n",
        );

        let hook = fresh_hook(dir.path());
        hook.rebuild_all().unwrap();
        hook.rebuild_all().unwrap();

        let hits = hook.search("action", 10).unwrap();
        // Two runs over the same fixture produces the same docs (delete_all + re-add),
        // not duplicate hits.
        assert!(
            hits.len() <= 2,
            "expected ≤ 2 docs (notes + transcript), got {}",
            hits.len()
        );
    }

    #[test]
    fn test_indexer_rebuild_all_skips_dotfolder_so_index_does_not_index_itself() {
        let dir = tempfile::tempdir().unwrap();
        write_session(
            dir.path(),
            "2026-04-29-1430-x-01HXY7K9RZNRC8WVCZ8K3J5T2A",
            "X",
            "## TL;DR\n- alpha\n",
            "**Me** [00:00:01]: alpha\n",
        );

        let hook = fresh_hook(dir.path());
        // First rebuild creates `.index/`; running again must not
        // descend into it.
        hook.rebuild_all().unwrap();
        let count = hook.rebuild_all().unwrap();

        assert_eq!(count, 1);
    }

    #[test]
    fn test_indexer_rebuild_all_returns_zero_on_missing_storage_root() {
        let dir = tempfile::tempdir().unwrap();
        let absent = dir.path().join("does-not-exist");
        let cfg = TantivyIndexerHookConfig::new(absent);
        let hook = TantivyIndexerHook::new(cfg).unwrap();

        let count = hook.rebuild_all().unwrap();

        assert_eq!(count, 0);
    }

    #[test]
    fn test_indexer_search_returns_hits_for_title_terms() {
        let dir = tempfile::tempdir().unwrap();
        write_session(
            dir.path(),
            "2026-04-29-1430-acme-discovery-01HXY7K9RZNRC8WVCZ8K3J5T2A",
            "Acme Discovery Call",
            "## TL;DR\n- nothing relevant\n",
            "**Me** [00:00:01]: misc\n",
        );

        let hook = fresh_hook(dir.path());
        hook.rebuild_all().unwrap();

        let hits = hook.search("acme", 5).unwrap();
        assert!(!hits.is_empty(), "title-field search should hit");
    }

    #[test]
    fn test_indexer_handles_session_with_only_transcript_and_no_notes() {
        let dir = tempfile::tempdir().unwrap();
        let folder = "2026-04-29-1430-partial-01HXY7K9RZNRC8WVCZ8K3J5T2A";
        let session = dir.path().join(folder);
        std::fs::create_dir_all(&session).unwrap();
        std::fs::write(
            session.join(META_FILE),
            "session_id = \"01HXY7K9RZNRC8WVCZ8K3J5T2A\"\ntitle = \"partial\"\n",
        )
        .unwrap();
        std::fs::write(session.join(TRANSCRIPT_FILE), "**Me**: midflight\n").unwrap();

        let hook = fresh_hook(dir.path());
        hook.rebuild_all().unwrap();

        let hits = hook.search("midflight", 5).unwrap();
        assert!(hits.iter().any(|h| h.kind == "transcript"));
    }

    #[test]
    fn test_indexer_event_with_orphan_notes_path_is_no_op() {
        let dir = tempfile::tempdir().unwrap();
        let hook = fresh_hook(dir.path());
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();

        let event = LifecycleEvent::NotesGenerated {
            id: SessionId::new(),
            // No parent folder context — hook should accept and skip.
            notes_path: PathBuf::from("notes.md"),
        };

        runtime
            .block_on(async { hook.on_event(&event).await })
            .unwrap();
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn test_config_index_path_joins_storage_root_with_index_subdir() {
        let cfg = TantivyIndexerHookConfig::new(PathBuf::from("/var/scrybe"));

        assert_eq!(cfg.index_path(), PathBuf::from("/var/scrybe/.index"));
    }

    #[test]
    fn test_config_default_display_name_is_indexer() {
        let cfg = TantivyIndexerHookConfig::new(PathBuf::from("/var/scrybe"));

        assert_eq!(cfg.display_name, "indexer");
    }

    #[test]
    fn test_read_title_returns_empty_when_meta_missing() {
        let dir = tempfile::tempdir().unwrap();

        let title = read_title(&dir.path().join("missing.toml"));

        assert_eq!(title, "");
    }

    #[test]
    fn test_read_title_extracts_quoted_value_from_well_formed_meta_toml() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("meta.toml");
        std::fs::write(
            &path,
            "session_id = \"01HXY\"\ntitle = \"Acme discovery\"\nlanguage = \"en\"\n",
        )
        .unwrap();

        let title = read_title(&path);

        assert_eq!(title, "Acme discovery");
    }

    #[test]
    fn test_read_title_returns_empty_when_no_title_field_present() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("meta.toml");
        std::fs::write(&path, "session_id = \"01HXY\"\n").unwrap();

        let title = read_title(&path);

        assert_eq!(title, "");
    }

    #[cfg(not(feature = "hook-tantivy"))]
    #[tokio::test]
    async fn test_indexer_hook_returns_unsupported_when_feature_disabled() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = TantivyIndexerHookConfig::new(dir.path().to_path_buf());
        let hook = TantivyIndexerHook::new(cfg).unwrap();
        let event = LifecycleEvent::NotesGenerated {
            id: SessionId::new(),
            notes_path: dir.path().join("session/notes.md"),
        };

        let err = hook.on_event(&event).await.unwrap_err();

        let HookError::Hook(boxed) = err else {
            panic!("expected HookError::Hook with feature disabled");
        };
        assert!(boxed.to_string().contains("hook-tantivy"));
    }
}
