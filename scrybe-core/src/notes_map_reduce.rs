// Copyright 2026 Mathews Tom
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
// http://www.apache.org/licenses/LICENSE-2.0
//
//! Capped ordered map-reduce orchestration for canonical transcript segments.

use std::fmt::Write as _;
use std::path::Path;

use tokenizers::Tokenizer;

use crate::config::NotesConfig;
use crate::context::MeetingContext;
use crate::error::{ConfigError, CoreError, LlmError};
use crate::notes::{render_map_prompt, render_reduce_prompt};
use crate::notes_segments::SegmentChunk;
use crate::providers::LlmProvider;

/// Loaded local tokenizer and the request limits it enforces.
pub struct NotesRuntime {
    tokenizer: Tokenizer,
    input_cap_tokens: u32,
    target_tokens: u32,
    overlap_segments: u32,
}

impl NotesRuntime {
    /// Load the caller-provided tokenizer and validate real-LLM request bounds.
    ///
    /// # Errors
    ///
    /// Returns a configuration error for an absent, unreadable, malformed, or
    /// numerically unsafe notes configuration.
    pub fn load(config: &NotesConfig) -> Result<Self, ConfigError> {
        let path = config
            .tokenizer_path
            .as_deref()
            .ok_or_else(|| ConfigError::Missing {
                key: "notes.tokenizer_path".to_string(),
            })?;
        let input_cap_tokens = config
            .input_cap_tokens
            .ok_or_else(|| ConfigError::Missing {
                key: "notes.input_cap_tokens".to_string(),
            })?;
        validate_limits(config.target_tokens, input_cap_tokens)?;
        let tokenizer = load_tokenizer(path)?;
        Ok(Self {
            tokenizer,
            input_cap_tokens,
            target_tokens: config.target_tokens,
            overlap_segments: config.overlap_segments,
        })
    }

    /// Count text with the exact user-provided model tokenizer.
    ///
    /// # Errors
    ///
    /// Returns an invalid-tokenizer configuration error when the loaded model
    /// cannot encode a request string.
    pub fn count_tokens(&self, text: &str) -> Result<u32, ConfigError> {
        let encoding =
            self.tokenizer
                .encode(text, false)
                .map_err(|error| ConfigError::Invalid {
                    key: "notes.tokenizer_path".to_string(),
                    reason: error.to_string(),
                })?;
        u32::try_from(encoding.len()).map_err(|_| ConfigError::Invalid {
            key: "notes.tokenizer_path".to_string(),
            reason: "token count exceeds u32".to_string(),
        })
    }

    /// Whether a fully rendered request fits the configured input cap.
    ///
    /// # Errors
    ///
    /// Propagates tokenizer encoding failure as configuration evidence.
    pub fn prompt_fits(&self, prompt: &str) -> Result<bool, ConfigError> {
        Ok(self.count_tokens(prompt)? <= self.input_cap_tokens)
    }

    /// Preferred token target for packing new transcript segments.
    #[must_use]
    pub const fn target_tokens(&self) -> u32 {
        self.target_tokens
    }

    /// Whole trailing source segments repeated for map context.
    #[must_use]
    pub const fn overlap_segments(&self) -> u32 {
        self.overlap_segments
    }
}

fn load_tokenizer(path: &Path) -> Result<Tokenizer, ConfigError> {
    Tokenizer::from_file(path).map_err(|error| ConfigError::Invalid {
        key: "notes.tokenizer_path".to_string(),
        reason: format!("{}: {error}", path.display()),
    })
}

fn validate_limits(target: u32, cap: u32) -> Result<(), ConfigError> {
    if cap == 0 {
        return Err(invalid(
            "notes.input_cap_tokens",
            "must be greater than zero",
        ));
    }
    if target == 0 {
        return Err(invalid("notes.target_tokens", "must be greater than zero"));
    }
    if target > cap {
        return Err(invalid(
            "notes.target_tokens",
            "must not exceed notes.input_cap_tokens",
        ));
    }
    Ok(())
}

fn invalid(key: &str, reason: &str) -> ConfigError {
    ConfigError::Invalid {
        key: key.to_string(),
        reason: reason.to_string(),
    }
}

