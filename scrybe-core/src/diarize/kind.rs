// Copyright 2026 Mathews Tom
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     https://www.apache.org/licenses/LICENSE-2.0

//! `DiarizerKind` — the small enum that names every diarizer scrybe
//! ships, plus the auto-routing helper that turns a configured choice
//! plus runtime evidence into a concrete kind.
//!
//! Kept in `scrybe-core` rather than splitting across the diarizer impls
//! so the CLI and `scrybe-android` can take a routing decision without
//! depending on the `diarize-pyannote` feature gate.

use crate::context::MeetingContext;
use crate::types::Capabilities;

use super::requires_neural;

/// Stable string used in `[diarizer] kind = "..."`.
pub const DIARIZER_KIND_BINARY_CHANNEL: &str = "binary-channel";
/// Stable string used in `[diarizer] kind = "..."`.
pub const DIARIZER_KIND_PYANNOTE_ONNX: &str = "pyannote-onnx";

/// Every diarizer the scrybe pipeline can route through. Tier-2 stable —
/// adding a variant is a minor-version change because the on-disk
/// `meta.toml` serializes the chosen kind verbatim.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum DiarizerKind {
    /// `BinaryChannelDiarizer` — the v0.1 default; correct for 1:1 calls.
    #[default]
    BinaryChannel,
    /// `PyannoteOnnxDiarizer` — neural fallback for multi-party / in-room
    /// meetings. Requires the `diarize-pyannote` cargo feature for the
    /// live ONNX runtime; the trait surface and the routing are
    /// available without the feature.
    PyannoteOnnx,
}

impl DiarizerKind {
    /// Parse a config string. Returns `None` for unknown values so the
    /// config loader can surface a useful error rather than silently
    /// defaulting.
    #[must_use]
    pub fn from_config_str(s: &str) -> Option<Self> {
        match s {
            DIARIZER_KIND_BINARY_CHANNEL => Some(Self::BinaryChannel),
            DIARIZER_KIND_PYANNOTE_ONNX => Some(Self::PyannoteOnnx),
            _ => None,
        }
    }

    /// Stable string surfaced in `[diarizer] kind = "..."` and in
    /// `meta.toml`'s `[providers]` table.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::BinaryChannel => DIARIZER_KIND_BINARY_CHANNEL,
            Self::PyannoteOnnx => DIARIZER_KIND_PYANNOTE_ONNX,
        }
    }
}

impl std::fmt::Display for DiarizerKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Resolve the configured diarizer choice against runtime evidence.
///
/// `configured` is what the user wrote in `[diarizer] kind`. `None`
/// means "the user did not set the field; pick for them" and routes
/// through the auto-rule documented in `system-design.md` §4.4: a call
/// that is multi-party (≥3 attendees) or single-channel (no system
/// audio) goes to the neural fallback; everything else stays on the
/// binary-channel default.
///
/// `Some(_)` is honored verbatim — explicit user choice always wins
/// over the auto-rule, including the case where a user opts into
/// `binary-channel` for a multi-party call.
#[must_use]
pub const fn select_kind(
    configured: Option<DiarizerKind>,
    capabilities: &Capabilities,
    ctx: &MeetingContext,
) -> DiarizerKind {
    match configured {
        Some(kind) => kind,
        None => {
            if requires_neural(capabilities, ctx) {
                DiarizerKind::PyannoteOnnx
            } else {
                DiarizerKind::BinaryChannel
            }
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use crate::types::PermissionModel;
    use pretty_assertions::assert_eq;

    fn caps(supports_system_audio: bool) -> Capabilities {
        Capabilities {
            supports_system_audio,
            supports_per_app_capture: false,
            native_sample_rates: vec![48_000],
            permission_model: PermissionModel::CoreAudioTap,
        }
    }

    #[test]
    fn test_diarizer_kind_default_is_binary_channel() {
        assert_eq!(DiarizerKind::default(), DiarizerKind::BinaryChannel);
    }

    #[test]
    fn test_diarizer_kind_from_config_str_parses_canonical_names() {
        assert_eq!(
            DiarizerKind::from_config_str("binary-channel"),
            Some(DiarizerKind::BinaryChannel),
        );
        assert_eq!(
            DiarizerKind::from_config_str("pyannote-onnx"),
            Some(DiarizerKind::PyannoteOnnx),
        );
    }

    #[test]
    fn test_diarizer_kind_from_config_str_returns_none_for_unknown_value() {
        assert_eq!(DiarizerKind::from_config_str("neural"), None);
        assert_eq!(DiarizerKind::from_config_str(""), None);
        assert_eq!(DiarizerKind::from_config_str("Binary-Channel"), None);
    }

    #[test]
    fn test_diarizer_kind_as_str_round_trips_through_from_config_str() {
        for kind in [DiarizerKind::BinaryChannel, DiarizerKind::PyannoteOnnx] {
            assert_eq!(DiarizerKind::from_config_str(kind.as_str()), Some(kind));
        }
    }

    #[test]
    fn test_diarizer_kind_display_matches_as_str() {
        assert_eq!(format!("{}", DiarizerKind::BinaryChannel), "binary-channel");
        assert_eq!(format!("{}", DiarizerKind::PyannoteOnnx), "pyannote-onnx");
    }

    #[test]
    fn test_select_kind_explicit_choice_overrides_auto_rule_even_for_multi_party() {
        let ctx = MeetingContext {
            attendees: vec!["a".into(), "b".into(), "c".into(), "d".into()],
            ..MeetingContext::default()
        };

        let chosen = select_kind(Some(DiarizerKind::BinaryChannel), &caps(true), &ctx);

        assert_eq!(chosen, DiarizerKind::BinaryChannel);
    }

    #[test]
    fn test_select_kind_explicit_pyannote_honored_for_one_on_one_remote_call() {
        let ctx = MeetingContext {
            attendees: vec!["me".into(), "them".into()],
            ..MeetingContext::default()
        };

        let chosen = select_kind(Some(DiarizerKind::PyannoteOnnx), &caps(true), &ctx);

        assert_eq!(chosen, DiarizerKind::PyannoteOnnx);
    }

    #[test]
    fn test_select_kind_auto_routes_multi_party_to_pyannote() {
        let ctx = MeetingContext {
            attendees: vec!["a".into(), "b".into(), "c".into()],
            ..MeetingContext::default()
        };

        let chosen = select_kind(None, &caps(true), &ctx);

        assert_eq!(chosen, DiarizerKind::PyannoteOnnx);
    }

    #[test]
    fn test_select_kind_auto_routes_single_channel_to_pyannote() {
        let ctx = MeetingContext::default();

        let chosen = select_kind(None, &caps(false), &ctx);

        assert_eq!(chosen, DiarizerKind::PyannoteOnnx);
    }

    #[test]
    fn test_select_kind_auto_routes_one_on_one_to_binary_channel() {
        let ctx = MeetingContext {
            attendees: vec!["me".into(), "them".into()],
            ..MeetingContext::default()
        };

        let chosen = select_kind(None, &caps(true), &ctx);

        assert_eq!(chosen, DiarizerKind::BinaryChannel);
    }

    #[test]
    fn test_select_kind_auto_routes_solo_session_to_binary_channel() {
        let ctx = MeetingContext::default();

        let chosen = select_kind(None, &caps(true), &ctx);

        assert_eq!(chosen, DiarizerKind::BinaryChannel);
    }
}
