// Copyright 2026 Mathews Tom
// Licensed under the Apache License, Version 2.0

//! Regenerate `notes.md` from a session's durable transcript.
//!
//! Session resolution, eligibility, the durable replacement, and cache
//! invalidation belong to the shared application service. What stays
//! here is the configured provider: which model produces the text, and
//! how the transcript is segmented and capped for it.

use std::path::PathBuf;

#[cfg(feature = "llm-openai-compat")]
use anyhow::Context;
use anyhow::Result;
#[cfg(any(test, feature = "llm-openai-compat"))]
use chrono::{DateTime, NaiveDateTime, Utc};
use clap::Args as ClapArgs;
#[cfg(feature = "llm-openai-compat")]
use scrybe_application::sessions::{NotesGenerationRequest, NotesGenerator};
#[cfg(feature = "llm-openai-compat")]
use scrybe_application::SessionRef;
#[cfg(feature = "llm-openai-compat")]
use scrybe_core::config::Config;
#[cfg(feature = "llm-openai-compat")]
use scrybe_core::context::MeetingContext;
#[cfg(feature = "llm-openai-compat")]
use scrybe_core::notes;
#[cfg(feature = "llm-openai-compat")]
use scrybe_core::notes_map_reduce::{map_reduce, NotesRuntime};
#[cfg(feature = "llm-openai-compat")]
use scrybe_core::notes_segments::{pack_segments, parse_canonical_transcript};
#[cfg(feature = "llm-openai-compat")]
use scrybe_core::providers::openai_compat_llm::OpenAiCompatLlmProvider;

#[cfg(feature = "llm-openai-compat")]
use crate::runtime::{application, load_or_default_config};

#[derive(ClapArgs, Debug)]
pub struct Args {
    /// Session folder name, or an unambiguous session-ID prefix. Paths
    /// are not accepted: every session resolves beneath the configured
    /// storage root.
    pub id_or_folder: String,

    /// Override the storage root from config.
    #[arg(long)]
    pub root: Option<PathBuf>,
}

/// The configured notes provider, as the repository sees it.
///
/// Segmentation, request capping, and map-reduce orchestration are
/// presentation-adjacent policy the CLI already owned and continues to
/// own; the repository only asks for markdown.
#[cfg(feature = "llm-openai-compat")]
struct ConfiguredNotes {
    config: Config,
}

#[cfg(feature = "llm-openai-compat")]
#[async_trait::async_trait]
impl NotesGenerator for ConfiguredNotes {
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
}

/// Regenerate notes from the canonical durable transcript.
///
/// # Errors
///
/// Returns configuration, transcript-parse, provider, or storage errors.
#[allow(clippy::unused_async)]
pub async fn run(args: Args) -> Result<()> {
    #[cfg(feature = "llm-openai-compat")]
    {
        let app = application(args.root.as_deref())?;
        let repository = app.sessions();
        let id = SessionRef::parse(&args.id_or_folder)
            .map_err(scrybe_application::ApplicationError::from)
            .with_context(|| format!("resolving session {}", args.id_or_folder))?;
        let generator = ConfiguredNotes {
            config: load_or_default_config()?,
        };
        let result = repository
            .regenerate_notes(&id, &generator)
            .await
            .with_context(|| format!("regenerating notes for session {}", args.id_or_folder))?;
        let path = repository.root().resolve(&result.id).join("notes.md");
        println!("scrybe notes: wrote {}", path.display());
        Ok(())
    }

    #[cfg(not(feature = "llm-openai-compat"))]
    {
        let _ = args;
        anyhow::bail!("scrybe notes requires a build with the `llm-openai-compat` feature");
    }
}

#[cfg(any(test, feature = "llm-openai-compat"))]
fn transcript_title(transcript: &str) -> Option<String> {
    transcript
        .lines()
        .next()
        .and_then(|line| line.strip_prefix("# "))
        .map(str::trim)
        .filter(|title| !title.is_empty() && *title != "Untitled session")
        .map(str::to_string)
}

#[cfg(any(test, feature = "llm-openai-compat"))]
fn transcript_started_at(transcript: &str) -> Option<DateTime<Utc>> {
    let value = transcript.lines().nth(1)?.trim_matches('*');
    let started = value.split_once(" — ").map_or(value, |(start, _)| start);
    NaiveDateTime::parse_from_str(started, "%Y-%m-%d %H:%M")
        .ok()
        .map(|datetime| datetime.and_utc())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_title_and_start_time_from_transcript_header() {
        let transcript = "# Weekly sync\n*2026-09-11 21:50*\n\n";
        assert_eq!(transcript_title(transcript).as_deref(), Some("Weekly sync"));
        assert_eq!(
            transcript_started_at(transcript).map(|value| value.to_rfc3339()),
            Some("2026-09-11T21:50:00+00:00".to_string())
        );
    }
}