/// A map unit that completed successfully.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MapGroup {
    /// First canonical segment ordinal represented by this unit.
    pub start_ordinal: usize,
    /// Last canonical segment ordinal represented by this unit.
    pub end_ordinal: usize,
    /// Factual bullets returned by the map request.
    pub bullets: String,
}

/// Deterministic source coverage information outside model control.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProcessingGap {
    /// First canonical segment ordinal not represented by a successful map.
    pub start_ordinal: usize,
    /// Last canonical segment ordinal not represented by a successful map.
    pub end_ordinal: usize,
    /// Stable sanitized failure classification.
    pub reason: &'static str,
}

/// Successful groups, named map gaps, and the final reduction output.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MapReduceOutput {
    /// Successful map outputs in source order.
    pub groups: Vec<MapGroup>,
    /// Named map failures in source order.
    pub gaps: Vec<ProcessingGap>,
    /// Structured notes from the final reduction request.
    pub reduced_notes: String,
}

/// Map each complete chunk, retaining map failures as deterministic gaps, then
/// reduce the successful groups and gaps.
///
/// The checker sees the complete rendered map request before provider dispatch.
/// A request above the explicit cap becomes an `input-cap-exceeded` gap; it is
/// never sent, split, or silently omitted.
///
/// # Errors
///
/// Returns configuration/tokenizer failure or a final reduction provider error.
/// Individual map provider failures remain visible in the returned gap list.
pub async fn map_reduce<L, F>(
    provider: &L,
    chunks: &[SegmentChunk<'_>],
    context: &MeetingContext,
    mut prompt_fits: F,
) -> Result<MapReduceOutput, CoreError>
where
    L: LlmProvider,
    F: FnMut(&str) -> Result<bool, ConfigError>,
{
    let mut groups = Vec::with_capacity(chunks.len());
    let mut gaps = Vec::new();
    for chunk in chunks {
        let (Some(first), Some(last)) = (chunk.segments.first(), chunk.segments.last()) else {
            continue;
        };
        let prompt = render_map_prompt(&chunk.overlap, &chunk.segments);
        if !prompt_fits(&prompt).map_err(CoreError::Config)? {
            gaps.push(ProcessingGap {
                start_ordinal: first.ordinal,
                end_ordinal: last.ordinal,
                reason: "input-cap-exceeded",
            });
            continue;
        }
        match provider.complete(&prompt).await {
            Ok(bullets) => groups.push(MapGroup {
                start_ordinal: first.ordinal,
                end_ordinal: last.ordinal,
                bullets,
            }),
            Err(error) => gaps.push(ProcessingGap {
                start_ordinal: first.ordinal,
                end_ordinal: last.ordinal,
                reason: error_class(&error),
            }),
        }
    }
    let (reduce_prompt, groups) = capped_reduce_prompt(groups, &gaps, context, &mut prompt_fits)
        .map_err(CoreError::Config)?;
    let reduced_notes = provider.complete(&reduce_prompt).await?;
    Ok(MapReduceOutput {
        groups,
        gaps,
        reduced_notes,
    })
}

fn capped_reduce_prompt<F>(
    groups: Vec<MapGroup>,
    gaps: &[ProcessingGap],
    context: &MeetingContext,
    prompt_fits: &mut F,
) -> Result<(String, Vec<MapGroup>), ConfigError>
where
    F: FnMut(&str) -> Result<bool, ConfigError>,
{
    let prompt = render_reduce_prompt(&groups, gaps, context);
    if prompt_fits(&prompt)? {
        return Ok((prompt, groups));
    }
    if groups.len() < 3 {
        return Err(invalid(
            "notes.input_cap_tokens",
            "cannot fit the reduction prompt without dropping required content",
        ));
    }
    let compact = vec![
        groups[0].clone(),
        MapGroup {
            start_ordinal: 0,
            end_ordinal: 0,
            bullets: "[OMITTED MIDDLE MAP GROUPS DUE TO INPUT CAP]".to_string(),
        },
        groups[groups.len() - 1].clone(),
    ];
    let prompt = render_reduce_prompt(&compact, gaps, context);
    if !prompt_fits(&prompt)? {
        return Err(invalid(
            "notes.input_cap_tokens",
            "cannot fit required reduction scaffolding, gaps, and omitted-middle marker",
        ));
    }
    Ok((prompt, compact))
}

/// Render named gaps outside the model response for durable visibility.
#[must_use]
pub fn render_processing_gaps(gaps: &[ProcessingGap]) -> String {
    if gaps.is_empty() {
        return String::new();
    }
    let mut out = String::from("## Processing gaps\n");
    for gap in gaps {
        if gap.start_ordinal == gap.end_ordinal {
            let _ = writeln!(
                out,
                "- Transcript segment {} ({})",
                gap.start_ordinal, gap.reason
            );
        } else {
            let _ = writeln!(
                out,
                "- Transcript segments {}–{} ({})",
                gap.start_ordinal, gap.end_ordinal, gap.reason
            );
        }
    }
    out
}

const fn error_class(error: &LlmError) -> &'static str {
    match error {
        LlmError::ProviderStatus { .. } => "provider-status",
        LlmError::RetriesExhausted { .. } => "retries-exhausted",
        LlmError::PromptRendering(_) => "prompt-rendering",
        LlmError::Transport(_) => "transport",
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::notes_segments::TranscriptSegment;
    use async_trait::async_trait;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct ScriptedProvider {
        calls: AtomicUsize,
    }

    #[async_trait]
    impl LlmProvider for ScriptedProvider {
        async fn complete(&self, _prompt: &str) -> Result<String, LlmError> {
            match self.calls.fetch_add(1, Ordering::SeqCst) {
                0 => Err(LlmError::ProviderStatus { status: 503 }),
                1 => Ok("- second fact".to_string()),
                2 => Ok("## TL;DR\nSecond fact.".to_string()),
                _ => panic!("unexpected provider request"),
            }
        }

        fn name(&self) -> &'static str {
            "scripted"
        }
    }

    struct ReduceOnlyProvider;

    #[async_trait]
    impl LlmProvider for ReduceOnlyProvider {
        async fn complete(&self, _prompt: &str) -> Result<String, LlmError> {
            Ok("## TL;DR\nNo source was mapped.".to_string())
        }

        fn name(&self) -> &'static str {
            "reduce-only"
        }
    }

    fn segment(ordinal: usize, text: &str) -> TranscriptSegment {
        TranscriptSegment {
            ordinal,
            speaker: "Me".to_string(),
            start_ms: 0,
            text: text.to_string(),
        }
    }

    #[tokio::test]
    async fn map_failure_is_visible_and_later_chunk_still_reduces() {
        let first = segment(1, "first");
        let second = segment(2, "second");
        let chunks = [
            SegmentChunk {
                segments: vec![&first],
                overlap: vec![],
            },
            SegmentChunk {
                segments: vec![&second],
                overlap: vec![],
            },
        ];
        let provider = ScriptedProvider {
            calls: AtomicUsize::new(0),
        };

        let output = map_reduce(&provider, &chunks, &MeetingContext::default(), |_| Ok(true))
            .await
            .unwrap();

        assert_eq!(output.groups.len(), 1);
        assert_eq!(output.groups[0].start_ordinal, 2);
        assert_eq!(
            output.gaps,
            vec![ProcessingGap {
                start_ordinal: 1,
                end_ordinal: 1,
                reason: "provider-status",
            }]
        );
        assert!(render_processing_gaps(&output.gaps).contains("segment 1 (provider-status)"));
        assert_eq!(provider.calls.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn over_cap_map_prompt_is_never_dispatched_and_becomes_gap() {
        let only = segment(1, "too large");
        let chunks = [SegmentChunk {
            segments: vec![&only],
            overlap: vec![],
        }];

        let output = map_reduce(
            &ReduceOnlyProvider,
            &chunks,
            &MeetingContext::default(),
            |prompt| Ok(!prompt.contains("TRANSCRIPT SEGMENTS (new coverage)")),
        )
        .await
        .unwrap();

        assert!(output.groups.is_empty());
        assert_eq!(output.gaps[0].reason, "input-cap-exceeded");
        assert!(render_processing_gaps(&output.gaps).contains("input-cap-exceeded"));
    }
}
