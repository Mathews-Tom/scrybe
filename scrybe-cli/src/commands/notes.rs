// Copyright 2026 Mathews Tom
// Licensed under the Apache License, Version 2.0

//! Regenerate `notes.md` from a session's durable transcript.

use std::path::PathBuf;

#[cfg(feature = "llm-openai-compat")]
use anyhow::Context;
use anyhow::Result;
#[cfg(any(test, feature = "llm-openai-compat"))]
use chrono::{DateTime, NaiveDateTime, Utc};
use clap::Args as ClapArgs;
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
use scrybe_core::storage::atomic_replace;

#[cfg(feature = "llm-openai-compat")]
use crate::runtime::{expand_root, load_or_default_config, resolve_session_folder};

#[derive(ClapArgs, Debug)]
pub struct Args {
    /// Session folder, folder name, or unambiguous session-ID prefix.
    pub id_or_folder: String,

    /// Override the storage root from config.
    #[arg(long)]
    pub root: Option<PathBuf>,
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
        let cfg = load_or_default_config()?;
        let root = args
            .root
            .as_deref()
            .map_or_else(|| expand_root(&cfg.storage.root), expand_root);
        let folder = resolve_session_folder(&root, &args.id_or_folder)
            .with_context(|| format!("resolving session {}", args.id_or_folder))?;
        let transcript_path = folder.join("transcript.md");
        let transcript = std::fs::read_to_string(&transcript_path)
            .with_context(|| format!("reading {}", transcript_path.display()))?;
        let title = transcript_title(&transcript);
        let started_at = transcript_started_at(&transcript).unwrap_or_else(Utc::now);
        let context = MeetingContext {
            title: title.clone(),
            ..MeetingContext::default()
        };
        let runtime = NotesRuntime::load(&cfg.notes).context("loading notes tokenizer")?;
        let segments =
            parse_canonical_transcript(&transcript).context("parsing canonical transcript")?;
        let token_counts: Vec<u32> = segments
            .iter()
            .map(|segment| runtime.count_tokens(&segment.text))
            .collect::<Result<_, _>>()
            .context("counting transcript tokens")?;
        let chunks = pack_segments(
            &segments,
            runtime.target_tokens(),
            runtime.overlap_segments(),
            |segment| token_counts[segment.ordinal - 1],
        );
        let provider = OpenAiCompatLlmProvider::from_config(&cfg.llm)
            .context("initializing configured notes provider")?;
        eprintln!(
            "scrybe: regenerating notes from {} request group{}",
            chunks.len(),
            if chunks.len() == 1 { "" } else { "s" }
        );
        let output = map_reduce(&provider, &chunks, &context, |prompt| {
            runtime.prompt_fits(prompt)
        })
        .await
        .context("generating notes")?;
        let body = notes::render_notes_body_with_gaps(
            title.as_deref(),
            started_at,
            &output.reduced_notes,
            &output.gaps,
        );
        let notes_path = folder.join("notes.md");
        atomic_replace(&notes_path, body.as_bytes())
            .with_context(|| format!("writing {}", notes_path.display()))?;
        println!("scrybe notes: wrote {}", notes_path.display());
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
