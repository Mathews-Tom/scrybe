// Copyright 2026 Mathews Tom
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     https://www.apache.org/licenses/LICENSE-2.0

//! Adapter-local error type that the lifetime-erased
//! `CaptureError::Platform` carries up into `scrybe-core`. Mirrors the
//! adapter pattern in `docs/system-design.md` §4.6.

use scrybe_core::error::{BoxError, CaptureError};

#[derive(thiserror::Error, Debug)]
pub enum MacCaptureError {
    #[error("Core Audio Tap requires macOS 14.4 or later (found {found})")]
    CoreAudioTapUnsupported { found: String },

    #[error("ScreenCaptureKit error: {0}")]
    ScreenCaptureKit(#[source] BoxError),

    #[error("AVAudioEngine error: {0}")]
    AvAudioEngine(#[source] BoxError),

    #[error("TCC permission denied for {api}")]
    TccDenied { api: &'static str },
}

impl From<MacCaptureError> for CaptureError {
    fn from(e: MacCaptureError) -> Self {
        match e {
            MacCaptureError::TccDenied { api } => {
                Self::PermissionDenied(format!("macOS TCC: {api}"))
            }
            other => Self::Platform(Box::new(other)),
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn test_mac_capture_error_tcc_denied_promotes_to_capture_permission_denied() {
        let err = MacCaptureError::TccDenied {
            api: "ScreenCaptureKit",
        };

        let promoted: CaptureError = err.into();

        assert!(matches!(promoted, CaptureError::PermissionDenied(_)));
        assert_eq!(
            promoted.to_string(),
            "permission denied: macOS TCC: ScreenCaptureKit"
        );
    }

    #[test]
    fn test_mac_capture_error_unsupported_promotes_to_capture_platform() {
        let err = MacCaptureError::CoreAudioTapUnsupported {
            found: "14.0".into(),
        };

        let promoted: CaptureError = err.into();

        assert!(matches!(promoted, CaptureError::Platform(_)));
    }

    #[test]
    fn test_mac_capture_error_screencapturekit_chains_inner_source() {
        let inner = std::io::Error::other("ScrErrCanceled");
        let err = MacCaptureError::ScreenCaptureKit(Box::new(inner));

        let rendered = err.to_string();

        assert!(rendered.starts_with("ScreenCaptureKit error"));
        assert!(std::error::Error::source(&err).is_some());
    }
}
