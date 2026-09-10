// Copyright 2026 Mathews Tom
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     https://www.apache.org/licenses/LICENSE-2.0

//! Markdown rendering for `transcript.md` and the LLM prompt that
//! produces `notes.md`. The default template is baked in and selected
//! through `config.notes.template`.
//!
//! Tier-3 internal: `transcript.md` line format is documented in
//! `docs/system-design.md` §5 but is not a stability contract.

use std::fmt::Write as _;

use chrono::{DateTime, Utc};

use crate::context::MeetingContext;
use crate::notes_map_reduce::{render_processing_gaps, MapGroup, ProcessingGap};
use crate::notes_segments::TranscriptSegment;
use crate::types::{AttributedChunk, SpeakerLabel};

/// Render the static header for a `transcript.md` file. Called once
/// when the session folder is created; subsequent chunks are appended
/// via `render_transcript_line`.
#[must_use]
pub fn render_transcript_header(
    title: Option<&str>,
    started_at: DateTime<Utc>,
    ended_at: Option<DateTime<Utc>>,
) -> String {
    let title_line = title.unwrap_or("Untitled session");
    let mut out = format!("# {title_line}\n");
    let started = started_at.format("%Y-%m-%d %H:%M");
    if let Some(end) = ended_at {
        let _ = writeln!(out, "*{} — {}*", started, end.format("%H:%M"));
    } else {
        let _ = writeln!(out, "*{started}*");
    }
    out.push('\n');
    out
}

/// Format a single attributed chunk as one transcript line. Lines end
/// with a newline so callers can pass the rendered string straight to
/// `storage::append_durable`.
#[must_use]
pub fn render_transcript_line(chunk: &AttributedChunk) -> String {
    let speaker = label_for(&chunk.speaker);
    let timestamp = format_hms_ms(chunk.chunk.start_ms);
    let text = chunk.chunk.text.trim();
    format!("**{speaker}** [{timestamp}]: {text}\n")
}

const fn label_for(label: &SpeakerLabel) -> &'static str {
    match label {
        SpeakerLabel::Me => "Me",
        SpeakerLabel::Them => "Them",
        SpeakerLabel::Named(_) => "Named",
        SpeakerLabel::Unknown => "Unknown",
    }
}

fn format_hms_ms(start_ms: u64) -> String {
    let total_secs = start_ms / 1_000;
    let h = total_secs / 3_600;
    let m = (total_secs % 3_600) / 60;
    let s = total_secs % 60;
    format!("{h:02}:{m:02}:{s:02}")
}

/// The only shipped named notes structure.
pub const DEFAULT_TEMPLATE: &str = "default";

/// Reject a template name before capture starts.
///
/// # Errors
///
/// Returns a configuration error for every unknown template name.
pub fn validate_template(name: &str) -> Result<(), crate::error::ConfigError> {
    if name == DEFAULT_TEMPLATE {
        Ok(())
    } else {
        Err(crate::error::ConfigError::Invalid {
            key: "notes.template".to_string(),
            reason: format!("unknown template {name:?}; expected {DEFAULT_TEMPLATE:?}"),
        })
    }
}

/// Render the LLM prompt that produces `notes.md`.
///
/// The default template is grep-friendly: each section is a header so
/// the user can re-run extraction with `awk` or `pandoc` if they
/// prefer not to rely on the LLM's structuring.
#[must_use]
pub fn render_notes_prompt(transcript: &str, ctx: &MeetingContext) -> String {
    let mut out = String::new();
    out.push_str("You are summarizing a meeting transcript into structured notes.\n");
    out.push_str("Produce markdown with these sections, in order:\n");
    out.push_str("- TL;DR (2–3 sentences)\n");
    out.push_str("- Action items (bulleted)\n");
    out.push_str("- Decisions (bulleted)\n");
    out.push_str("- Follow-ups (bulleted)\n\n");

    if let Some(title) = ctx.title.as_deref() {
        let _ = writeln!(out, "Meeting title: {title}");
    }
    if !ctx.attendees.is_empty() {
        let _ = writeln!(out, "Attendees: {}", ctx.attendees.join(", "));
    }
    if let Some(agenda) = ctx.agenda.as_deref() {
        let _ = writeln!(out, "Agenda: {agenda}");
    }
    if let Some(language) = ctx.language.as_ref() {
        let _ = writeln!(out, "Language: {language}");
    }

    out.push_str("\n--- TRANSCRIPT ---\n");
    out.push_str(transcript);
    if !transcript.ends_with('\n') {
        out.push('\n');
    }
    out.push_str("--- END TRANSCRIPT ---\n");
    out
}

