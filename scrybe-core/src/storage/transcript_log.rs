// Copyright 2026 Mathews Tom
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     https://www.apache.org/licenses/LICENSE-2.0

//! `transcript.partial.jsonl` — write-ahead log for per-chunk crash
//! recovery (`docs/system-design.md` §8.3 "Storage layout invariants").
//!
//! Each line is a JSON object describing one [`AttributedChunk`] with
//! the rendered-markdown bookkeeping needed for the post-crash replay
//! step:
//!
//! - `seq` monotonically increases and is the truncation cursor `scrybe
//!   doctor` uses to identify orphaned chunks.
//! - `flushed_to_transcript` flips to `true` only after the durable
//!   append to `transcript.md` succeeds. A crash between the WAL write
//!   and the markdown render leaves `flushed_to_transcript = false`,
//!   which the recovery scanner replays.
//! - `chunk` is the structured payload — speaker, text, timestamps —
//!   so a recovered session can re-render markdown without rerunning
//!   STT.
//!
//! The WAL deliberately uses `serde_json::to_string` (not `serde_json::
//! to_writer`) so the on-disk shape stays stable across compiler /
//! serde version bumps that change inline-writer behavior.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::StorageError;
use crate::storage::atomic::append_durable;
use crate::types::AttributedChunk;

/// File name used for the WAL inside each session folder.
pub const TRANSCRIPT_PARTIAL_LOG_NAME: &str = "transcript.partial.jsonl";

/// What a WAL line describes.
///
/// Serde-defaulted to [`Self::Final`]: every line written before this
/// field existed is a completed chunk, which is exactly the recovery
/// semantics those files already had.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TranscriptRecordKind {
    /// A completed chunk that belongs in `transcript.md`.
    #[default]
    Final,
    /// A growing streaming hypothesis, written while recording so a
    /// crash leaves evidence of in-flight speech. Never rendered into
    /// `transcript.md`.
    Partial,
}

/// One line in the WAL. The shape is stable inside the v0.x train —
/// `scrybe doctor` reads any prior session's file for recovery.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct TranscriptPartialRecord {
    /// 1-indexed monotonic counter assigned by [`TranscriptPartialLog`].
    /// `scrybe doctor` reads the highest `seq` whose
    /// `flushed_to_transcript` is `true` and replays everything after.
    pub seq: u64,
    /// Set to `true` once the chunk has been durably appended to
    /// `transcript.md`. The append-then-mark protocol means the WAL
    /// always records the flush state observed at the time of the WAL
    /// append; a process crash between the markdown append and the WAL
    /// mark is the exact case the recovery scanner repairs.
    ///
    /// Partial records are written with `true` on purpose: an older
    /// binary reading a newer session file does not know the `kind`
    /// field, and this is what stops it from replaying a partial
    /// hypothesis into `transcript.md`.
    pub flushed_to_transcript: bool,
    /// Record class. Absent in files written before streaming partials
    /// existed, where every line was final.
    #[serde(default)]
    pub kind: TranscriptRecordKind,
    pub chunk: AttributedChunk,
}

/// Append-only writer for `transcript.partial.jsonl`.
///
/// The writer keeps the next sequence number in memory; callers must
/// not interleave append calls with concurrent writers on the same path
/// — the per-session pid lock (see [`acquire_session_lock`]) is what
/// enforces single-writer access.
///
/// [`acquire_session_lock`]: crate::storage::acquire_session_lock
pub struct TranscriptPartialLog {
    path: PathBuf,
    next_seq: u64,
}

impl TranscriptPartialLog {
    /// Open or create the WAL inside `session_folder`. Existing records
    /// are scanned to recover the next sequence number.
    ///
    /// # Errors
    ///
    /// `StorageError::Io` for filesystem failures.
    pub fn open(session_folder: &Path) -> Result<Self, StorageError> {
        let path = session_folder.join(TRANSCRIPT_PARTIAL_LOG_NAME);
        let next_seq = match std::fs::read_to_string(&path) {
            Ok(text) => last_seq(&text).saturating_add(1).max(1),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => 1,
            Err(e) => return Err(StorageError::Io(e)),
        };
        Ok(Self { path, next_seq })
    }

