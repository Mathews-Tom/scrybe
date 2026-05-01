// Copyright 2026 Mathews Tom
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     https://www.apache.org/licenses/LICENSE-2.0

//! `ContextProvider` and `MeetingContext` (`docs/system-design.md` §4.2).
//!
//! `MeetingContext` is **Tier 1** stability: the field set is frozen at
//! v1.0 because it is serialized into the LLM prompt template and into
//! `meta.toml`. Adding a field requires a major version bump.

#[cfg(feature = "context-ics")]
pub mod ics;

#[cfg(feature = "context-ics")]
pub use ics::{IcsFileConfig, IcsFileProvider, MatchWindow};

use std::collections::BTreeMap;
use std::path::PathBuf;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::error::CoreError;
use crate::types::Language;

/// Pre-call context populated from a calendar entry, CLI flags, or any
/// future provider. Drives the LLM prompt and the `notes.md` template.
#[derive(Clone, Default, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct MeetingContext {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attendees: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agenda: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub prior_notes: Vec<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub language: Option<Language>,
    /// Free-form provider-specific extras. Use `extra` to attach calendar
    /// metadata that would otherwise pollute the structured fields. Future
    /// providers add keys here without touching the trait.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub extra: BTreeMap<String, String>,
}

/// Source of pre-call context.
///
/// Implementations are concrete adapters (`CliFlagProvider`,
/// `IcsFileProvider`, `EventKitProvider`, …) wired in via cargo
/// features. The trait is `async` so providers can perform I/O
/// (read a calendar file, query a system framework).
#[async_trait]
pub trait ContextProvider: Send + Sync {
    /// Look up context for a session that started at `started_at`. The
    /// timestamp is in UTC; providers translate to local time as needed.
    ///
    /// # Errors
    ///
    /// Implementations return `CoreError::Config` for misconfiguration
    /// (e.g. a missing `.ics` path) and `CoreError::Storage` for I/O
    /// failures while reading the source.
    async fn context_for(&self, started_at: DateTime<Utc>) -> Result<MeetingContext, CoreError>;
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    struct StubProvider {
        ctx: MeetingContext,
    }

    #[async_trait]
    impl ContextProvider for StubProvider {
        async fn context_for(
            &self,
            _started_at: DateTime<Utc>,
        ) -> Result<MeetingContext, CoreError> {
            Ok(self.ctx.clone())
        }
    }

    #[tokio::test]
    async fn test_context_provider_returns_supplied_context() {
        let stub = StubProvider {
            ctx: MeetingContext {
                title: Some("Standup".into()),
                attendees: vec!["Alex".into(), "Tom".into()],
                ..MeetingContext::default()
            },
        };

        let result = stub.context_for(Utc::now()).await.unwrap();

        assert_eq!(result.title.as_deref(), Some("Standup"));
        assert_eq!(result.attendees.len(), 2);
    }

    #[test]
    fn test_meeting_context_default_serializes_to_empty_table() {
        let ctx = MeetingContext::default();

        let toml = toml::to_string(&ctx).unwrap();

        assert_eq!(toml, "");
    }

    #[test]
    fn test_meeting_context_round_trips_through_toml_with_attendees() {
        let original = MeetingContext {
            title: Some("Acme discovery".into()),
            attendees: vec!["Tom".into(), "Alex".into()],
            agenda: Some("Walk through proposal".into()),
            prior_notes: vec![PathBuf::from("/tmp/prev.md")],
            language: Some(Language::new("en")),
            extra: {
                let mut m = BTreeMap::new();
                m.insert("calendar".into(), "google".into());
                m
            },
        };

        let encoded = toml::to_string(&original).unwrap();
        let decoded: MeetingContext = toml::from_str(&encoded).unwrap();

        assert_eq!(decoded, original);
    }

    #[test]
    fn test_meeting_context_omits_empty_attendees_in_toml_output() {
        let ctx = MeetingContext {
            title: Some("Solo".into()),
            ..MeetingContext::default()
        };

        let encoded = toml::to_string(&ctx).unwrap();

        assert!(!encoded.contains("attendees"));
        assert!(encoded.contains("title"));
    }
}
