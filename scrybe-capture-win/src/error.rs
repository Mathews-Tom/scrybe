// Copyright 2026 Mathews Tom
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     https://www.apache.org/licenses/LICENSE-2.0

//! Adapter-local error type for the Windows capture path.
//!
//! Carried up into `scrybe-core` through the lifetime-erased
//! `CaptureError::Platform` arm. Mirrors the adapter pattern in
//! `docs/system-design.md` §4.6 and the Linux adapter in
//! `scrybe-capture-linux::error`.

use scrybe_core::error::CaptureError;

/// Reason a Windows capture-backend request could not be honored.
///
/// Every variant funnels into [`CaptureError::DeviceUnavailable`] so
/// callers see a uniform error category regardless of which backend
/// failed; the rendered string preserves the specific cause for
/// diagnostics.
#[derive(thiserror::Error, Debug)]
pub enum WindowsCaptureError {
    #[error("WASAPI loopback backend not yet implemented in this release")]
    WasapiLoopbackDisabled,

    #[error("WASAPI per-process loopback backend not yet implemented in this release; system-wide loopback is the supported primary path")]
    WasapiProcessLoopbackDisabled,

    #[error("no supported audio backend detected on this host (WASAPI loopback requires Windows Vista+; per-process loopback requires Windows 10 build 20348+)")]
    NoBackendAvailable,

    #[error("requested backend `{requested}` is not available on this host")]
    RequestedBackendUnavailable { requested: &'static str },

    #[error(
        "WASAPI per-process loopback requires Windows 10 build 20348 or newer; detected build {build}"
    )]
    ProcessLoopbackRequiresNewerBuild { build: u32 },
}

impl From<WindowsCaptureError> for CaptureError {
    fn from(e: WindowsCaptureError) -> Self {
        Self::DeviceUnavailable(e.to_string())
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn test_windows_capture_error_wasapi_loopback_disabled_promotes_to_device_unavailable() {
        let err = WindowsCaptureError::WasapiLoopbackDisabled;

        let promoted: CaptureError = err.into();

        assert!(matches!(promoted, CaptureError::DeviceUnavailable(_)));
    }

    #[test]
    fn test_windows_capture_error_process_loopback_disabled_promotes_to_device_unavailable() {
        let err = WindowsCaptureError::WasapiProcessLoopbackDisabled;

        let promoted: CaptureError = err.into();

        assert!(matches!(promoted, CaptureError::DeviceUnavailable(_)));
    }

    #[test]
    fn test_windows_capture_error_no_backend_promotes_to_device_unavailable() {
        let err = WindowsCaptureError::NoBackendAvailable;

        let promoted: CaptureError = err.into();

        assert!(matches!(promoted, CaptureError::DeviceUnavailable(_)));
    }

    #[test]
    fn test_windows_capture_error_requested_unavailable_includes_backend_name() {
        let err = WindowsCaptureError::RequestedBackendUnavailable {
            requested: "wasapi-process-loopback",
        };

        let rendered = err.to_string();

        assert!(rendered.contains("wasapi-process-loopback"));
    }

    #[test]
    fn test_windows_capture_error_requested_unavailable_promotes_to_device_unavailable_with_backend_name(
    ) {
        let err = WindowsCaptureError::RequestedBackendUnavailable {
            requested: "wasapi-loopback",
        };

        let promoted: CaptureError = err.into();

        let CaptureError::DeviceUnavailable(message) = promoted else {
            panic!("expected DeviceUnavailable");
        };
        assert!(message.contains("wasapi-loopback"));
    }

    #[test]
    fn test_windows_capture_error_loopback_disabled_message_documents_release_state() {
        let err = WindowsCaptureError::WasapiLoopbackDisabled;

        let rendered = err.to_string();

        assert!(rendered.contains("WASAPI"));
        assert!(rendered.contains("not yet implemented"));
    }

    #[test]
    fn test_windows_capture_error_no_backend_message_names_required_windows_versions() {
        let err = WindowsCaptureError::NoBackendAvailable;

        let rendered = err.to_string();

        assert!(rendered.contains("Vista"));
        assert!(rendered.contains("20348"));
    }

    #[test]
    fn test_windows_capture_error_process_loopback_requires_newer_build_includes_detected_build() {
        let err = WindowsCaptureError::ProcessLoopbackRequiresNewerBuild { build: 19044 };

        let rendered = err.to_string();

        assert!(rendered.contains("20348"));
        assert!(rendered.contains("19044"));
    }

    #[test]
    fn test_windows_capture_error_process_loopback_requires_newer_build_promotes_to_device_unavailable(
    ) {
        let err = WindowsCaptureError::ProcessLoopbackRequiresNewerBuild { build: 19044 };

        let promoted: CaptureError = err.into();

        let CaptureError::DeviceUnavailable(message) = promoted else {
            panic!("expected DeviceUnavailable");
        };
        assert!(message.contains("19044"));
    }
}
