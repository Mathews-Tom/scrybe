// Copyright 2026 Mathews Tom
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     https://www.apache.org/licenses/LICENSE-2.0

//! Bounded anchor-aligned accepted-final buffer (`.docs/DEVELOPMENT_PLAN.md`
//! M6 PR-1).
//!
//! `session::emit_final_chunk` used to append a source-labelled line to
//! `transcript.md` and dispatch `ChunkTranscribed` the moment a chunk
//! finalized, whichever path (batch or streaming) produced it. That
//! left no safe place for a later cross-source decision (M6 PR-2's
//! echo dedup) to act before persistence and hook dispatch already
//! happened.
//!
//! This module is the replacement boundary every final path now
//! funnels through. While at most one source's journal anchor is
//! known there is no counterpart to reorder against, so a record
//! releases immediately — a session with no system anchor stays
//! immediate for its whole lifetime. Once both anchors are known, a
//! record's anchor-aligned wall-clock time makes it comparable
//! against the sibling source, and the record is held for a bounded,
//! conservative window before release so a same-instant counterpart
//! has a chance to land and interleave correctly.
//!
//! Release is gated by **both** sources' progress, not either one
//! alone: the release watermark is `min(mic_progress_ms,
//! system_progress_ms) - window`. Using the minimum (not whichever
//! source last reported progress) matters because the two sources
//! advance independently — one can race ahead in real captured audio
//! while the other's VAD is still mid-segment. If a fast source alone
//! could clear the watermark, it would release a held record from the
//! *slow* source before the slow source has itself corroborated
//! anywhere near that instant, which both breaks the ordering
//! guarantee and (for M6 PR-2) would let an echo-dedup comparison run
//! before its counterpart could possibly exist yet.
//!
//! Every anchor-aligned instant this module receives is always built
//! by the caller (`session.rs`) from a source's `first_frame_epoch_ms`
//! anchor plus its *accumulated captured audio duration* — sample
//! counts, never `AudioFrame::timestamp_ns`. That field's origin is
//! source-defined and undefined across sources
//! (`types::audio::AudioFrame::timestamp_ns`); M2's own offline merge
//! established this exact rule (`pipeline::merge`: alignment uses the
//! epoch anchors and sample-derived durations, never raw device
//! timestamps), and this boundary follows the same rule. This module
//! never reads a raw frame timestamp — it only ever receives
//! already-resolved `Option<i64>` millisecond values.
//!
//! PR-1's own accept decision is unconditional — every pushed record
//! is eventually returned by [`AcceptedFinalBoundary::push`] or
//! [`AcceptedFinalBoundary::finish`], exactly once. PR-2 changes only
//! that decision at the same release point: a `Mic` final is rejected
//! only when some already-known `System` final's tokens are an exact,
//! position-for-position echo of it. "Exact" tolerates only lossless
//! ASR presentation noise — surrounding whitespace, `SentencePiece`'s
//! leading `▁` marker, and case — and every paired token's
//! anchor-aligned instant (never a raw frame timestamp) must still
//! agree within `UNRESOLVED_WINDOW`. Anything short of that — no
//! tokens, a length mismatch, a token that normalizes to nothing, or
//! one paired instant outside the window — leaves the record
//! untouched, so dedup only ever suppresses a genuine echo, never a
//! real utterance. `System` finals are never rejected, and once one
//! has been seen it stays available for comparison for the rest of
//! the session: an echo's segment can start well outside the
//! buffering window even though its individual tokens still land
//! inside it.

use std::time::Duration;

use crate::pipeline::chunker::ChunkBoundary;
use crate::types::{FrameSource, TranscriptChunk};

/// First-frame wall-clock anchors known so far, one per source, in
/// epoch milliseconds.
///
/// The minimum slice of `SessionJournals`' internal state the
/// boundary needs — never sample rate, channel count, or the writer
/// handle.
#[derive(Clone, Copy, Debug, Default)]
pub(super) struct SourceAnchors {
    pub(super) mic: Option<i64>,
    pub(super) system: Option<i64>,
}

impl SourceAnchors {
    pub(super) const fn anchor_for(self, source: FrameSource) -> Option<i64> {
        match source {
            FrameSource::System => self.system,
            FrameSource::Mic | FrameSource::Mixed => self.mic,
        }
    }

    pub(super) const fn both_known(self) -> bool {
        self.mic.is_some() && self.system.is_some()
    }
}

