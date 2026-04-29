// Copyright 2026 Mathews Tom
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     https://www.apache.org/licenses/LICENSE-2.0

//! Cross-crate domain types: audio frames, sessions, transcripts, consent.

mod audio;
mod consent;
mod language;
mod session;
mod transcript;

pub use audio::{AudioFrame, Capabilities, FrameSource, PermissionModel};
pub use consent::{ConsentAttestation, ConsentMode};
pub use language::Language;
pub use session::SessionId;
pub use transcript::{AttributedChunk, AudioChunk, SpeakerLabel, TranscriptChunk};
