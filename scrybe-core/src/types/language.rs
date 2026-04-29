// Copyright 2026 Mathews Tom
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     https://www.apache.org/licenses/LICENSE-2.0

//! BCP-47 language tag wrapper used in `MeetingContext` and `meta.toml`.
//!
//! We deliberately do not validate the full BCP-47 grammar at construction
//! time — that would require pulling in `language-tags` and committing to
//! the full IANA registry. Whisper accepts ISO-639-1/-2/-3 codes; the
//! pipeline forwards whatever the user supplies. Validation is at the
//! provider boundary, not here.

use serde::{Deserialize, Serialize};

/// BCP-47 language tag. Stored as a lowercased string; prefer the constructors
/// to enforce the lowercase invariant.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Language(String);

impl Language {
    /// Construct from any BCP-47-shaped string. The tag is normalized to
    /// lowercase for stable equality and stable serialization.
    pub fn new(tag: impl Into<String>) -> Self {
        Self(tag.into().to_ascii_lowercase())
    }

    /// Borrow the lowercased tag.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// "Auto-detect" sentinel used by Whisper providers and config loaders.
    #[must_use]
    pub fn auto() -> Self {
        Self::new("auto")
    }

    /// True when the tag is the "auto" sentinel.
    #[must_use]
    pub fn is_auto(&self) -> bool {
        self.0 == "auto"
    }
}

impl std::fmt::Display for Language {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn test_language_new_lowercases_input_tag() {
        let lang = Language::new("EN-US");

        assert_eq!(lang.as_str(), "en-us");
    }

    #[test]
    fn test_language_auto_returns_sentinel_and_is_auto_true() {
        let lang = Language::auto();

        assert!(lang.is_auto());
        assert_eq!(lang.as_str(), "auto");
    }

    #[test]
    fn test_language_is_auto_false_for_real_tag() {
        assert!(!Language::new("ja").is_auto());
    }

    #[test]
    fn test_language_round_trips_through_json_as_transparent_string() {
        let lang = Language::new("pt-br");

        let encoded = serde_json::to_string(&lang).unwrap();

        assert_eq!(encoded, "\"pt-br\"");
        let decoded: Language = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded, lang);
    }

    #[test]
    fn test_language_display_writes_lowercased_tag() {
        let rendered = format!("{}", Language::new("Hi-IN"));

        assert_eq!(rendered, "hi-in");
    }
}