    /// Path to the underlying file. Useful for `scrybe doctor` so it
    /// can attach the path to a recovery report.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Append one record. Sets `flushed_to_transcript = false`; callers
    /// upgrade the record after the durable append to `transcript.md`
    /// via [`Self::mark_flushed`] in the same chunk's lifecycle.
    ///
    /// # Errors
    ///
    /// `StorageError::Io` for filesystem failures.
    pub fn append_pending(&mut self, chunk: AttributedChunk) -> Result<u64, StorageError> {
        let seq = self.next_seq;
        let record = TranscriptPartialRecord {
            seq,
            flushed_to_transcript: false,
            kind: TranscriptRecordKind::Final,
            chunk,
        };
        self.write_line(&record)?;
        self.next_seq = seq.saturating_add(1);
        Ok(seq)
    }

    /// Re-append the same record with `flushed_to_transcript = true`.
    /// Recovery treats the highest-sequence record per chunk as
    /// authoritative, so re-appending does not corrupt history.
    ///
    /// # Errors
    ///
    /// `StorageError::Io` for filesystem failures.
    pub fn mark_flushed(&mut self, seq: u64, chunk: AttributedChunk) -> Result<(), StorageError> {
        let record = TranscriptPartialRecord {
            seq,
            flushed_to_transcript: true,
            kind: TranscriptRecordKind::Final,
            chunk,
        };
        self.write_line(&record)
    }

    /// Append one streaming hypothesis.
    ///
    /// Tagged [`TranscriptRecordKind::Partial`] and marked flushed so no
    /// recovery reader — including one built before partials existed —
    /// can replay a hypothesis into `transcript.md`. The record exists
    /// so a crashed session still shows what was being said when it
    /// died.
    ///
    /// # Errors
    ///
    /// `StorageError::Io` for filesystem failures.
    pub fn append_partial(&mut self, chunk: AttributedChunk) -> Result<u64, StorageError> {
        let seq = self.next_seq;
        let record = TranscriptPartialRecord {
            seq,
            flushed_to_transcript: true,
            kind: TranscriptRecordKind::Partial,
            chunk,
        };
        self.write_line(&record)?;
        self.next_seq = seq.saturating_add(1);
        Ok(seq)
    }

    fn write_line(&self, record: &TranscriptPartialRecord) -> Result<(), StorageError> {
        let line = serde_json::to_string(record)
            .map_err(|e| StorageError::Io(std::io::Error::other(e)))?;
        let mut payload = line.into_bytes();
        payload.push(b'\n');
        append_durable(&self.path, &payload)
    }
}

/// Recovery view of a session WAL.
#[derive(Debug, Default, Eq, PartialEq)]
pub struct RecoveryReport {
    /// Records present in the WAL whose `flushed_to_transcript = true`
    /// — these are already in `transcript.md` and need no replay.
    pub flushed_seqs: Vec<u64>,
    /// Records that were appended to the WAL but whose
    /// `flushed_to_transcript = true` followup never landed. Recovery
    /// renders these into `transcript.md` and then re-marks them.
    pub orphans: Vec<TranscriptPartialRecord>,
    /// Lines that failed to deserialize. The presence of any malformed
    /// line is logged but does not abort recovery.
    pub malformed_line_count: u64,
    /// Streaming hypotheses seen in the WAL. Reported for triage only:
    /// recovery never renders them, and they are neither flushed nor
    /// orphaned work.
    pub partial_record_count: u64,
}

