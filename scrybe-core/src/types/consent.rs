// Copyright 2026 Mathews Tom
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     https://www.apache.org/licenses/LICENSE-2.0

//! Consent attestation types used by the courtesy-notification step.
//!
//! `ConsentAttestation` is a Tier-1 schema field on `meta.toml`
//! (`docs/system-design.md` §12) — once written to disk, scrybe must
//! be able to parse a v0.1 attestation through every later version.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Three configurable courtesy-notification modes (`docs/system-design.md` §5).
/// `Quick` is the floor: the consent step cannot be disabled.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ConsentMode {
    Quick,
    Notify,
    Announce,
}

impl ConsentMode {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Quick => "quick",
            Self::Notify => "notify",
            Self::Announce => "announce",
        }
    }
}

impl std::fmt::Display for ConsentMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Recorded by the consent step before capture starts. Persisted into
/// `meta.toml` so the audit trail survives even if the application is
/// uninstalled.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ConsentAttestation {
    pub mode: ConsentMode,
    pub attested_at: DateTime<Utc>,
    pub by_user: String,
    /// Whether a chat message was actually injected (only valid for
    /// `Notify` and `Announce` modes).
    #[serde(default)]
    pub chat_message_sent: bool,
    /// Detected meeting platform when known (e.g. `"zoom"`, `"meet"`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chat_message_target: Option<String>,
    /// Whether the spoken disclosure played (only meaningful for `Announce`).
    #[serde(default)]
    pub tts_announce_played: bool,
}

impl ConsentAttestation {
    /// Construct a minimal attestation. Use the public fields to fill in
    /// transport details after the courtesy step actually fires.
    #[must_use]
    pub fn new(mode: ConsentMode, by_user: impl Into<String>) -> Self {
        Self {
            mode,
            attested_at: Utc::now(),
            by_user: by_user.into(),
            chat_message_sent: false,
            chat_message_target: None,
            tts_announce_played: false,
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn test_consent_mode_as_str_matches_serde_lowercase_tag() {
        for (mode, expected) in [
            (ConsentMode::Quick, "quick"),
            (ConsentMode::Notify, "notify"),
            (ConsentMode::Announce, "announce"),
        ] {
            assert_eq!(mode.as_str(), expected);
            assert_eq!(
                serde_json::to_string(&mode).unwrap(),
                format!("\"{expected}\"")
            );
        }
    }

    #[test]
    fn test_consent_attestation_new_defaults_optional_fields_to_unset() {
        let att = ConsentAttestation::new(ConsentMode::Quick, "tom");

        assert_eq!(att.mode, ConsentMode::Quick);
        assert_eq!(att.by_user, "tom");
        assert!(!att.chat_message_sent);
        assert!(att.chat_message_target.is_none());
        assert!(!att.tts_announce_played);
    }

    #[test]
    fn test_consent_attestation_round_trips_through_toml() {
        let original = ConsentAttestation {
            mode: ConsentMode::Notify,
            attested_at: DateTime::parse_from_rfc3339("2026-04-29T14:29:58Z")
                .unwrap()
                .with_timezone(&Utc),
            by_user: "tom".into(),
            chat_message_sent: true,
            chat_message_target: Some("zoom".into()),
            tts_announce_played: false,
        };

        let encoded = toml::to_string(&original).unwrap();
        let decoded: ConsentAttestation = toml::from_str(&encoded).unwrap();

        assert_eq!(decoded, original);
    }

    #[test]
    fn test_consent_attestation_skips_none_target_in_toml_output() {
        let att = ConsentAttestation::new(ConsentMode::Quick, "tom");

        let encoded = toml::to_string(&att).unwrap();

        assert!(!encoded.contains("chat_message_target"));
    }
}
