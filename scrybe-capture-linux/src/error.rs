// Copyright 2026 Mathews Tom
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     https://www.apache.org/licenses/LICENSE-2.0

//! Adapter-local error type that the lifetime-erased
//! `CaptureError::Platform` carries up into `scrybe-core`. Mirrors the
//! adapter pattern in `docs/system-design.md` §4.6.

use scrybe_core::error::CaptureError;

#[derive(thiserror::Error, Debug)]
pub enum LinuxCaptureError {
    #[error("PipeWire backend not yet implemented in this release")]
    PipeWireDisabled,

    #[error("PulseAudio backend not yet implemented in this release; PipeWire is the supported primary path")]
    PulseDisabled,

    #[error("no supported audio backend detected on this host (looked for PipeWire and PulseAudio sockets under XDG_RUNTIME_DIR)")]
    NoBackendAvailable,

    #[error("requested backend `{requested}` is not available on this host")]
    RequestedBackendUnavailable { requested: &'static str },
}

impl From<LinuxCaptureError> for CaptureError {
    fn from(e: LinuxCaptureError) -> Self {
        Self::DeviceUnavailable(e.to_string())
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn test_linux_capture_error_pipewire_disabled_promotes_to_device_unavailable() {
        let err = LinuxCaptureError::PipeWireDisabled;

        let promoted: CaptureError = err.into();

        assert!(matches!(promoted, CaptureError::DeviceUnavailable(_)));
    }

    #[test]
    fn test_linux_capture_error_pulse_disabled_promotes_to_device_unavailable() {
        let err = LinuxCaptureError::PulseDisabled;

        let promoted: CaptureError = err.into();

        assert!(matches!(promoted, CaptureError::DeviceUnavailable(_)));
    }

    #[test]
    fn test_linux_capture_error_no_backend_promotes_to_device_unavailable() {
        let err = LinuxCaptureError::NoBackendAvailable;

        let promoted: CaptureError = err.into();

        assert!(matches!(promoted, CaptureError::DeviceUnavailable(_)));
    }

    #[test]
    fn test_linux_capture_error_requested_unavailable_includes_backend_name() {
        let err = LinuxCaptureError::RequestedBackendUnavailable {
            requested: "pipewire",
        };

        let rendered = err.to_string();

        assert!(rendered.contains("pipewire"));
    }

    #[test]
    fn test_linux_capture_error_requested_unavailable_promotes_to_device_unavailable_with_backend_name(
    ) {
        let err = LinuxCaptureError::RequestedBackendUnavailable { requested: "pulse" };

        let promoted: CaptureError = err.into();

        let CaptureError::DeviceUnavailable(message) = promoted else {
            panic!("expected DeviceUnavailable");
        };
        assert!(message.contains("pulse"));
    }

    #[test]
    fn test_linux_capture_error_pipewire_disabled_message_documents_release_state() {
        let err = LinuxCaptureError::PipeWireDisabled;

        let rendered = err.to_string();

        assert!(rendered.contains("PipeWire"));
        assert!(rendered.contains("not yet implemented"));
    }

    #[test]
    fn test_linux_capture_error_no_backend_message_names_xdg_runtime_dir() {
        let err = LinuxCaptureError::NoBackendAvailable;

        let rendered = err.to_string();

        assert!(rendered.contains("XDG_RUNTIME_DIR"));
        assert!(rendered.contains("PipeWire"));
        assert!(rendered.contains("PulseAudio"));
    }
}