/// One final chunk on its way to `transcript.md` and the
/// `ChunkTranscribed` hook.
///
/// `wal_seq` is the streaming path's crash-recovery WAL sequence
/// number, written as pending the moment the segment finalized. The
/// caller marks it flushed only once this record is actually
/// released by the boundary, so the WAL's "flushed" bit always agrees
/// with what is really in `transcript.md`. The batch path carries no
/// WAL and always leaves this `None`.
#[derive(Debug)]
pub(super) struct AcceptedFinal {
    pub(super) transcript: TranscriptChunk,
    pub(super) source: FrameSource,
    pub(super) ended_on: ChunkBoundary,
    pub(super) wal_seq: Option<u64>,
}

/// A held record plus the aligned time the buffer orders by. `seq` is
/// the record's push order — the deterministic tie-break when two
/// records share the same aligned millisecond.
struct Pending {
    record: AcceptedFinal,
    aligned_ms: i64,
    seq: u64,
}

/// One token's normalized identity and anchor-aligned instant, kept
/// only long enough to compare a later record's tokens against it.
#[derive(Clone, Debug)]
struct DedupToken {
    normalized: String,
    aligned_ms: i64,
}

/// Normalize one token for cross-source comparison. Only losslessly
/// safe ASR presentation differences are erased: surrounding
/// whitespace, `SentencePiece`'s leading `▁` word-boundary marker, and
/// case. Returns `None` when nothing is left afterward, so a blank or
/// marker-only token can never manufacture a false match.
fn normalize_token(token: &str) -> Option<String> {
    let trimmed = token.trim();
    let trimmed = trimmed.strip_prefix('\u{2581}').unwrap_or(trimmed).trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_lowercase())
    }
}

/// The anchor-aligned wall-clock instant one token was spoken:
/// `segment_aligned_ms` — the record's already anchor-aligned segment
/// start, never a raw `AudioFrame` timestamp — plus the token's own
/// offset within that segment, `token_ms - transcript_start_ms` (both
/// session-relative, per [`TokenTiming`]'s and
/// [`TranscriptChunk::start_ms`]'s own contracts).
fn token_aligned_ms(segment_aligned_ms: i64, transcript_start_ms: u64, token_ms: u64) -> i64 {
    let token_ms = i64::try_from(token_ms).unwrap_or(i64::MAX);
    let start_ms = i64::try_from(transcript_start_ms).unwrap_or(i64::MAX);
    segment_aligned_ms.saturating_add(token_ms.saturating_sub(start_ms))
}

/// One record's tokens as comparable dedup keys, or `None` when this
/// record can never participate in a dedup match at all: no tokens,
/// or any single token normalizes to nothing.
fn dedup_tokens(transcript: &TranscriptChunk, segment_aligned_ms: i64) -> Option<Vec<DedupToken>> {
    if transcript.tokens.is_empty() {
        return None;
    }
    transcript
        .tokens
        .iter()
        .map(|token| {
            normalize_token(&token.token).map(|normalized| DedupToken {
                normalized,
                aligned_ms: token_aligned_ms(
                    segment_aligned_ms,
                    transcript.start_ms,
                    token.timestamp_ms,
                ),
            })
        })
        .collect()
}

/// Whether `mic` is an exact, position-for-position echo of `system`:
/// equal length, matching normalized text at every position, and
/// every paired token's aligned instant within `window_ms` of its
/// counterpart.
fn tokens_match_within_window(mic: &[DedupToken], system: &[DedupToken], window_ms: i64) -> bool {
    mic.len() == system.len()
        && mic.iter().zip(system).all(|(m, s)| {
            m.normalized == s.normalized && (m.aligned_ms - s.aligned_ms).abs() <= window_ms
        })
}

/// Conservative window a final record is held once both source
/// anchors are known, giving a same-instant final from the sibling
/// source time to arrive and interleave in the right order before
/// release.
///
/// Matches `ChunkerConfig::silence_split_after`'s default: the
/// largest gap the two independently-VAD-chunked sources can
/// plausibly disagree by while both are describing the same
/// otherwise-simultaneous speech. M6 PR-2 reuses this window for its
/// echo-dedup comparison.
pub(super) const UNRESOLVED_WINDOW: Duration = Duration::from_secs(5);