/// Read the WAL at `session_folder/transcript.partial.jsonl` and return
/// a [`RecoveryReport`] describing flushed and orphaned records.
///
/// Missing files yield an empty report — that is the steady state for
/// a session that completed cleanly (the WAL is left in place as a
/// trace; `scrybe doctor` reports zero orphans).
///
/// # Errors
///
/// `StorageError::Io` for filesystem failures other than `NotFound`.
pub fn scan_recovery(session_folder: &Path) -> Result<RecoveryReport, StorageError> {
    let path = session_folder.join(TRANSCRIPT_PARTIAL_LOG_NAME);
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(RecoveryReport::default()),
        Err(e) => return Err(StorageError::Io(e)),
    };

    let mut latest_per_seq: BTreeMap<u64, TranscriptPartialRecord> = BTreeMap::new();
    let mut malformed = 0_u64;

    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        match serde_json::from_str::<TranscriptPartialRecord>(trimmed) {
            Ok(record) => {
                let seq = record.seq;
                let entry = latest_per_seq.entry(seq).or_insert_with(|| record.clone());
                if record.flushed_to_transcript && !entry.flushed_to_transcript {
                    *entry = record;
                }
            }
            Err(_) => malformed = malformed.saturating_add(1),
        }
    }

    let mut report = RecoveryReport {
        malformed_line_count: malformed,
        ..RecoveryReport::default()
    };

    for record in latest_per_seq.into_values() {
        match record.kind {
            // A hypothesis is not pending work: the audio it came from
            // was either finalized (and its final record replayed like
            // any other) or lost with the crashed session.
            TranscriptRecordKind::Partial => {
                report.partial_record_count = report.partial_record_count.saturating_add(1);
            }
            TranscriptRecordKind::Final if record.flushed_to_transcript => {
                report.flushed_seqs.push(record.seq);
            }
            TranscriptRecordKind::Final => report.orphans.push(record),
        }
    }

    report.flushed_seqs.sort_unstable();
    report.orphans.sort_by_key(|r| r.seq);

    Ok(report)
}

