// Copyright 2026 Mathews Tom
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     https://www.apache.org/licenses/LICENSE-2.0

//! Canonical-transcript segmentation and boundary-aligned packing for
//! M7 long-meeting notes (`.docs/superpowers/specs/2026-09-10-long-meeting-notes-design.md`).
//!
//! `transcript.md` is the sole M7 input artifact: M6's accepted-final
//! boundary durably appends every accepted final to it in
//! durable/hook order via `notes::render_transcript_header` and
//! `notes::render_transcript_line`. [`parse_canonical_transcript`]
//! parses that rendered markdown back into [`TranscriptSegment`]
//! values; [`pack_segments`] groups them into map-reduce input chunks
//! without ever splitting one segment across two chunks.
//!
//! This module is pure: it takes a token-counting closure rather than
//! loading a tokenizer itself. Wiring a real `tokenizers`-backed
//! counter and the request-cap/omitted-middle path is M7 PR-2/PR-3.
//!
//! Tier-3 internal, like `notes.rs`: the `transcript.md` line format
//! is documented in `docs/system-design.md` §5 but is not a stability
//! contract. This parser and `notes::render_transcript_line` are
//! tested as a pair and change together.

/// One parsed line from the canonical transcript body: one accepted
/// final.
///
/// `speaker` is the rendered label exactly as `transcript.md` carries
/// it (e.g. `"Me"`, `"Them"`, `"Unknown"`, or a diarized display
/// name) — this module only round-trips what was rendered; it does
/// not re-derive a `SpeakerLabel`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TranscriptSegment {
    /// 1-indexed position of this segment within the parsed
    /// transcript, in accepted-final order.
    pub ordinal: usize,
    /// Rendered speaker label.
    pub speaker: String,
    /// Start of the segment relative to session start, in
    /// milliseconds. Parsed from the rendered `HH:MM:SS` timestamp
    /// (`notes::format_hms_ms`'s inverse), so sub-second precision is
    /// not recoverable from the canonical artifact.
    pub start_ms: u64,
    /// Segment text exactly as rendered (already trimmed by
    /// `notes::render_transcript_line` before it was written).
    pub text: String,
}

/// A canonical transcript body line did not match the
/// `**speaker** [HH:MM:SS]: text` shape
/// `notes::render_transcript_line` produces.
///
/// Carries the 1-indexed source line number so the error names the
/// malformed line. Such a line is never silently skipped because that
/// would silently lose speech.
#[derive(thiserror::Error, Debug, Eq, PartialEq)]
#[error("malformed transcript line {line}: {reason}")]
pub struct TranscriptParseError {
    pub line: usize,
    pub reason: String,
}

fn malformed(line: usize, reason: &str) -> TranscriptParseError {
    TranscriptParseError {
        line,
        reason: reason.to_string(),
    }
}

/// Parse a complete canonical `transcript.md` document into ordered
/// segments.
///
/// Skips the two-line static header (`notes::render_transcript_header`
/// writes a title line then a `*started[ — ended]*` line) and any
/// blank lines, including the blank line the header always emits
/// before the body. Every remaining nonblank line must match the
/// rendered segment shape or parsing fails, naming the malformed
/// line's 1-indexed number — a malformed nonblank line is never
/// treated as an ignorable gap.
///
/// # Errors
///
/// Returns [`TranscriptParseError`] naming the first nonblank body
/// line that does not match `**speaker** [HH:MM:SS]: text`.
pub fn parse_canonical_transcript(
    markdown: &str,
) -> Result<Vec<TranscriptSegment>, TranscriptParseError> {
    let lines: Vec<&str> = markdown.lines().collect();
    let body_start = skip_header(&lines);

    let mut segments = Vec::new();
    for (offset, raw) in lines[body_start..].iter().enumerate() {
        if raw.trim().is_empty() {
            continue;
        }
        let line_no = body_start + offset + 1;
        let (speaker, start_ms, text) = parse_segment_line(raw, line_no)?;
        segments.push(TranscriptSegment {
            ordinal: segments.len() + 1,
            speaker,
            start_ms,
            text,
        });
    }
    Ok(segments)
}

/// Index of the first line after the static header: the title line
/// (`# ...`) if present, the `*timestamp[ — timestamp]*` metadata line
/// if present, then every immediately-following blank line.
fn skip_header(lines: &[&str]) -> usize {
    let mut idx = 0;
    if lines.first().is_some_and(|line| line.starts_with("# ")) {
        idx += 1;
    }
    if lines
        .get(idx)
        .is_some_and(|line| is_timestamp_meta_line(line))
    {
        idx += 1;
    }
    while lines.get(idx).is_some_and(|line| line.trim().is_empty()) {
        idx += 1;
    }
    idx
}