/// The release step's outcome, split by disposition: every record
/// [`AcceptedFinalBoundary::push`], [`AcceptedFinalBoundary::advance`],
/// or [`AcceptedFinalBoundary::finish`] resolves lands in exactly one
/// of these two lists, in aligned-time release order within each.
///
/// `suppressed` exists so the caller can still find a rejected echo's
/// `wal_seq` and mark its WAL entry accordingly — PR-1's boundary
/// silently dropped a record's caller-visible existence the moment it
/// resolved; PR-2's dedup decision must not repeat that for the WAL,
/// or a suppressed record's crash-recovery entry stays an orphan
/// forever (never marked flushed) and gets replayed into
/// `transcript.md` on the next recovery pass, precisely undoing the
/// dedup decision. `accepted` is what actually reaches
/// `transcript.md` and the `ChunkTranscribed` hook.
#[derive(Debug, Default)]
pub(super) struct Resolved {
    pub(super) accepted: Vec<AcceptedFinal>,
    pub(super) suppressed: Vec<AcceptedFinal>,
}

impl Resolved {
    pub(super) const fn is_empty(&self) -> bool {
        self.accepted.is_empty() && self.suppressed.is_empty()
    }

    fn single_accepted(record: AcceptedFinal) -> Self {
        Self {
            accepted: vec![record],
            suppressed: Vec::new(),
        }
    }
}

/// One record's disposition as it leaves the pending buffer.
enum ReleaseVerdict {
    Accepted(AcceptedFinal),
    Suppressed(AcceptedFinal),
}

/// Bounded anchor-aligned accepted-final buffer.
///
/// See the module docs for the release policy and for where the
/// horizon values every method takes come from. A buffered record is
/// only ever delayed, never dropped: [`Self::finish`] releases
/// whatever remains, unconditionally, in the same aligned-time order.
pub(super) struct AcceptedFinalBoundary {
    window: Duration,
    pending: Vec<Pending>,
    mic_progress_ms: Option<i64>,
    system_progress_ms: Option<i64>,
    next_seq: u64,
    /// Every `System` final's dedup tokens seen so far, retained for
    /// the rest of the session so a later `Mic` echo can still be
    /// matched against it even once its own segment has released.
    system_history: Vec<Vec<DedupToken>>,
}

impl AcceptedFinalBoundary {
    pub(super) const fn new() -> Self {
        Self {
            window: UNRESOLVED_WINDOW,
            pending: Vec::new(),
            mic_progress_ms: None,
            system_progress_ms: None,
            next_seq: 0,
            system_history: Vec::new(),
        }
    }

    /// Accept one final chunk.
    ///
    /// `record_horizon_ms` is the anchor-aligned instant this specific
    /// chunk's own segment *began* (backdated by the chunk's
    /// duration) — the sort key held records are ordered by.
    /// `progress_horizon_ms` is `record.source`'s *current*
    /// anchor-aligned captured-audio position (not backdated) — the
    /// per-source progress signal folded into the release watermark's
    /// minimum. Both `None` (at most one source anchor known) release
    /// the record immediately, unconditionally.
    ///
    /// Returns every record now resolved, in the order they must be
    /// persisted and dispatched.
    pub(super) fn push(
        &mut self,
        record: AcceptedFinal,
        record_horizon_ms: Option<i64>,
        progress_horizon_ms: Option<i64>,
    ) -> Resolved {
        let (Some(aligned_ms), Some(progress_ms)) = (record_horizon_ms, progress_horizon_ms) else {
            return Resolved::single_accepted(record);
        };
        let source = record.source;
        let seq = self.next_seq;
        self.next_seq += 1;
        self.advance_progress(source, progress_ms);
        if source == FrameSource::System {
            if let Some(tokens) = dedup_tokens(&record.transcript, aligned_ms) {
                self.system_history.push(tokens);
            }
        }
        self.pending.push(Pending {
            record,
            aligned_ms,
            seq,
        });
        self.release_resolved()
    }

    /// Advance `source`'s release watermark contribution using
    /// captured-audio progress that is not tied to any particular
    /// finalized chunk. `horizon_ms` is the same kind of
    /// anchor-aligned instant [`Self::push`]'s `progress_horizon_ms`
    /// is; `None` (dual anchors not yet known) is a no-op.
    ///
    /// A source can keep accumulating frames for up to
    /// `ChunkerConfig::max_chunk` — or longer, while its VAD keeps
    /// classifying speech — before its next final chunk appears.
    /// Driving the watermark only from [`Self::push`] would leave an
    /// already-resolvable held record sitting untouched for that
    /// entire span even though captured audio has clearly moved past
    /// it. Every captured frame is instead a progress signal for its
    /// own source. Returns every record the advance resolved, in the
    /// order [`Self::push`] would have.
    pub(super) fn advance(&mut self, source: FrameSource, horizon_ms: Option<i64>) -> Resolved {
        let Some(candidate_ms) = horizon_ms else {
            return Resolved::default();
        };
        self.advance_progress(source, candidate_ms);
        self.release_resolved()
    }

