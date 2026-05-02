// Copyright 2026 Mathews Tom
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     https://www.apache.org/licenses/LICENSE-2.0

//! Runtime backend detection for the Android capture adapter.
//!
//! `MediaProjection` is the primary path on Android API 29 (Android 10)
//! and newer; it captures the system audio mix the user hears. The
//! `MicOnly` fallback handles hosts that decline the `MediaProjection`
//! consent prompt or that run on API 28 or older. `Backend::Auto` walks
//! the host's API level and prefers `MediaProjection` when it is
//! supported and the user has granted consent.
//!
//! `ProbeResult` is constructible directly so tests can exercise the
//! resolution table across the four representative host shapes (no
//! Android runtime / API 28 / API 29+ without consent / API 29+ with
//! consent) without requiring a live Android runner.

/// Minimum Android API level that exposes `MediaProjection` for system-
/// audio capture (Android 10).
pub const MEDIA_PROJECTION_MIN_API: u32 = 29;

/// Audio backend selected at construction time. The `Auto` variant
/// resolves to a concrete backend at `AndroidCapture::start()`.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Backend {
    #[default]
    Auto,
    MediaProjection,
    MicOnly,
}

impl Backend {
    pub const AUTO_NAME: &'static str = "auto";
    pub const MEDIA_PROJECTION_NAME: &'static str = "media-projection";
    pub const MIC_ONLY_NAME: &'static str = "mic-only";

    /// Parse a backend name from the `android.audio_backend` config
    /// string. Returns `None` for unknown values so the config loader
    /// can surface a useful error rather than silently defaulting.
    #[must_use]
    pub fn from_config_str(s: &str) -> Option<Self> {
        match s {
            Self::AUTO_NAME => Some(Self::Auto),
            Self::MEDIA_PROJECTION_NAME => Some(Self::MediaProjection),
            Self::MIC_ONLY_NAME => Some(Self::MicOnly),
            _ => None,
        }
    }

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Auto => Self::AUTO_NAME,
            Self::MediaProjection => Self::MEDIA_PROJECTION_NAME,
            Self::MicOnly => Self::MIC_ONLY_NAME,
        }
    }
}

/// Result of probing the host runtime for available backends.
///
/// Each flag reflects a separate runtime gate: `MediaProjection`
/// requires API 29+ and a granted consent prompt; mic-only requires an
/// Android runtime at all.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ProbeResult {
    pub media_projection_present: bool,
    pub mic_only_present: bool,
}

impl ProbeResult {
    /// Build a probe result from a host API level. Tests use this to
    /// exercise the resolution table without invoking the real JNI
    /// `Build.VERSION.SDK_INT` query.
    ///
    /// `media_projection_consent_granted` is the gate for the consent
    /// prompt at session start: even on a supported API level, the
    /// adapter cannot fall back to `MediaProjection` if the user
    /// declined.
    #[must_use]
    pub const fn from_api_level(api_level: u32, media_projection_consent_granted: bool) -> Self {
        let media_projection_present =
            api_level >= MEDIA_PROJECTION_MIN_API && media_projection_consent_granted;
        let mic_only_present = api_level > 0;
        Self {
            media_projection_present,
            mic_only_present,
        }
    }

    /// Resolve the requested `Backend` against the probe outcome.
    /// Returns the concrete backend that should be wired up, or `None`
    /// if the requested backend is unavailable.
    ///
    /// `Auto` resolution prefers `MediaProjection` over `MicOnly` so
    /// users on a supported host with consent granted get system-audio
    /// capture; users without either condition fall through to the
    /// microphone-only path.
    #[must_use]
    pub const fn resolve(self, requested: Backend) -> Option<Backend> {
        match requested {
            Backend::Auto => {
                if self.media_projection_present {
                    Some(Backend::MediaProjection)
                } else if self.mic_only_present {
                    Some(Backend::MicOnly)
                } else {
                    None
                }
            }
            Backend::MediaProjection if self.media_projection_present => {
                Some(Backend::MediaProjection)
            }
            Backend::MicOnly if self.mic_only_present => Some(Backend::MicOnly),
            Backend::MediaProjection | Backend::MicOnly => None,
        }
    }
}

/// Resolve the configured backend against the live host.
///
/// Without the `media-projection` cargo feature (the default outside
/// the Android release build) and without a live Android runtime,
/// `detect()` returns `None` so the adapter surfaces a clean
/// `DeviceUnavailable` rather than promising a backend the host cannot
/// honour. The live JNI probe lands when the `media-projection`
/// feature wires up the JNI binding.
#[must_use]
pub const fn detect(requested: Backend) -> Option<Backend> {
    host_probe().resolve(requested)
}

#[cfg(any(not(target_os = "android"), not(feature = "media-projection")))]
const fn host_probe() -> ProbeResult {
    ProbeResult {
        media_projection_present: false,
        mic_only_present: false,
    }
}

