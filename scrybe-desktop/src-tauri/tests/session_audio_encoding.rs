// Copyright 2026 Mathews Tom
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     https://www.apache.org/licenses/LICENSE-2.0

//! The encoder a desktop recording is saved through.
//!
//! `scrybe-core` picks the session encoder at compile time: Ogg Opus when
//! `encoder-opus` is enabled anywhere in the graph, otherwise a test-only
//! encoder that writes raw f32 PCM. The desktop host once shipped without
//! the feature, so every saved `audio.opus` was undecodable PCM. This runs
//! under the host's default features — the shipped build — and asks the
//! same factory the post-capture merge uses for the stereo layout a
//! `mic+system` recording is written in.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use scrybe_core::pipeline::encoder::{default_session_encoder, EncoderConfig};

#[test]
fn shipped_session_encoder_writes_ogg_opus_not_raw_pcm() {
    let config = EncoderConfig {
        sample_rate: 48_000,
        channels: 2,
        ..EncoderConfig::default()
    };
    let mut encoder = default_session_encoder(config).unwrap();

    // One second of stereo audio: enough for the Opus headers and at
    // least one audio page, and enough that raw f32 PCM (384 000 bytes)
    // is unmistakable next to 32 kbps Opus (about 4 000 bytes).
    let pcm = vec![0.25_f32; 48_000 * 2];
    let mut bytes = encoder.push_pcm(&pcm).unwrap();
    bytes.extend(encoder.finish().unwrap());

    assert_eq!(
        bytes.get(..4),
        Some(b"OggS".as_slice()),
        "a saved recording must start with the Ogg capture pattern"
    );
    assert!(
        bytes.windows(8).any(|window| window == b"OpusHead"),
        "the Ogg stream must carry an Opus identification header"
    );
    assert!(
        bytes.len() < 48_000,
        "one second at 32 kbps encoded to {} bytes; raw f32 PCM would be 384000",
        bytes.len()
    );
}