    fn advance_progress(&mut self, source: FrameSource, candidate_ms: i64) {
        let slot = match source {
            FrameSource::System => &mut self.system_progress_ms,
            FrameSource::Mic | FrameSource::Mixed => &mut self.mic_progress_ms,
        };
        *slot = Some(slot.map_or(candidate_ms, |current| current.max(candidate_ms)));
    }

    /// Release every record no longer eligible for a sibling-source
    /// comparison: its aligned time is at or before the current
    /// watermark, `min(mic_progress_ms, system_progress_ms) - window`.
    ///
    /// The minimum, not either source alone, is what proves *neither*
    /// source could still emit something earlier: one source racing
    /// ahead never releases the other's held record on its own.
    fn release_resolved(&mut self) -> Resolved {
        let (Some(mic_ms), Some(system_ms)) = (self.mic_progress_ms, self.system_progress_ms)
        else {
            return Resolved::default();
        };
        let safe_ms = mic_ms.min(system_ms);
        let watermark = safe_ms.saturating_sub(window_to_i64(self.window));
        self.pending.sort_by_key(|p| (p.aligned_ms, p.seq));
        let split = self.pending.partition_point(|p| p.aligned_ms <= watermark);
        let resolved: Vec<Pending> = self.pending.drain(..split).collect();
        let mut out = Resolved::default();
        for pending in resolved {
            match self.resolve_release(pending) {
                ReleaseVerdict::Accepted(record) => out.accepted.push(record),
                ReleaseVerdict::Suppressed(record) => out.suppressed.push(record),
            }
        }
        out
    }

    /// Classify one record leaving the pending buffer. Only ever
    /// suppresses `Mic` — see the module docs for the full rejection
    /// rule.
    fn resolve_release(&self, pending: Pending) -> ReleaseVerdict {
        let Pending {
            record, aligned_ms, ..
        } = pending;
        if record.source != FrameSource::Mic {
            return ReleaseVerdict::Accepted(record);
        }
        let Some(mic_tokens) = dedup_tokens(&record.transcript, aligned_ms) else {
            return ReleaseVerdict::Accepted(record);
        };
        let window_ms = window_to_i64(self.window);
        let is_echo = self
            .system_history
            .iter()
            .any(|system_tokens| tokens_match_within_window(&mic_tokens, system_tokens, window_ms));
        if is_echo {
            ReleaseVerdict::Suppressed(record)
        } else {
            ReleaseVerdict::Accepted(record)
        }
    }

    /// Release every remaining record, in aligned-time order,
    /// regardless of the window. Called once at session end so a
    /// buffered final is never lost — subject to the same dedup
    /// decision [`Self::release_resolved`] applies.
    pub(super) fn finish(mut self) -> Resolved {
        self.pending.sort_by_key(|p| (p.aligned_ms, p.seq));
        let pending = std::mem::take(&mut self.pending);
        let mut out = Resolved::default();
        for p in pending {
            match self.resolve_release(p) {
                ReleaseVerdict::Accepted(record) => out.accepted.push(record),
                ReleaseVerdict::Suppressed(record) => out.suppressed.push(record),
            }
        }
        out
    }
}