/// Render a map request over complete canonical transcript segments.
#[must_use]
pub fn render_map_prompt(
    overlap: &[&TranscriptSegment],
    segments: &[&TranscriptSegment],
) -> String {
    let mut out = String::from(
        "Extract factual bullets from the canonical transcript segments below.\n\
         The transcript is ground truth. Preserve speaker attribution.\n\
         Do not infer, invent, or turn meeting context into facts.\n\
         Return bullets only.\n",
    );
    if !overlap.is_empty() {
        out.push_str("\n--- CONTEXT OVERLAP (already covered) ---\n");
        render_segments(&mut out, overlap);
    }
    out.push_str("\n--- TRANSCRIPT SEGMENTS (new coverage) ---\n");
    render_segments(&mut out, segments);
    out.push_str("--- END TRANSCRIPT SEGMENTS ---\n");
    out
}

/// Render the final notes request from factual map bullets and named gaps.
#[must_use]
pub fn render_reduce_prompt(
    groups: &[MapGroup],
    gaps: &[ProcessingGap],
    context: &MeetingContext,
) -> String {
    let mut out = String::from(
        "Write structured meeting notes from the factual map bullets below.\n\
         The map bullets are the only factual source. User context supplies intent only;\n\
         do not treat it as evidence. Preserve speaker attribution and do not invent facts.\n\
         Produce markdown with these sections, in order:\n\
         - TL;DR (2–3 sentences)\n\
         - Action items (bulleted)\n\
         - Decisions (bulleted)\n\
         - Follow-ups (bulleted)\n",
    );
    if let Some(title) = context.title.as_deref() {
        let _ = writeln!(out, "\nRequested meeting title: {title}");
    }
    if !context.attendees.is_empty() {
        let _ = writeln!(
            out,
            "Requested attendee context: {}",
            context.attendees.join(", ")
        );
    }
    if let Some(agenda) = context.agenda.as_deref() {
        let _ = writeln!(out, "Requested agenda context: {agenda}");
    }
    out.push_str("\n--- FACTUAL MAP BULLETS ---\n");
    for group in groups {
        if group.start_ordinal == 0 {
            let _ = writeln!(out, "{}", group.bullets.trim());
        } else {
            let _ = writeln!(
                out,
                "[Transcript segments {}–{}]\n{}",
                group.start_ordinal,
                group.end_ordinal,
                group.bullets.trim()
            );
        }
    }
    for gap in gaps {
        let _ = writeln!(
            out,
            "[Processing gap: transcript segments {}–{} ({})]",
            gap.start_ordinal, gap.end_ordinal, gap.reason
        );
    }
    out.push_str("--- END FACTUAL MAP BULLETS ---\n");
    out
}

fn render_segments(out: &mut String, segments: &[&TranscriptSegment]) {
    for segment in segments {
        let _ = writeln!(
            out,
            "[{}] **{}** [{}]: {}",
            segment.ordinal,
            segment.speaker,
            format_hms_ms(segment.start_ms),
            segment.text
        );
    }
}

