// Copyright 2026 Mathews Tom
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     https://www.apache.org/licenses/LICENSE-2.0

//! Adapter-local error type for the Android capture path.
//!
//! Carried up into `scrybe-core` through the lifetime-erased
//! `CaptureError::Platform` arm. Mirrors the adapter pattern in
//! `docs/system-design.md` §4.6 and the Linux / Windows adapters.

use scrybe_core::error::CaptureError;

/// Reason an Android capture-backend request could not be honored.
///
/// Every variant funnels into [`CaptureError::DeviceUnavailable`] so
/// callers see a uniform error category regardless of which backend
/// failed; the rendered string preserves the specific cause for
/// diagnostics.
#[derive(thiserror::Error, Debug)]
pub enum AndroidCaptureError {
    #[error("MediaProjection backend not yet implemented in this release; the live JNI binding is a v0.5.x follow-up")]
    MediaProjectionDisabled,

    #[error("microphone-only fallback not yet implemented in this release; the live JNI binding is a v0.5.x follow-up")]
    MicOnlyDisabled,

    #[error(
        "no supported Android audio backend detected on this host (MediaProjection requires API 29+; microphone-only requires an Android runtime)"
    )]
    NoBackendAvailable,

    #[error("requested backend `{requested}` is not available on this host")]
    RequestedBackendUnavailable { requested: &'static str },

    #[error("MediaProjection requires Android API 29 or newer; detected API {api_level}")]
    MediaProjectionRequiresNewerApi { api_level: u32 },

    #[error("user declined the MediaProjection consent prompt at session start")]
    UserDeclinedConsent,
}

impl From<AndroidCaptureError> for CaptureError {
    fn from(e: AndroidCaptureError) -> Self {
        match e {
            AndroidCaptureError::UserDeclinedConsent => {
                Self::PermissionDenied("Android MediaProjection: user declined".into())
            }
            other => Self::DeviceUnavailable(other.to_string()),
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn test_android_capture_error_media_projection_disabled_promotes_to_device_unavailable() {
        let err = AndroidCaptureError::MediaProjectionDisabled;

        let promoted: CaptureError = err.into();

        assert!(matches!(promoted, CaptureError::DeviceUnavailable(_)));
    }

    #[test]
    fn test_android_capture_error_mic_only_disabled_promotes_to_device_unavailable() {
        let err = AndroidCaptureError::MicOnlyDisabled;

        let promoted: CaptureError = err.into();

        assert!(matches!(promoted, CaptureError::DeviceUnavailable(_)));
    }

    #[test]
    fn test_android_capture_error_no_backend_promotes_to_device_unavailable() {
        let err = AndroidCaptureError::NoBackendAvailable;

        let promoted: CaptureError = err.into();

        assert!(matches!(promoted, CaptureError::DeviceUnavailable(_)));
    }

    #[test]
    fn test_android_capture_error_requested_unavailable_includes_backend_name() {
        let err = AndroidCaptureError::RequestedBackendUnavailable {
            requested: "media-projection",
        };

        let rendered = err.to_string();

        assert!(rendered.contains("media-projection"));
    }

    #[test]
    fn test_android_capture_error_requested_unavailable_promotes_to_device_unavailable_with_backend_name(
    ) {
        let err = AndroidCaptureError::RequestedBackendUnavailable {
            requested: "mic-only",
        };

        let promoted: CaptureError = err.into();

        let CaptureError::DeviceUnavailable(message) = promoted else {
            panic!("expected DeviceUnavailable");
        };
        assert!(message.contains("mic-only"));
    }

    #[test]
    fn test_android_capture_error_media_projection_requires_newer_api_includes_detected_level() {
        let err = AndroidCaptureError::MediaProjectionRequiresNewerApi { api_level: 28 };

        let rendered = err.to_string();

        assert!(rendered.contains("29"));
        assert!(rendered.contains("28"));
    }

    #[test]
    fn test_android_capture_error_media_projection_requires_newer_api_promotes_to_device_unavailable(
    ) {
        let err = AndroidCaptureError::MediaProjectionRequiresNewerApi { api_level: 28 };

        let promoted: CaptureError = err.into();

        let CaptureError::DeviceUnavailable(message) = promoted else {
            panic!("expected DeviceUnavailable");
        };
        assert!(message.contains("28"));
    }

    #[test]
    fn test_android_capture_error_user_declined_consent_promotes_to_permission_denied() {
        let err = AndroidCaptureError::UserDeclinedConsent;

        let promoted: CaptureError = err.into();

        assert!(matches!(promoted, CaptureError::PermissionDenied(_)));
    }

    #[test]
    fn test_android_capture_error_no_backend_message_names_required_api_levels() {
        let err = AndroidCaptureError::NoBackendAvailable;

        let rendered = err.to_string();

        assert!(rendered.contains("29"));
        assert!(rendered.contains("MediaProjection"));
    }

    #[test]
    fn test_android_capture_error_media_projection_disabled_message_documents_release_state() {
        let err = AndroidCaptureError::MediaProjectionDisabled;

        let rendered = err.to_string();

        assert!(rendered.contains("MediaProjection"));
        assert!(rendered.contains("not yet implemented"));
    }
}
