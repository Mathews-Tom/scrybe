#![cfg(feature = "openai-compat")]
#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use std::env;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};

use scrybe_core::config::Config;
use scrybe_core::context::MeetingContext;
use scrybe_core::notes_map_reduce::{map_reduce, NotesRuntime};
use scrybe_core::notes_segments::{pack_segments, parse_canonical_transcript};
use scrybe_core::providers::{LlmProvider, OpenAiCompatLlmProvider};

#[tokio::test]
#[ignore = "requires SCRYBE_M7_CONFIG and a reachable local OpenAI-compatible endpoint"]
async fn three_hour_fixture_uses_only_capped_requests() {
    let config_path = env::var("SCRYBE_M7_CONFIG")
        .expect("set SCRYBE_M7_CONFIG to the local Ollama configuration path");
    let config = Config::load(Path::new(&config_path)).expect("live evidence config must load");
    let runtime = NotesRuntime::load(&config.notes).expect("notes runtime must preflight");
    let provider = OpenAiCompatLlmProvider::from_config(&config.llm).expect("provider must build");
    let transcript = include_str!("fixtures/three-hour-canonical-transcript.md");
    let segments = parse_canonical_transcript(transcript).expect("fixture must parse");
    let chunks = pack_segments(
        &segments,
        runtime.target_tokens(),
        runtime.overlap_segments(),
        |segment| {
            runtime
                .count_tokens(&segment.text)
                .expect("fixture token count must fit u32")
        },
    );
    let request_count = AtomicUsize::new(0);

    let output = map_reduce(&provider, &chunks, &MeetingContext::default(), |prompt| {
        request_count.fetch_add(1, Ordering::Relaxed);
        runtime.prompt_fits(prompt)
    })
    .await
    .expect("capped local Ollama map-reduce must succeed");

    let covered: Vec<usize> = output
        .groups
        .iter()
        .flat_map(|group| group.start_ordinal..=group.end_ordinal)
        .chain(
            output
                .gaps
                .iter()
                .flat_map(|gap| gap.start_ordinal..=gap.end_ordinal),
        )
        .collect();
    assert_eq!(covered, (1..=segments.len()).collect::<Vec<_>>());
    assert!(request_count.load(Ordering::Relaxed) >= 2);
    assert!(!output.reduced_notes.trim().is_empty());
    assert!(LlmProvider::name(&provider).contains("qwen2.5:7b-instruct"));
}