fn window_to_i64(window: Duration) -> i64 {
    // `UNRESOLVED_WINDOW` is a handful of seconds; this never
    // approaches `i64::MAX` milliseconds.
    i64::try_from(window.as_millis()).unwrap_or(i64::MAX)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use crate::types::TokenTiming;
    use pretty_assertions::assert_eq;

    fn record(source: FrameSource, text: &str) -> AcceptedFinal {
        AcceptedFinal {
            transcript: TranscriptChunk {
                text: text.to_string(),
                source,
                start_ms: 0,
                duration_ms: 0,
                language: None,
                tokens: Vec::new(),
            },
            source,
            ended_on: ChunkBoundary::SilenceAfterSpeech,
            wal_seq: None,
        }
    }

    fn texts(records: &[AcceptedFinal]) -> Vec<&str> {
        records.iter().map(|r| r.transcript.text.as_str()).collect()
    }

    /// Convenience for tests that do not care about the
    /// backdated-start vs. current-progress distinction: both push
    /// horizons equal, i.e. `back_ms == 0`.
    fn push_at(
        boundary: &mut AcceptedFinalBoundary,
        source: FrameSource,
        text: &str,
        aligned_ms: i64,
    ) -> Vec<AcceptedFinal> {
        boundary
            .push(record(source, text), Some(aligned_ms), Some(aligned_ms))
            .accepted
    }

    /// Build a record with explicit per-token timings, all relative
    /// to a `start_ms` of `0` so a token's `timestamp_ms` is directly
    /// its offset within the segment.
    fn record_with_tokens(
        source: FrameSource,
        text: &str,
        tokens: &[(&str, u64)],
    ) -> AcceptedFinal {
        AcceptedFinal {
            transcript: TranscriptChunk {
                text: text.to_string(),
                source,
                start_ms: 0,
                duration_ms: 0,
                language: None,
                tokens: tokens
                    .iter()
                    .map(|(token, timestamp_ms)| TokenTiming {
                        token: (*token).to_string(),
                        timestamp_ms: *timestamp_ms,
                    })
                    .collect(),
            },
            source,
            ended_on: ChunkBoundary::SilenceAfterSpeech,
            wal_seq: None,
        }
    }

    fn push_at_with_tokens(
        boundary: &mut AcceptedFinalBoundary,
        source: FrameSource,
        text: &str,
        tokens: &[(&str, u64)],
        aligned_ms: i64,
    ) -> Vec<AcceptedFinal> {
        boundary
            .push(
                record_with_tokens(source, text, tokens),
                Some(aligned_ms),
                Some(aligned_ms),
            )
            .accepted
    }

    fn owned_texts(records: &[AcceptedFinal]) -> Vec<String> {
        records.iter().map(|r| r.transcript.text.clone()).collect()
    }

    #[test]
    fn test_push_releases_immediately_when_horizon_is_none() {
        let mut boundary = AcceptedFinalBoundary::new();

        let released = boundary
            .push(record(FrameSource::Mic, "one"), None, None)
            .accepted;
        assert_eq!(texts(&released), vec!["one"]);

        let released = boundary
            .push(record(FrameSource::Mic, "two"), None, None)
            .accepted;
        assert_eq!(texts(&released), vec!["two"]);

        // Nothing was ever held back.
        assert!(texts(&boundary.finish().accepted).is_empty());
    }

    #[test]
    fn test_push_holds_dual_source_records_inside_the_unresolved_window() {
        let mut boundary = AcceptedFinalBoundary::new();

        let released = push_at(&mut boundary, FrameSource::Mic, "mic-first", 1_500);
        assert!(
            released.is_empty(),
            "first record has no watermark yet to clear it"
        );

        let released = push_at(&mut boundary, FrameSource::System, "sys-first", 1_000);
        assert!(
            released.is_empty(),
            "both records are within the unresolved window of each other"
        );
    }

    #[test]
    fn test_push_orders_dual_source_records_by_anchor_aligned_time_once_resolved() {
        let mut boundary = AcceptedFinalBoundary::new();

        // Pushed in reverse of their true aligned-time order.
        push_at(&mut boundary, FrameSource::Mic, "mic-early", 1_500);
        push_at(&mut boundary, FrameSource::System, "sys-early", 1_000);

        // Advance both sources' own progress far enough that their
        // MINIMUM clears the watermark for both held early records.
        push_at(&mut boundary, FrameSource::System, "sys-late", 10_000);
        let released = push_at(&mut boundary, FrameSource::Mic, "mic-late", 10_000);

        assert_eq!(
            texts(&released),
            vec!["sys-early", "mic-early"],
            "resolved records release in ascending anchor-aligned order, \
             not push order"
        );
        assert_eq!(
            texts(&boundary.finish().accepted),
            vec!["sys-late", "mic-late"],
            "records too close to the current minimum stay held"
        );
    }

    #[test]
    fn test_push_breaks_equal_aligned_time_ties_by_push_order() {
        let mut boundary = AcceptedFinalBoundary::new();

        push_at(
            &mut boundary,
            FrameSource::System,
            "system-pushed-first",
            1_000,
        );
        push_at(&mut boundary, FrameSource::Mic, "mic-pushed-second", 1_000);
        push_at(&mut boundary, FrameSource::System, "system-trigger", 31_000);
        let released = push_at(&mut boundary, FrameSource::Mic, "mic-trigger", 31_000);

        assert_eq!(
            texts(&released),
            vec!["system-pushed-first", "mic-pushed-second"],
        );
        assert_eq!(
            texts(&boundary.finish().accepted),
            vec!["system-trigger", "mic-trigger"]
        );
    }

    #[test]
    fn test_finish_releases_every_remaining_record_regardless_of_window() {
        let mut boundary = AcceptedFinalBoundary::new();

        let released = push_at(&mut boundary, FrameSource::Mic, "only", 1_000);
        assert!(
            released.is_empty(),
            "a single record never clears its own watermark"
        );

        let remaining = boundary.finish().accepted;
        assert_eq!(
            texts(&remaining),
            vec!["only"],
            "finish must not silently drop an unresolved record"
        );
    }

    #[test]
    fn test_finish_orders_multiple_remaining_records_by_aligned_time() {
        let mut boundary = AcceptedFinalBoundary::new();

        push_at(&mut boundary, FrameSource::Mic, "mic", 1_500);
        push_at(&mut boundary, FrameSource::System, "system", 1_000);

        assert_eq!(texts(&boundary.finish().accepted), vec!["system", "mic"]);
    }

    #[test]
    fn test_advance_releases_a_held_record_once_both_sources_progress_clears_the_window() {
        let mut boundary = AcceptedFinalBoundary::new();

        // System's only final of the whole test; nothing else from
        // system ever finalizes, so `push` alone could never resolve
        // this record.
        let released = push_at(
            &mut boundary,
            FrameSource::System,
            "system-only-final",
            1_000,
        );
        assert!(released.is_empty(), "no watermark yet to clear it");

        // Both sources advance via raw progress (no new final chunk
        // from either) until the minimum of their progress clears the
        // 5s unresolved window.
        boundary.advance(FrameSource::Mic, Some(1_000 + 40_000));
        let released = boundary
            .advance(FrameSource::System, Some(1_000 + 40_000))
            .accepted;

        assert_eq!(
            texts(&released),
            vec!["system-only-final"],
            "captured-audio progress, not just a new final chunk, must resolve a held record"
        );
    }

    #[test]
    fn test_advance_does_not_release_a_held_sibling_record_while_only_one_source_races_ahead() {
        let mut boundary = AcceptedFinalBoundary::new();

        // System pushes one final and holds; system's own progress is
        // exactly that final's aligned time — nothing more from
        // system yet.
        let released = push_at(&mut boundary, FrameSource::System, "system-held", 1_000);
        assert!(released.is_empty());

        // Mic races far ahead — 60s of mic-only progress — while
        // system reports nothing further. The release watermark uses
        // the MINIMUM of both sources' progress, so mic alone must
        // not release system's held record: system itself has not
        // corroborated reaching anywhere near that instant.
        let released = boundary.advance(FrameSource::Mic, Some(1_000 + 60_000));
        assert!(
            released.is_empty(),
            "one source racing ahead alone must not release a held sibling's record"
        );

        // Only once the SLOWER source (system) itself crosses the
        // window does the held record resolve.
        let released = boundary
            .advance(FrameSource::System, Some(1_000 + 6_000))
            .accepted;
        assert_eq!(texts(&released), vec!["system-held"]);
    }

    #[test]
    fn test_advance_does_not_release_a_held_record_still_inside_the_window() {
        let mut boundary = AcceptedFinalBoundary::new();

        push_at(&mut boundary, FrameSource::System, "held", 1_000);
        boundary.advance(FrameSource::Mic, Some(1_000 + 2_000));
        // Only 2s of progress on both sources: still inside the 5s
        // window.
        let released = boundary.advance(FrameSource::System, Some(1_000 + 2_000));

        assert!(released.is_empty());
        assert_eq!(texts(&boundary.finish().accepted), vec!["held"]);
    }

    #[test]
    fn test_advance_is_a_no_op_when_horizon_is_none() {
        let mut boundary = AcceptedFinalBoundary::new();

        let released = boundary.advance(FrameSource::Mic, None);

        assert!(released.is_empty());
        assert!(texts(&boundary.finish().accepted).is_empty());
    }

    #[test]
    fn test_dedup_rejects_mic_echo_within_window_at_known_offset() {
        let mut boundary = AcceptedFinalBoundary::new();

        push_at_with_tokens(
            &mut boundary,
            FrameSource::System,
            "system-original",
            &[("hello", 0), ("world", 400)],
            1_000,
        );
        let released = push_at_with_tokens(
            &mut boundary,
            FrameSource::Mic,
            "mic-echo",
            &[("hello", 0), ("world", 400)],
            1_800,
        );
        assert!(
            released.is_empty(),
            "both records are still within the unresolved window"
        );

        push_at_with_tokens(
            &mut boundary,
            FrameSource::System,
            "system-trigger",
            &[],
            31_000,
        );
        let released =
            push_at_with_tokens(&mut boundary, FrameSource::Mic, "mic-trigger", &[], 31_000);

        assert_eq!(
            texts(&released),
            vec!["system-original"],
            "the mic echo of an already-seen system final must be suppressed at release"
        );
        assert_eq!(
            texts(&boundary.finish().accepted),
            vec!["system-trigger", "mic-trigger"]
        );
    }

    #[test]
    fn test_dedup_retains_genuine_simultaneous_different_speech() {
        let mut boundary = AcceptedFinalBoundary::new();

        push_at_with_tokens(
            &mut boundary,
            FrameSource::System,
            "system-original",
            &[("hello", 0), ("world", 400)],
            1_000,
        );
        push_at_with_tokens(
            &mut boundary,
            FrameSource::Mic,
            "mic-different",
            &[("goodbye", 0), ("friend", 400)],
            1_000,
        );

        push_at_with_tokens(
            &mut boundary,
            FrameSource::System,
            "system-trigger",
            &[],
            31_000,
        );
        let released =
            push_at_with_tokens(&mut boundary, FrameSource::Mic, "mic-trigger", &[], 31_000);

        assert_eq!(
            texts(&released),
            vec!["system-original", "mic-different"],
            "different token text is not an echo, regardless of aligned timing"
        );
    }

    #[test]
    fn test_dedup_retains_mic_final_with_absent_tokens() {
        let mut boundary = AcceptedFinalBoundary::new();

        push_at_with_tokens(
            &mut boundary,
            FrameSource::System,
            "system-original",
            &[("hello", 0), ("world", 400)],
            1_000,
        );
        // No tokens at all, despite aligning exactly with the system
        // final above -- which would otherwise look like a perfect
        // echo.
        push_at_with_tokens(&mut boundary, FrameSource::Mic, "mic-no-tokens", &[], 1_000);

        push_at_with_tokens(
            &mut boundary,
            FrameSource::System,
            "system-trigger",
            &[],
            31_000,
        );
        let released =
            push_at_with_tokens(&mut boundary, FrameSource::Mic, "mic-trigger", &[], 31_000);

        assert_eq!(
            texts(&released),
            vec!["system-original", "mic-no-tokens"],
            "a mic final with no tokens can never be proven an echo"
        );
    }

    #[test]
    fn test_dedup_retains_when_normalization_produces_empty_token() {
        let mut boundary = AcceptedFinalBoundary::new();

        push_at_with_tokens(
            &mut boundary,
            FrameSource::System,
            "system-original",
            &[("hello", 0)],
            1_000,
        );
        // A lone SentencePiece marker normalizes to nothing, so it
        // can never be proven to match anything even though it is
        // the only token and aligns exactly.
        push_at_with_tokens(
            &mut boundary,
            FrameSource::Mic,
            "mic-blank-token",
            &[("\u{2581}", 0)],
            1_000,
        );

        push_at_with_tokens(
            &mut boundary,
            FrameSource::System,
            "system-trigger",
            &[],
            31_000,
        );
        let released =
            push_at_with_tokens(&mut boundary, FrameSource::Mic, "mic-trigger", &[], 31_000);

        assert_eq!(
            texts(&released),
            vec!["system-original", "mic-blank-token"],
            "a token that normalizes to nothing can never be proven an echo"
        );
    }

    #[test]
    fn test_dedup_never_rejects_a_system_final() {
        let mut boundary = AcceptedFinalBoundary::new();

        push_at_with_tokens(
            &mut boundary,
            FrameSource::System,
            "system-first",
            &[("hello", 0)],
            1_000,
        );
        // A second system final with tokens identical to the first --
        // dedup only ever targets `Mic`, so this must still release.
        push_at_with_tokens(
            &mut boundary,
            FrameSource::System,
            "system-second",
            &[("hello", 0)],
            1_100,
        );

        push_at_with_tokens(
            &mut boundary,
            FrameSource::System,
            "system-trigger",
            &[],
            31_000,
        );
        let released =
            push_at_with_tokens(&mut boundary, FrameSource::Mic, "mic-trigger", &[], 31_000);

        assert_eq!(
            texts(&released),
            vec!["system-first", "system-second"],
            "System finals are never subject to dedup rejection"
        );
    }

    #[test]
    fn test_dedup_applies_at_finish_as_well_as_release_resolved() {
        let mut boundary = AcceptedFinalBoundary::new();

        push_at_with_tokens(
            &mut boundary,
            FrameSource::System,
            "system-original",
            &[("hello", 0)],
            1_000,
        );
        push_at_with_tokens(
            &mut boundary,
            FrameSource::Mic,
            "mic-echo",
            &[("hello", 0)],
            1_000,
        );

        // The session ends before the watermark would ever have
        // released these on its own -- `finish` must still apply the
        // same dedup decision, not release everything unconditionally.
        let remaining = boundary.finish().accepted;

        assert_eq!(texts(&remaining), vec!["system-original"]);
    }

    #[test]
    fn test_dedup_rejected_mic_never_reaches_accepted_but_is_reported_suppressed() {
        let mut boundary = AcceptedFinalBoundary::new();
        let mut all_accepted: Vec<String> = Vec::new();
        let mut all_suppressed: Vec<String> = Vec::new();

        let r = boundary.push(
            record_with_tokens(
                FrameSource::System,
                "system-original",
                &[("hello", 0), ("world", 400)],
            ),
            Some(1_000),
            Some(1_000),
        );
        all_accepted.extend(owned_texts(&r.accepted));
        all_suppressed.extend(owned_texts(&r.suppressed));

        let r = boundary.push(
            record_with_tokens(
                FrameSource::Mic,
                "mic-echo",
                &[("hello", 0), ("world", 400)],
            ),
            Some(1_800),
            Some(1_800),
        );
        all_accepted.extend(owned_texts(&r.accepted));
        all_suppressed.extend(owned_texts(&r.suppressed));

        let r = boundary.push(
            record_with_tokens(FrameSource::System, "system-trigger", &[]),
            Some(31_000),
            Some(31_000),
        );
        all_accepted.extend(owned_texts(&r.accepted));
        all_suppressed.extend(owned_texts(&r.suppressed));

        let r = boundary.push(
            record_with_tokens(FrameSource::Mic, "mic-trigger", &[]),
            Some(31_000),
            Some(31_000),
        );
        all_accepted.extend(owned_texts(&r.accepted));
        all_suppressed.extend(owned_texts(&r.suppressed));

        let last = boundary.finish();
        all_accepted.extend(owned_texts(&last.accepted));
        all_suppressed.extend(owned_texts(&last.suppressed));

        assert!(
            !all_accepted.iter().any(|text| text == "mic-echo"),
            "a rejected mic echo must never reach the accepted/persisted list"
        );
        assert_eq!(
            all_suppressed,
            vec!["mic-echo"],
            "the rejected echo must still be reported so its WAL entry can be marked suppressed"
        );
        assert_eq!(
            all_accepted,
            vec!["system-original", "system-trigger", "mic-trigger"]
        );
    }

    #[test]
    fn test_dedup_retains_mic_when_matching_tokens_align_beyond_the_window() {
        // Same token text as
        // `test_dedup_rejects_mic_echo_within_window_at_known_offset`,
        // but the mic final's own anchor-aligned instant is 6s beyond
        // the system final's -- outside `UNRESOLVED_WINDOW` -- so
        // exact token-text equality alone must never suppress it.
        let mut boundary = AcceptedFinalBoundary::new();
        let mut all_accepted: Vec<String> = Vec::new();
        let mut all_suppressed: Vec<String> = Vec::new();

        let r = boundary.push(
            record_with_tokens(
                FrameSource::System,
                "system-original",
                &[("hello", 0), ("world", 400)],
            ),
            Some(1_000),
            Some(1_000),
        );
        all_accepted.extend(owned_texts(&r.accepted));
        all_suppressed.extend(owned_texts(&r.suppressed));

        let r = boundary.push(
            record_with_tokens(
                FrameSource::Mic,
                "mic-far-echo",
                &[("hello", 0), ("world", 400)],
            ),
            Some(1_000 + 6_000),
            Some(1_000 + 6_000),
        );
        all_accepted.extend(owned_texts(&r.accepted));
        all_suppressed.extend(owned_texts(&r.suppressed));

        let r = boundary.push(
            record_with_tokens(FrameSource::System, "system-trigger", &[]),
            Some(50_000),
            Some(50_000),
        );
        all_accepted.extend(owned_texts(&r.accepted));
        all_suppressed.extend(owned_texts(&r.suppressed));

        let r = boundary.push(
            record_with_tokens(FrameSource::Mic, "mic-trigger", &[]),
            Some(50_000),
            Some(50_000),
        );
        all_accepted.extend(owned_texts(&r.accepted));
        all_suppressed.extend(owned_texts(&r.suppressed));

        let last = boundary.finish();
        all_accepted.extend(owned_texts(&last.accepted));
        all_suppressed.extend(owned_texts(&last.suppressed));

        assert!(
            all_suppressed.is_empty(),
            "matching token text alone, without a within-window aligned instant, must never suppress"
        );
        assert_eq!(
            all_accepted,
            vec![
                "system-original",
                "mic-far-echo",
                "system-trigger",
                "mic-trigger"
            ]
        );
    }
}