fn is_timestamp_meta_line(line: &str) -> bool {
    let trimmed = line.trim();
    trimmed.len() >= 2 && trimmed.starts_with('*') && trimmed.ends_with('*')
}

/// Parse one rendered `**speaker** [HH:MM:SS]: text` line into its
/// three fields.
fn parse_segment_line(
    line: &str,
    line_no: usize,
) -> Result<(String, u64, String), TranscriptParseError> {
    let after_open = line
        .strip_prefix("**")
        .ok_or_else(|| malformed(line_no, "missing opening speaker marker `**`"))?;
    let (speaker, after_speaker) = after_open
        .split_once("** [")
        .ok_or_else(|| malformed(line_no, "missing speaker/timestamp marker `** [`"))?;
    if speaker.is_empty() {
        return Err(malformed(line_no, "empty speaker label"));
    }
    let (timestamp, text) = after_speaker
        .split_once("]: ")
        .ok_or_else(|| malformed(line_no, "missing timestamp delimiter `]: `"))?;
    let start_ms = parse_hms(timestamp)
        .ok_or_else(|| malformed(line_no, "invalid timestamp, expected HH:MM:SS"))?;
    Ok((speaker.to_string(), start_ms, text.to_string()))
}

/// Inverse of `notes::format_hms_ms`: `"HH:MM:SS"` → milliseconds.
/// `HH` may be more than two digits for recordings past 99 hours; `MM`
/// and `SS` must each be in `0..60`.
fn parse_hms(raw: &str) -> Option<u64> {
    let mut parts = raw.split(':');
    let h: u64 = parts.next()?.parse().ok()?;
    let m: u64 = parts.next()?.parse().ok()?;
    let s: u64 = parts.next()?.parse().ok()?;
    if parts.next().is_some() || m >= 60 || s >= 60 {
        return None;
    }
    Some((h * 3_600 + m * 60 + s) * 1_000)
}

/// One packed map chunk: the segments it introduces in transcript
/// order, plus whole trailing segments repeated from the previous
/// chunk for context continuity.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SegmentChunk<'a> {
    /// Segments first introduced by this chunk, in transcript order.
    /// Never empty, and never a partial segment — a chunk always
    /// holds at least the one segment that started it, even when that
    /// single segment alone exceeds `target_tokens`.
    pub segments: Vec<&'a TranscriptSegment>,
    /// Trailing segments repeated from the previous chunk's
    /// `segments`. Always a suffix of that chunk's own new segments
    /// (never re-including a previous overlap), and always empty for
    /// the first chunk.
    pub overlap: Vec<&'a TranscriptSegment>,
}

/// Pack `segments` into ordered map chunks using a caller-supplied
/// token counter.
///
/// A chunk grows one segment at a time; adding the next segment stops
/// and starts a new chunk only when the chunk already holds at least
/// one segment and the addition would push its running token count
/// (over its own new segments only) past `target_tokens`. This means:
///
/// - a chunk boundary always falls between two segments, never inside
///   one;
/// - a single segment whose own count already exceeds `target_tokens`
///   still becomes its own one-segment chunk rather than being split
///   or dropped (the request-cap and gap-record handling for an
///   over-cap segment is M7 PR-2/PR-3's concern, not this packer's);
/// - `overlap_segments` whole trailing segments from a closed chunk's
///   own new segments carry into the following chunk's `overlap`,
///   added on top of (not counted against) that chunk's own target
///   budget.
///
/// Returns one [`SegmentChunk`] per group in transcript order, or an
/// empty vector when `segments` is empty.
#[must_use]
pub fn pack_segments<'a, F>(
    segments: &'a [TranscriptSegment],
    target_tokens: u32,
    overlap_segments: u32,
    mut count_tokens: F,
) -> Vec<SegmentChunk<'a>>
where
    F: FnMut(&str) -> u32,
{
    let overlap_len = overlap_segments as usize;
    let target = u64::from(target_tokens);

    let mut chunks = Vec::new();
    let mut current: Vec<&'a TranscriptSegment> = Vec::new();
    let mut current_tokens: u64 = 0;
    let mut pending_overlap: Vec<&'a TranscriptSegment> = Vec::new();

    for segment in segments {
        let segment_tokens = u64::from(count_tokens(&segment.text));
        let would_overflow = !current.is_empty() && current_tokens + segment_tokens > target;
        if would_overflow {
            let overlap = std::mem::take(&mut pending_overlap);
            pending_overlap = trailing(&current, overlap_len);
            chunks.push(SegmentChunk {
                segments: std::mem::take(&mut current),
                overlap,
            });
            current_tokens = 0;
        }
        current.push(segment);
        current_tokens += segment_tokens;
    }
    if !current.is_empty() {
        chunks.push(SegmentChunk {
            segments: current,
            overlap: pending_overlap,
        });
    }
    chunks
}

