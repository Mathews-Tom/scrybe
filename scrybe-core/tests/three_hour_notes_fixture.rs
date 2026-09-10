#![allow(clippy::panic)]

use scrybe_core::notes_segments::parse_canonical_transcript;

#[test]
fn preserves_every_canonical_segment_boundary() {
    let fixture = include_str!("fixtures/three-hour-canonical-transcript.md");
    let segments = match parse_canonical_transcript(fixture) {
        Ok(segments) => segments,
        Err(error) => panic!("fixture must parse: {error}"),
    };

    assert_eq!(segments.len(), 180);
    assert_eq!(segments.first().map(|segment| segment.start_ms), Some(0));
    assert_eq!(
        segments.last().map(|segment| segment.start_ms),
        Some(10_740_000)
    );
    assert_eq!(segments.last().map(|segment| segment.ordinal), Some(180));
}
