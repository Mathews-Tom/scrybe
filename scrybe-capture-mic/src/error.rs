// Copyright 2026 Mathews Tom
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     https://www.apache.org/licenses/LICENSE-2.0

//! Adapter-local error type for `scrybe-capture-mic`. Promotes to
//! `scrybe_core::error::CaptureError` at the trait boundary.

use scrybe_core::error::CaptureError;

#[derive(thiserror::Error, Debug)]
pub enum MicCaptureError {
    #[error("live-mic feature disabled at compile time")]
    FeatureDisabled,

    #[error("no default input device available on this host")]
    NoDefaultInputDevice,

    #[error("default input device returned no supported configurations")]
    NoSupportedConfig,

    #[error("cpal: {0}")]
    Cpal(String),
}

impl From<MicCaptureError> for CaptureError {
    fn from(err: MicCaptureError) -> Self {
        match err {
            MicCaptureError::FeatureDisabled => Self::PermissionDenied(err.to_string()),
            MicCaptureError::NoDefaultInputDevice | MicCaptureError::NoSupportedConfig => {
                Self::DeviceUnavailable(err.to_string())
            }
            MicCaptureError::Cpal(_) => {
                Self::Platform(Box::new(std::io::Error::other(err.to_string())))
            }
        }
    }
}