/// Last `n` elements of `items`, or all of them if `items.len() <= n`.
fn trailing<'a>(items: &[&'a TranscriptSegment], n: usize) -> Vec<&'a TranscriptSegment> {
    let start = items.len().saturating_sub(n);
    items[start..].to_vec()
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    fn segment(ordinal: usize, speaker: &str, start_ms: u64, text: &str) -> TranscriptSegment {
        TranscriptSegment {
            ordinal,
            speaker: speaker.to_string(),
            start_ms,
            text: text.to_string(),
        }
    }

    // Word count stands in for a real tokenizer in these pure-packer
    // tests; PR-3 wires a `tokenizers`-backed counter.
    fn word_count(text: &str) -> u32 {
        u32::try_from(text.split_whitespace().count()).unwrap()
    }

    fn canonical_header() -> &'static str {
        "# Acme discovery\n*2026-04-29 14:30*\n\n"
    }

    #[test]
    fn test_parse_canonical_transcript_parses_lines_in_order() {
        let markdown = format!(
            "{}**Me** [00:00:03]: Hi there.\n**Them** [00:00:05]: Sure thing.\n",
            canonical_header()
        );

        let segments = parse_canonical_transcript(&markdown).unwrap();

        assert_eq!(
            segments,
            vec![
                segment(1, "Me", 3_000, "Hi there."),
                segment(2, "Them", 5_000, "Sure thing."),
            ]
        );
    }

    #[test]
    fn test_parse_canonical_transcript_skips_static_header() {
        let markdown = format!("{}**Me** [00:00:00]: Hello.\n", canonical_header());

        let segments = parse_canonical_transcript(&markdown).unwrap();

        assert_eq!(segments.len(), 1);
        assert_eq!(segments[0].text, "Hello.");
    }

    #[test]
    fn test_parse_canonical_transcript_skips_header_with_ended_at_range() {
        let markdown = "# Standup\n*2026-04-29 09:00 — 09:15*\n\n**Me** [00:00:00]: Hi.\n";

        let segments = parse_canonical_transcript(markdown).unwrap();

        assert_eq!(segments, vec![segment(1, "Me", 0, "Hi.")]);
    }

    #[test]
    fn test_parse_canonical_transcript_empty_body_yields_no_segments() {
        let markdown = canonical_header();

        let segments = parse_canonical_transcript(markdown).unwrap();

        assert!(segments.is_empty());
    }

    #[test]
    fn test_parse_canonical_transcript_empty_string_yields_no_segments() {
        let segments = parse_canonical_transcript("").unwrap();

        assert!(segments.is_empty());
    }

    #[test]
    fn test_parse_canonical_transcript_rejects_malformed_nonblank_line_with_line_number() {
        let markdown = format!(
            "{}**Me** [00:00:00]: Hi.\nsomeone forgot the markers here\n",
            canonical_header()
        );

        let err = parse_canonical_transcript(&markdown).unwrap_err();

        assert_eq!(err.line, 5);
    }

    #[test]
    fn test_parse_canonical_transcript_rejects_missing_timestamp_delimiter() {
        let markdown = format!("{}**Me** [00:00:00] Hi.\n", canonical_header());

        let err = parse_canonical_transcript(&markdown).unwrap_err();

        assert_eq!(err.line, 4);
        assert!(err.reason.contains("timestamp"));
    }

    #[test]
    fn test_parse_canonical_transcript_rejects_invalid_hms_timestamp() {
        let markdown = format!("{}**Me** [not-a-time]: Hi.\n", canonical_header());

        let err = parse_canonical_transcript(&markdown).unwrap_err();

        assert_eq!(err.line, 4);
    }

    #[test]
    fn test_parse_canonical_transcript_ignores_blank_lines_between_segments() {
        let markdown = format!(
            "{}**Me** [00:00:00]: Hi.\n\n**Them** [00:00:01]: Hello.\n",
            canonical_header()
        );

        let segments = parse_canonical_transcript(&markdown).unwrap();

        assert_eq!(segments.len(), 2);
    }

    #[test]
    fn test_pack_segments_empty_input_yields_no_chunks() {
        let segments: Vec<TranscriptSegment> = Vec::new();

        let chunks = pack_segments(&segments, 10, 1, word_count);

        assert!(chunks.is_empty());
    }

    #[test]
    fn test_pack_segments_keeps_all_segments_in_one_chunk_when_under_target() {
        let segments = vec![
            segment(1, "Me", 0, "one two"),
            segment(2, "Them", 1_000, "three four"),
        ];

        let chunks = pack_segments(&segments, 100, 0, word_count);

        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].segments, vec![&segments[0], &segments[1]]);
        assert!(chunks[0].overlap.is_empty());
    }

    #[test]
    fn test_pack_segments_splits_exactly_at_target_boundary() {
        // Each segment is 2 words; target 4 means exactly two segments
        // (4 words) fit before the third would push the running count
        // to 6, over target.
        let segments = vec![
            segment(1, "Me", 0, "a b"),
            segment(2, "Me", 1_000, "c d"),
            segment(3, "Me", 2_000, "e f"),
        ];

        let chunks = pack_segments(&segments, 4, 0, word_count);

        assert_eq!(chunks.len(), 2);
        assert_eq!(chunks[0].segments, vec![&segments[0], &segments[1]]);
        assert_eq!(chunks[1].segments, vec![&segments[2]]);
    }

    #[test]
    fn test_pack_segments_one_token_over_target_starts_new_chunk() {
        // Segment 1 is exactly at target (4 words); segment 2 (1 word)
        // would push the running count to 5, one over target.
        let segments = vec![segment(1, "Me", 0, "a b c d"), segment(2, "Me", 1_000, "e")];

        let chunks = pack_segments(&segments, 4, 0, word_count);

        assert_eq!(chunks.len(), 2);
        assert_eq!(chunks[0].segments, vec![&segments[0]]);
        assert_eq!(chunks[1].segments, vec![&segments[1]]);
    }

    #[test]
    fn test_pack_segments_never_splits_a_single_oversized_segment() {
        let segments = vec![
            segment(1, "Me", 0, "one two three four five"),
            segment(2, "Me", 1_000, "six"),
        ];

        // target smaller than segment 1's own token count (5 words).
        let chunks = pack_segments(&segments, 2, 0, word_count);

        assert_eq!(chunks.len(), 2);
        assert_eq!(chunks[0].segments, vec![&segments[0]]);
        assert_eq!(chunks[1].segments, vec![&segments[1]]);
    }

    #[test]
    fn test_pack_segments_carries_whole_trailing_overlap_into_next_chunk() {
        let segments = vec![
            segment(1, "Me", 0, "a b"),
            segment(2, "Me", 1_000, "c d"),
            segment(3, "Me", 2_000, "e f"),
        ];

        let chunks = pack_segments(&segments, 4, 1, word_count);

        assert_eq!(chunks.len(), 2);
        assert_eq!(chunks[0].segments, vec![&segments[0], &segments[1]]);
        assert!(chunks[0].overlap.is_empty());
        assert_eq!(chunks[1].segments, vec![&segments[2]]);
        // Whole trailing segment from chunk 0, not a partial segment.
        assert_eq!(chunks[1].overlap, vec![&segments[1]]);
    }

    #[test]
    fn test_pack_segments_overlap_clamps_to_available_segments() {
        let segments = vec![segment(1, "Me", 0, "a b"), segment(2, "Me", 1_000, "c d")];

        // overlap_segments (5) exceeds the single segment in chunk 0.
        let chunks = pack_segments(&segments, 2, 5, word_count);

        assert_eq!(chunks.len(), 2);
        assert_eq!(chunks[1].overlap, vec![&segments[0]]);
    }

    #[test]
    fn test_pack_segments_no_split_identity_reconstructs_original_order() {
        let segments: Vec<TranscriptSegment> = (0..10)
            .map(|i| {
                segment(
                    i + 1,
                    "Me",
                    u64::try_from(i).expect("test indices fit u64") * 1_000,
                    "word word",
                )
            })
            .collect();

        let chunks = pack_segments(&segments, 4, 1, word_count);

        let reconstructed: Vec<&TranscriptSegment> = chunks
            .iter()
            .flat_map(|chunk| chunk.segments.iter().copied())
            .collect();
        assert_eq!(reconstructed, segments.iter().collect::<Vec<_>>());
    }
}
