// Copyright 2026 Mathews Tom
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     https://www.apache.org/licenses/LICENSE-2.0

//! # scrybe-core
//!
//! Core domain types, extension traits, and storage primitives for scrybe.
//! Platform capture adapters, the audio pipeline, and the CLI live in
//! sibling crates and depend on this crate for the contract they implement.
//!
//! See `docs/system-design.md` for the engineering contract; trait
//! shapes here track the Tier-1 / Tier-2 stability tiers documented there.

#![deny(clippy::unwrap_used, clippy::expect_used)]
#![allow(clippy::module_name_repetitions)]

pub mod capture;
pub mod config;
pub mod consent;
pub mod context;
pub mod diarize;
pub mod error;
pub mod hooks;
pub mod notes;
pub mod pipeline;
pub mod providers;
pub mod session;
pub mod storage;
pub mod types;

pub use diarize::{
    requires_neural, select_kind, BinaryChannelDiarizer, Diarizer, DiarizerKind, PyannoteBackend,
    PyannoteOnnxConfig, PyannoteOnnxDiarizer, SpeakerCluster,
};

pub use error::{
    CaptureError, ConfigError, ConsentError, CoreError, HookError, LlmError, PipelineError,
    StorageError, SttError,
};
pub use types::{
    AttributedChunk, AudioChunk, AudioFrame, Capabilities, ConsentAttestation, ConsentMode,
    FrameSource, Language, PermissionModel, SessionId, SpeakerLabel, TranscriptChunk,
};
