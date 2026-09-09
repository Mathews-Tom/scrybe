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
//! that decision at the same release point.

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
}

impl AcceptedFinalBoundary {
    pub(super) const fn new() -> Self {
        Self {
            window: UNRESOLVED_WINDOW,
            pending: Vec::new(),
            mic_progress_ms: None,
            system_progress_ms: None,
            next_seq: 0,
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
    ) -> Vec<AcceptedFinal> {
        let (Some(aligned_ms), Some(progress_ms)) = (record_horizon_ms, progress_horizon_ms) else {
            return vec![record];
        };
        let source = record.source;
        let seq = self.next_seq;
        self.next_seq += 1;
        self.advance_progress(source, progress_ms);
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
    pub(super) fn advance(
        &mut self,
        source: FrameSource,
        horizon_ms: Option<i64>,
    ) -> Vec<AcceptedFinal> {
        let Some(candidate_ms) = horizon_ms else {
            return Vec::new();
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
    fn release_resolved(&mut self) -> Vec<AcceptedFinal> {
        let (Some(mic_ms), Some(system_ms)) = (self.mic_progress_ms, self.system_progress_ms)
        else {
            return Vec::new();
        };
        let safe_ms = mic_ms.min(system_ms);
        let watermark = safe_ms.saturating_sub(window_to_i64(self.window));
        self.pending.sort_by_key(|p| (p.aligned_ms, p.seq));
        let split = self.pending.partition_point(|p| p.aligned_ms <= watermark);
        self.pending.drain(..split).map(|p| p.record).collect()
    }

    /// Release every remaining record, in aligned-time order,
    /// regardless of the window. Called once at session end so a
    /// buffered final is never lost.
    pub(super) fn finish(mut self) -> Vec<AcceptedFinal> {
        self.pending.sort_by_key(|p| (p.aligned_ms, p.seq));
        self.pending.into_iter().map(|p| p.record).collect()
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
        boundary.push(record(source, text), Some(aligned_ms), Some(aligned_ms))
    }

    #[test]
    fn test_push_releases_immediately_when_horizon_is_none() {
        let mut boundary = AcceptedFinalBoundary::new();

        let released = boundary.push(record(FrameSource::Mic, "one"), None, None);
        assert_eq!(texts(&released), vec!["one"]);

        let released = boundary.push(record(FrameSource::Mic, "two"), None, None);
        assert_eq!(texts(&released), vec!["two"]);

        // Nothing was ever held back.
        assert!(texts(&boundary.finish()).is_empty());
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
            texts(&boundary.finish()),
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
            texts(&boundary.finish()),
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

        let remaining = boundary.finish();
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

        assert_eq!(texts(&boundary.finish()), vec!["system", "mic"]);
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
        let released = boundary.advance(FrameSource::System, Some(1_000 + 40_000));

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
        let released = boundary.advance(FrameSource::System, Some(1_000 + 6_000));
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
        assert_eq!(texts(&boundary.finish()), vec!["held"]);
    }

    #[test]
    fn test_advance_is_a_no_op_when_horizon_is_none() {
        let mut boundary = AcceptedFinalBoundary::new();

        let released = boundary.advance(FrameSource::Mic, None);

        assert!(released.is_empty());
        assert!(texts(&boundary.finish()).is_empty());
    }
}