fn last_seq(text: &str) -> u64 {
    let mut max = 0_u64;
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if let Ok(record) = serde_json::from_str::<TranscriptPartialRecord>(trimmed) {
            if record.seq > max {
                max = record.seq;
            }
        }
    }
    max
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use crate::types::{FrameSource, SpeakerLabel, TokenTiming, TranscriptChunk};
    use pretty_assertions::assert_eq;

    fn chunk(text: &str, speaker: SpeakerLabel, start_ms: u64) -> AttributedChunk {
        AttributedChunk {
            chunk: TranscriptChunk {
                text: text.into(),
                source: match speaker {
                    SpeakerLabel::Me => FrameSource::Mic,
                    SpeakerLabel::Them | SpeakerLabel::Named(_) | SpeakerLabel::Unknown => {
                        FrameSource::System
                    }
                },
                start_ms,
                duration_ms: 1_000,
                language: None,
                tokens: Vec::new(),
            },
            speaker,
        }
    }

    #[test]
    fn test_transcript_partial_log_open_creates_first_seq_at_one() {
        let dir = tempfile::tempdir().unwrap();
        let log = TranscriptPartialLog::open(dir.path()).unwrap();

        assert_eq!(log.next_seq, 1);
        assert_eq!(
            log.path(),
            dir.path().join(TRANSCRIPT_PARTIAL_LOG_NAME).as_path()
        );
    }

    #[test]
    fn test_transcript_partial_log_append_pending_returns_monotonic_seq() {
        let dir = tempfile::tempdir().unwrap();
        let mut log = TranscriptPartialLog::open(dir.path()).unwrap();

        let a = log
            .append_pending(chunk("hi", SpeakerLabel::Me, 0))
            .unwrap();
        let b = log
            .append_pending(chunk("ok", SpeakerLabel::Them, 1_000))
            .unwrap();
        let c = log
            .append_pending(chunk("yes", SpeakerLabel::Me, 2_000))
            .unwrap();

        assert_eq!((a, b, c), (1, 2, 3));
    }

    #[test]
    fn test_transcript_partial_log_reopen_resumes_seq_after_last_record() {
        let dir = tempfile::tempdir().unwrap();
        {
            let mut log = TranscriptPartialLog::open(dir.path()).unwrap();
            log.append_pending(chunk("a", SpeakerLabel::Me, 0)).unwrap();
            log.append_pending(chunk("b", SpeakerLabel::Them, 1_000))
                .unwrap();
        }

        let reopened = TranscriptPartialLog::open(dir.path()).unwrap();

        assert_eq!(reopened.next_seq, 3);
    }

    #[test]
    fn test_scan_recovery_returns_empty_report_when_log_missing() {
        let dir = tempfile::tempdir().unwrap();

        let report = scan_recovery(dir.path()).unwrap();

        assert_eq!(report, RecoveryReport::default());
    }

    #[test]
    fn test_scan_recovery_marks_pending_records_as_orphans() {
        let dir = tempfile::tempdir().unwrap();
        let mut log = TranscriptPartialLog::open(dir.path()).unwrap();
        let seq = log
            .append_pending(chunk("crash before flush", SpeakerLabel::Me, 0))
            .unwrap();

        let report = scan_recovery(dir.path()).unwrap();

        assert_eq!(report.flushed_seqs, Vec::<u64>::new());
        assert_eq!(report.orphans.len(), 1);
        assert_eq!(report.orphans[0].seq, seq);
        assert_eq!(report.orphans[0].chunk.chunk.text, "crash before flush");
    }

    #[test]
    fn test_scan_recovery_treats_marked_flush_as_authoritative_over_pending() {
        let dir = tempfile::tempdir().unwrap();
        let mut log = TranscriptPartialLog::open(dir.path()).unwrap();
        let chunk_value = chunk("flushed", SpeakerLabel::Them, 500);
        let seq = log.append_pending(chunk_value.clone()).unwrap();
        log.mark_flushed(seq, chunk_value).unwrap();

        let report = scan_recovery(dir.path()).unwrap();

        assert_eq!(report.orphans, Vec::<TranscriptPartialRecord>::new());
        assert_eq!(report.flushed_seqs, vec![seq]);
    }

    #[test]
    fn test_scan_recovery_separates_flushed_and_orphan_records() {
        let dir = tempfile::tempdir().unwrap();
        let mut log = TranscriptPartialLog::open(dir.path()).unwrap();

        let s1 = log
            .append_pending(chunk("first", SpeakerLabel::Me, 0))
            .unwrap();
        log.mark_flushed(s1, chunk("first", SpeakerLabel::Me, 0))
            .unwrap();

        let _s2 = log
            .append_pending(chunk("orphan", SpeakerLabel::Them, 1_000))
            .unwrap();

        let s3 = log
            .append_pending(chunk("third", SpeakerLabel::Me, 2_000))
            .unwrap();
        log.mark_flushed(s3, chunk("third", SpeakerLabel::Me, 2_000))
            .unwrap();

        let report = scan_recovery(dir.path()).unwrap();

        assert_eq!(report.flushed_seqs, vec![1, 3]);
        assert_eq!(report.orphans.len(), 1);
        assert_eq!(report.orphans[0].seq, 2);
    }

    #[test]
    fn test_scan_recovery_counts_malformed_lines_without_aborting() {
        let dir = tempfile::tempdir().unwrap();
        let mut log = TranscriptPartialLog::open(dir.path()).unwrap();
        log.append_pending(chunk("good", SpeakerLabel::Me, 0))
            .unwrap();

        std::fs::write(
            dir.path().join(TRANSCRIPT_PARTIAL_LOG_NAME),
            b"{\"seq\":1,\"flushed_to_transcript\":false,\"chunk\":{\"chunk\":{\"text\":\"good\",\"source\":\"mic\",\"start_ms\":0,\"duration_ms\":1000,\"language\":null},\"speaker\":{\"kind\":\"me\"}}}\n\
             this is not valid json\n",
        )
        .unwrap();

        let report = scan_recovery(dir.path()).unwrap();

        assert_eq!(report.malformed_line_count, 1);
        assert_eq!(report.orphans.len(), 1);
    }

    #[test]
    fn test_append_partial_tags_record_and_marks_it_flushed() {
        let dir = tempfile::tempdir().unwrap();
        let mut log = TranscriptPartialLog::open(dir.path()).unwrap();

        let seq = log
            .append_partial(chunk("growing hypo", SpeakerLabel::Me, 0))
            .unwrap();

        let line = std::fs::read_to_string(dir.path().join(TRANSCRIPT_PARTIAL_LOG_NAME)).unwrap();
        let record: TranscriptPartialRecord = serde_json::from_str(line.trim()).unwrap();
        assert_eq!(record.seq, seq);
        assert_eq!(record.kind, TranscriptRecordKind::Partial);
        // Load-bearing for older readers: they ignore `kind` and would
        // otherwise replay the hypothesis into `transcript.md`.
        assert!(record.flushed_to_transcript);
    }

    #[test]
    fn test_scan_recovery_ignores_partial_records() {
        let dir = tempfile::tempdir().unwrap();
        let mut log = TranscriptPartialLog::open(dir.path()).unwrap();
        log.append_partial(chunk("hypo one", SpeakerLabel::Me, 0))
            .unwrap();
        log.append_partial(chunk("hypo one two", SpeakerLabel::Me, 0))
            .unwrap();
        let orphan = log
            .append_pending(chunk("completed", SpeakerLabel::Me, 0))
            .unwrap();

        let report = scan_recovery(dir.path()).unwrap();

        assert_eq!(report.partial_record_count, 2);
        assert_eq!(report.flushed_seqs, Vec::<u64>::new());
        assert_eq!(report.orphans.len(), 1);
        assert_eq!(report.orphans[0].seq, orphan);
    }

    #[test]
    fn test_scan_recovery_treats_untagged_legacy_records_as_final() {
        // A WAL written before `kind` and `tokens` existed: an orphan
        // must still be replayed, so the defaults cannot change.
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join(TRANSCRIPT_PARTIAL_LOG_NAME),
            b"{\"seq\":1,\"flushed_to_transcript\":true,\"chunk\":{\"chunk\":{\"text\":\"done\",\"source\":\"mic\",\"start_ms\":0,\"duration_ms\":1000,\"language\":null},\"speaker\":{\"kind\":\"me\"}}}\n\
             {\"seq\":2,\"flushed_to_transcript\":false,\"chunk\":{\"chunk\":{\"text\":\"lost\",\"source\":\"mic\",\"start_ms\":1000,\"duration_ms\":1000,\"language\":null},\"speaker\":{\"kind\":\"me\"}}}\n",
        )
        .unwrap();

        let report = scan_recovery(dir.path()).unwrap();

        assert_eq!(report.flushed_seqs, vec![1]);
        assert_eq!(report.orphans.len(), 1);
        assert_eq!(report.orphans[0].kind, TranscriptRecordKind::Final);
        assert_eq!(report.orphans[0].chunk.chunk.text, "lost");
        assert_eq!(report.orphans[0].chunk.chunk.tokens, Vec::new());
        assert_eq!(report.partial_record_count, 0);
    }

    #[test]
    fn test_wal_round_trips_token_timings() {
        let dir = tempfile::tempdir().unwrap();
        let mut log = TranscriptPartialLog::open(dir.path()).unwrap();
        let mut timed = chunk("hello there", SpeakerLabel::Them, 4_000);
        timed.chunk.tokens = vec![
            TokenTiming {
                token: "▁hello".to_string(),
                timestamp_ms: 4_080,
            },
            TokenTiming {
                token: "▁there".to_string(),
                timestamp_ms: 4_520,
            },
        ];

        let seq = log.append_pending(timed.clone()).unwrap();
        log.mark_flushed(seq, timed.clone()).unwrap();
        let report = scan_recovery(dir.path()).unwrap();

        let text = std::fs::read_to_string(dir.path().join(TRANSCRIPT_PARTIAL_LOG_NAME)).unwrap();
        let last: TranscriptPartialRecord =
            serde_json::from_str(text.lines().last().unwrap()).unwrap();
        assert_eq!(last.chunk.chunk.tokens, timed.chunk.tokens);
        assert_eq!(report.flushed_seqs, vec![seq]);
    }
}