/// Render the LLM prompt that produces the folder/title slug source.
#[must_use]
pub fn render_title_prompt(transcript: &str) -> String {
    let mut out = String::new();
    out.push_str("Create a short, factual title for this transcript.\n");
    out.push_str("Return only the title: no markdown, no quotes, no date.\n");
    out.push_str("Use 3 to 8 words. Prefer the concrete topic over generic words.\n\n");
    out.push_str("--- TRANSCRIPT ---\n");
    out.push_str(transcript);
    if !transcript.ends_with('\n') {
        out.push('\n');
    }
    out.push_str("--- END TRANSCRIPT ---\n");
    out
}

/// Normalize a model-produced title before it enters metadata or paths.
#[must_use]
pub fn clean_generated_title(raw: &str) -> Option<String> {
    let first = raw
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())?
        .trim_matches(|c| matches!(c, '"' | '\'' | '`' | '#' | '*' | '-' | ':' | ' '));
    let collapsed = first.split_whitespace().collect::<Vec<_>>().join(" ");
    let clipped = collapsed.chars().take(80).collect::<String>();
    let title =
        clipped.trim_matches(|c| matches!(c, '"' | '\'' | '`' | '#' | '*' | '-' | ':' | ' '));
    if title.chars().any(char::is_alphanumeric) {
        Some(title.to_string())
    } else {
        None
    }
}

/// Wrap the LLM's response into a final `notes.md` body. Adds a
/// machine-friendly header (title + timestamp + provider stamp) and
/// the LLM's structured output verbatim.
#[must_use]
pub fn render_notes_body(
    title: Option<&str>,
    started_at: DateTime<Utc>,
    llm_output: &str,
) -> String {
    let mut out = String::new();
    let title_line = title.unwrap_or("Untitled session");
    let _ = writeln!(out, "# {title_line} — notes");
    let _ = writeln!(out, "*Generated {}*\n", started_at.format("%Y-%m-%d %H:%M"));
    out.push_str(llm_output.trim_end());
    out.push('\n');
    out
}

