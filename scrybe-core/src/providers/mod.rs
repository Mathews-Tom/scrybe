// Copyright 2026 Mathews Tom
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     https://www.apache.org/licenses/LICENSE-2.0

//! Provider trait surfaces: speech-to-text, language model, and the
//! shared retry policy used by every cloud-bound implementation.

pub mod llm;
pub mod retry;
pub mod stt;
pub mod whisper_local;

pub use llm::LlmProvider;
pub use retry::{retry_with_policy, RetryPolicy};
pub use stt::SttProvider;
pub use whisper_local::{WhisperLocalConfig, WhisperLocalProvider};