#[cfg(all(target_os = "android", feature = "media-projection"))]
const fn host_probe() -> ProbeResult {
    // Live JNI binding pending — the placeholder mirrors the
    // non-Android branch so a `media-projection`-enabled build on an
    // Android host still surfaces `DeviceUnavailable` until the JNI
    // binding lands. Resolution-table tests use
    // `ProbeResult::from_api_level` directly.
    ProbeResult {
        media_projection_present: false,
        mic_only_present: false,
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn test_backend_from_config_str_parses_three_supported_names() {
        assert_eq!(Backend::from_config_str("auto"), Some(Backend::Auto));
        assert_eq!(
            Backend::from_config_str("media-projection"),
            Some(Backend::MediaProjection),
        );
        assert_eq!(Backend::from_config_str("mic-only"), Some(Backend::MicOnly),);
    }

    #[test]
    fn test_backend_from_config_str_returns_none_for_unknown_value() {
        assert_eq!(Backend::from_config_str("system"), None);
        assert_eq!(Backend::from_config_str(""), None);
        assert_eq!(Backend::from_config_str("Media-Projection"), None);
    }

    #[test]
    fn test_backend_as_str_round_trips_through_from_config_str() {
        for backend in [Backend::Auto, Backend::MediaProjection, Backend::MicOnly] {
            assert_eq!(Backend::from_config_str(backend.as_str()), Some(backend));
        }
    }

    #[test]
    fn test_backend_default_is_auto() {
        assert_eq!(Backend::default(), Backend::Auto);
    }

    #[test]
    fn test_probe_result_from_api_level_zero_means_no_android_runtime() {
        let probed = ProbeResult::from_api_level(0, true);

        assert!(!probed.media_projection_present);
        assert!(!probed.mic_only_present);
    }

    #[test]
    fn test_probe_result_from_api_level_28_supports_mic_only_only() {
        let probed = ProbeResult::from_api_level(28, true);

        assert!(!probed.media_projection_present);
        assert!(probed.mic_only_present);
    }

    #[test]
    fn test_probe_result_from_api_level_29_with_consent_supports_both_backends() {
        let probed = ProbeResult::from_api_level(29, true);

        assert!(probed.media_projection_present);
        assert!(probed.mic_only_present);
    }

    #[test]
    fn test_probe_result_from_api_level_29_without_consent_supports_mic_only_only() {
        let probed = ProbeResult::from_api_level(29, false);

        assert!(!probed.media_projection_present);
        assert!(probed.mic_only_present);
    }

    #[test]
    fn test_probe_result_from_api_level_34_with_consent_supports_both() {
        let probed = ProbeResult::from_api_level(34, true);

        assert!(probed.media_projection_present);
        assert!(probed.mic_only_present);
    }

    #[test]
    fn test_resolve_auto_with_only_media_projection_picks_media_projection() {
        let probed = ProbeResult {
            media_projection_present: true,
            mic_only_present: false,
        };

        assert_eq!(
            probed.resolve(Backend::Auto),
            Some(Backend::MediaProjection)
        );
    }

    #[test]
    fn test_resolve_auto_with_only_mic_only_picks_mic_only() {
        let probed = ProbeResult {
            media_projection_present: false,
            mic_only_present: true,
        };

        assert_eq!(probed.resolve(Backend::Auto), Some(Backend::MicOnly));
    }

    #[test]
    fn test_resolve_auto_with_both_backends_prefers_media_projection() {
        let probed = ProbeResult {
            media_projection_present: true,
            mic_only_present: true,
        };

        assert_eq!(
            probed.resolve(Backend::Auto),
            Some(Backend::MediaProjection)
        );
    }

    #[test]
    fn test_resolve_auto_with_neither_present_returns_none() {
        let probed = ProbeResult::default();

        assert_eq!(probed.resolve(Backend::Auto), None);
    }

    #[test]
    fn test_resolve_explicit_media_projection_when_unavailable_returns_none() {
        let probed = ProbeResult {
            media_projection_present: false,
            mic_only_present: true,
        };

        assert_eq!(probed.resolve(Backend::MediaProjection), None);
    }

    #[test]
    fn test_resolve_explicit_mic_only_when_unavailable_returns_none() {
        let probed = ProbeResult {
            media_projection_present: true,
            mic_only_present: false,
        };

        assert_eq!(probed.resolve(Backend::MicOnly), None);
    }

    #[test]
    fn test_resolve_explicit_media_projection_when_present_returns_media_projection() {
        let probed = ProbeResult {
            media_projection_present: true,
            mic_only_present: false,
        };

        assert_eq!(
            probed.resolve(Backend::MediaProjection),
            Some(Backend::MediaProjection),
        );
    }

    #[test]
    fn test_resolve_explicit_mic_only_when_present_returns_mic_only() {
        let probed = ProbeResult {
            media_projection_present: false,
            mic_only_present: true,
        };

        assert_eq!(probed.resolve(Backend::MicOnly), Some(Backend::MicOnly));
    }

    #[test]
    fn test_detect_on_non_android_host_returns_none_for_every_request() {
        // The host-probe stub on non-Android builds reports both
        // backends as unavailable, so `detect()` collapses to `None`
        // for `Auto`, `MediaProjection`, and `MicOnly` alike.
        assert_eq!(detect(Backend::Auto), None);
        assert_eq!(detect(Backend::MediaProjection), None);
        assert_eq!(detect(Backend::MicOnly), None);
    }

    #[test]
    fn test_backend_from_config_str_agrees_with_scrybe_core_constants() {
        assert_eq!(
            Backend::from_config_str(scrybe_core::config::ANDROID_AUDIO_BACKEND_AUTO),
            Some(Backend::Auto),
        );
        assert_eq!(
            Backend::from_config_str(scrybe_core::config::ANDROID_AUDIO_BACKEND_MEDIA_PROJECTION),
            Some(Backend::MediaProjection),
        );
        assert_eq!(
            Backend::from_config_str(scrybe_core::config::ANDROID_AUDIO_BACKEND_MIC_ONLY),
            Some(Backend::MicOnly),
        );
    }
}
