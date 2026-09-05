// Copyright 2026 Mathews Tom
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     https://www.apache.org/licenses/LICENSE-2.0

//! Synchronous, idempotent ownership of live capture adapters.

use std::sync::{Arc, Mutex};

#[cfg(feature = "mic-capture")]
use scrybe_core::capture::AudioCapture;
use scrybe_core::error::CaptureError;
type Stopper = Box<dyn FnMut() -> Result<(), CaptureError> + Send>;

/// Stops every registered adapter at most once.
#[derive(Clone, Default)]
pub struct CaptureRegistry {
    stoppers: Arc<Mutex<Vec<Stopper>>>,
}

impl CaptureRegistry {
    /// Retain `capture` until shutdown and return its shared owner.
    #[cfg(feature = "mic-capture")]
    pub fn register<T: AudioCapture>(&self, capture: T) -> Arc<Mutex<T>> {
        let capture = Arc::new(Mutex::new(capture));
        let stop_target = Arc::clone(&capture);
        self.register_stopper(move || {
            let mut capture = stop_target.lock().map_err(|_| {
                CaptureError::Platform(Box::new(std::io::Error::other(
                    "capture registry adapter mutex poisoned",
                )))
            })?;
            capture.stop()
        });
        capture
    }

    /// Retain a custom capture stop operation until shutdown.
    #[cfg(any(test, feature = "mic-capture"))]
    pub fn register_stopper(
        &self,
        stopper: impl FnMut() -> Result<(), CaptureError> + Send + 'static,
    ) {
        let mut stoppers = self
            .stoppers
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        stoppers.push(Box::new(stopper));
    }
    /// Synchronously stop and deregister all adapters.
    pub(crate) fn stop_all(&self) -> Result<(), CaptureError> {
        let mut stoppers = {
            let mut registered = self
                .stoppers
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            std::mem::take(&mut *registered)
        };
        let mut first_error = None;
        for stop in &mut stoppers {
            if let Err(error) = stop() {
                first_error.get_or_insert(error);
            }
        }
        first_error.map_or(Ok(()), Err)
    }
}