/// Render notes and append deterministic gaps outside the provider response.
#[must_use]
pub fn render_notes_body_with_gaps(
    title: Option<&str>,
    started_at: DateTime<Utc>,
    llm_output: &str,
    gaps: &[ProcessingGap],
) -> String {
    let mut out = render_notes_body(title, started_at, llm_output);
    let rendered_gaps = render_processing_gaps(gaps);
    if !rendered_gaps.is_empty() {
        out.push('\n');
        out.push_str(&rendered_gaps);
    }
    out
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use crate::types::{FrameSource, TranscriptChunk};
    use chrono::TimeZone;
    use pretty_assertions::assert_eq;

    fn dt(year: i32, month: u32, day: u32, hour: u32, minute: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(year, month, day, hour, minute, 0)
            .unwrap()
    }

    #[test]
    fn test_render_transcript_header_includes_title_and_started_at() {
        let header =
            render_transcript_header(Some("Acme discovery"), dt(2026, 4, 29, 14, 30), None);

        assert!(header.starts_with("# Acme discovery\n"));
        assert!(header.contains("2026-04-29 14:30"));
    }

    #[test]
    fn test_render_transcript_header_includes_ended_at_when_provided() {
        let header = render_transcript_header(
            Some("Standup"),
            dt(2026, 4, 29, 9, 0),
            Some(dt(2026, 4, 29, 9, 15)),
        );

        assert!(header.contains("2026-04-29 09:00 — 09:15"));
    }

    #[test]
    fn test_render_transcript_header_uses_untitled_default_when_title_missing() {
        let header = render_transcript_header(None, dt(2026, 4, 29, 14, 30), None);

        assert!(header.contains("Untitled session"));
    }

    #[test]
    fn test_clean_generated_title_strips_markdown_and_quotes() {
        let title = clean_generated_title("## \"Local Validation Setup\"\nextra").unwrap();

        assert_eq!(title, "Local Validation Setup");
    }

    #[test]
    fn test_clean_generated_title_rejects_empty_punctuation() {
        assert_eq!(clean_generated_title(" --- "), None);
    }

    #[test]
    fn test_render_transcript_line_for_me_speaker_starts_with_me_label() {
        let chunk = AttributedChunk {
            chunk: TranscriptChunk {
                text: "Hi.".into(),
                source: FrameSource::Mic,
                start_ms: 3_000,
                duration_ms: 500,
                language: None,
                tokens: Vec::new(),
            },
            speaker: SpeakerLabel::Me,
        };

        let line = render_transcript_line(&chunk);

        assert_eq!(line, "**Me** [00:00:03]: Hi.\n");
    }

    #[test]
    fn test_render_transcript_line_for_them_speaker_starts_with_them_label() {
        let chunk = AttributedChunk {
            chunk: TranscriptChunk {
                text: "Sure.".into(),
                source: FrameSource::System,
                start_ms: 65_000,
                duration_ms: 500,
                language: None,
                tokens: Vec::new(),
            },
            speaker: SpeakerLabel::Them,
        };

        let line = render_transcript_line(&chunk);

        assert_eq!(line, "**Them** [00:01:05]: Sure.\n");
    }

    #[test]
    fn test_render_transcript_line_trims_inner_whitespace_from_text() {
        let chunk = AttributedChunk {
            chunk: TranscriptChunk {
                text: "  hi there  ".into(),
                source: FrameSource::Mic,
                start_ms: 0,
                duration_ms: 1_000,
                language: None,
                tokens: Vec::new(),
            },
            speaker: SpeakerLabel::Me,
        };

        let line = render_transcript_line(&chunk);

        assert!(line.contains("hi there"));
        assert!(!line.contains("  hi"));
    }

    #[test]
    fn test_render_notes_prompt_includes_transcript_between_markers() {
        let transcript = "**Me** [00:00:01]: Hi.\n";
        let ctx = MeetingContext::default();

        let prompt = render_notes_prompt(transcript, &ctx);

        assert!(prompt.contains("--- TRANSCRIPT ---"));
        assert!(prompt.contains(transcript));
        assert!(prompt.contains("--- END TRANSCRIPT ---"));
    }

    #[test]
    fn test_render_notes_prompt_includes_meeting_context_when_populated() {
        let ctx = MeetingContext {
            title: Some("Acme discovery".into()),
            attendees: vec!["Tom".into(), "Alex".into()],
            agenda: Some("Walk proposal".into()),
            ..MeetingContext::default()
        };

        let prompt = render_notes_prompt("transcript", &ctx);

        assert!(prompt.contains("Meeting title: Acme discovery"));
        assert!(prompt.contains("Attendees: Tom, Alex"));
        assert!(prompt.contains("Agenda: Walk proposal"));
    }

    #[test]
    fn test_render_notes_prompt_omits_context_lines_for_default_meeting_context() {
        let prompt = render_notes_prompt("transcript", &MeetingContext::default());

        assert!(!prompt.contains("Meeting title:"));
        assert!(!prompt.contains("Attendees:"));
        assert!(!prompt.contains("Agenda:"));
    }

    #[test]
    fn test_render_notes_body_wraps_llm_output_with_title_and_timestamp() {
        let body = render_notes_body(
            Some("Standup"),
            dt(2026, 4, 29, 9, 0),
            "## TL;DR\nShipped.\n",
        );

        assert!(body.starts_with("# Standup — notes\n"));
        assert!(body.contains("2026-04-29 09:00"));
        assert!(body.contains("## TL;DR\nShipped."));
    }

    #[test]
    fn test_render_notes_body_uses_untitled_default_when_no_title_supplied() {
        let body = render_notes_body(None, dt(2026, 4, 29, 9, 0), "## TL;DR\n\nA\n");

        assert!(body.contains("Untitled session"));
    }
    #[test]
    fn default_template_is_accepted() {
        assert!(validate_template(DEFAULT_TEMPLATE).is_ok());
    }

    #[test]
    fn unknown_template_names_are_rejected() {
        let error = validate_template("executive").unwrap_err();

        assert!(matches!(
            error,
            crate::error::ConfigError::Invalid { key, .. } if key == "notes.template"
        ));
    }
}
