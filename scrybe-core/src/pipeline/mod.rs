// Copyright 2026 Mathews Tom
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     https://www.apache.org/licenses/LICENSE-2.0

//! Pipeline stages between capture and storage: VAD, chunker, resample,
//! encoder. Each stage is independently testable; the orchestrator in
//! `crate::session` wires them together.
//!
//! See `docs/system-design.md` §5 for the canonical pipeline diagram and
//! the rationale behind the 30 s window / 5 s silence-after-5 s-speech
//! chunking rule.

pub mod chunker;
pub mod encoder;
pub mod resample;
pub mod vad;

pub use chunker::{ChunkSink, Chunker, ChunkerConfig, EmittedChunk};
pub use encoder::{Encoder, EncoderConfig, NullEncoder};
pub use resample::{resample_linear, ResampleError};
pub use vad::{EnergyVad, Vad, VadDecision};
