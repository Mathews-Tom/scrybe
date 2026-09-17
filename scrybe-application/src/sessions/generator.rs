// Copyright 2026 Mathews Tom
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     https://www.apache.org/licenses/LICENSE-2.0

//! The configured notes provider, as the repository sees it.
//!
//! [`SessionRepository::regenerate_notes`] owns resolution, eligibility,
//! the atomic write, and cache invalidation, and asks a caller-supplied
//! generator only for markdown. This is that generator, built from the
//! configuration the rest of the service layer already reads.
//!
//! It lives beside the trait rather than inside a frontend because both
//! frontends that can regenerate notes need the same answer, and two
//! copies of segmentation, capping, and map-reduce policy are two things
//! that can disagree about what a session's notes are.
//!
//! [`SessionRepository::regenerate_notes`]: crate::sessions::SessionRepository::regenerate_notes

#[cfg(any(test, feature = "notes-generation"))]
use chrono::{DateTime, NaiveDateTime, Utc};
use scrybe_core::config::Config;
#[cfg(feature = "notes-generation")]
use scrybe_core::context::MeetingContext;
#[cfg(feature = "notes-generation")]
use scrybe_core::notes;
#[cfg(feature = "notes-generation")]
use scrybe_core::notes_map_reduce::{map_reduce, NotesRuntime};
#[cfg(feature = "notes-generation")]
use scrybe_core::notes_segments::{pack_segments, parse_canonical_transcript};
#[cfg(feature = "notes-generation")]
use scrybe_core::providers::openai_compat_llm::OpenAiCompatLlmProvider;

use crate::sessions::repository::{NotesGenerationRequest, NotesGenerator};

/// Produces notes through the provider the configuration names.
pub struct ConfiguredNotesGenerator {
    config: Config,
}

impl ConfiguredNotesGenerator {
    /// A generator over `config`.
    #[must_use]
    pub const fn new(config: Config) -> Self {
        Self { config }
    }
}

#[async_trait::async_trait]
impl NotesGenerator for ConfiguredNotesGenerator {
    /// Renders notes markdown for `request`.
    ///
    /// # Errors
    ///
    /// A configuration the notes runtime rejects, a transcript that is
    /// not canonical, a provider failure, or — in a build compiled
    /// without `notes-generation` — a refusal saying so. A build with no
    /// provider linked cannot produce notes, and reporting that is the
    /// only honest thing it can do.
    #[cfg(feature = "notes-generation")]
    async fn generate(
        &self,
        request: &NotesGenerationRequest<'_>,
    ) -> std::result::Result<String, Box<dyn std::error::Error + Send + Sync>> {
        let title = transcript_title(request.transcript);
        let started_at = request
            .started_at
            .or_else(|| transcript_started_at(request.transcript))
            .unwrap_or_else(Utc::now);
        let context = MeetingContext {
            title: title.clone(),
            ..MeetingContext::default()
        };
        let runtime = NotesRuntime::load(&self.config.notes)?;
        let segments = parse_canonical_transcript(request.transcript)?;
        let token_counts: Vec<u32> = segments
            .iter()
            .map(|segment| runtime.count_tokens(&segment.text))
            .collect::<std::result::Result<_, _>>()?;
        let chunks = pack_segments(
            &segments,
            runtime.target_tokens(),
            runtime.overlap_segments(),
            |segment| token_counts[segment.ordinal - 1],
        );
        let provider = OpenAiCompatLlmProvider::from_config(&self.config.llm)?;
        eprintln!(
            "scrybe: regenerating notes from {} request group{}",
            chunks.len(),
            if chunks.len() == 1 { "" } else { "s" }
        );
        let output = map_reduce(&provider, &chunks, &context, |prompt| {
            runtime.prompt_fits(prompt)
        })
        .await?;
        Ok(notes::render_notes_body_with_gaps(
            title.as_deref(),
            started_at,
            &output.reduced_notes,
            &output.gaps,
        ))
    }

    #[cfg(not(feature = "notes-generation"))]
    async fn generate(
        &self,
        _request: &NotesGenerationRequest<'_>,
    ) -> std::result::Result<String, Box<dyn std::error::Error + Send + Sync>> {
        let _ = &self.config;
        Err("this build carries no notes provider".into())
    }
}

/// The session title the transcript's own header carries.
///
/// The placeholder a transcript written without a title carries is not
/// a title, and passing it through would put the word `Untitled` into
/// the notes as though someone had chosen it.
#[cfg(any(test, feature = "notes-generation"))]
fn transcript_title(transcript: &str) -> Option<String> {
    transcript
        .lines()
        .next()
        .and_then(|line| line.strip_prefix("# "))
        .map(str::trim)
        .filter(|title| !title.is_empty() && *title != "Untitled session")
        .map(str::to_string)
}

/// The start time the transcript's own header carries, for a session
/// whose `meta.toml` did not survive.
#[cfg(any(test, feature = "notes-generation"))]
fn transcript_started_at(transcript: &str) -> Option<DateTime<Utc>> {
    let value = transcript.lines().nth(1)?.trim_matches('*');
    let started = value.split_once(" — ").map_or(value, |(start, _)| start);
    NaiveDateTime::parse_from_str(started, "%Y-%m-%d %H:%M")
        .ok()
        .map(|datetime| datetime.and_utc())
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn test_the_transcript_header_supplies_the_title_and_start_time() {
        let transcript = "# Weekly sync\n*2026-09-11 21:50*\n\n";

        assert_eq!(transcript_title(transcript).as_deref(), Some("Weekly sync"));
        assert_eq!(
            transcript_started_at(transcript).map(|value| value.to_rfc3339()),
            Some("2026-09-11T21:50:00+00:00".to_string())
        );
    }

    #[test]
    fn test_the_placeholder_title_is_not_mistaken_for_one_somebody_chose() {
        assert_eq!(transcript_title("# Untitled session\n"), None);
    }
}
