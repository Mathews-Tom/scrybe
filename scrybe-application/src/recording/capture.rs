// Copyright 2026 Mathews Tom
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     https://www.apache.org/licenses/LICENSE-2.0

//! Owning live capture adapters, and opening the ones this build
//! carries.
//!
//! An adapter's stream is `'static` and outlives the adapter it came
//! from, so something has to keep the adapter alive and stoppable for
//! as long as frames are wanted. That is [`CaptureRegistry`], and every
//! frontend needs one for the same reason — it moved here from the
//! command-line recorder rather than being written a second time.
//!
//! Which adapters exist is a property of the build, and the richest
//! source a build can open is what [`super::CaptureSupport`] reports to
//! preflight. The two are written under the same conditions here, so a
//! preflight cannot clear a source this module then refuses to open.

use std::sync::{Arc, Mutex, PoisonError};

use scrybe_core::error::CaptureError;

/// One adapter's teardown, callable once.
type Stopper = Box<dyn FnMut() -> Result<(), CaptureError> + Send>;

/// A capture source's frames, boxed.
///
/// Boxed rather than returned as an `impl Stream`: `AudioCapture`
/// declares `frames` as a return-position `impl Trait` in a trait,
/// which captures the `&self` lifetime it was called on even though the
/// stream itself is `'static`. A trait object erases that, which is
/// what lets the adapter go on living in the registry while its frames
/// are consumed elsewhere.
pub type CaptureFrames = std::pin::Pin<
    Box<
        dyn futures::stream::Stream<Item = Result<scrybe_core::types::AudioFrame, CaptureError>>
            + Send,
    >,
>;

/// Stops every registered adapter at most once.
#[derive(Clone, Default)]
pub struct CaptureRegistry {
    stoppers: Arc<Mutex<Vec<Stopper>>>,
}

impl std::fmt::Debug for CaptureRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CaptureRegistry")
            .field(
                "registered",
                &self.stoppers.lock().map(|list| list.len()).ok(),
            )
            .finish()
    }
}

impl CaptureRegistry {
    /// Retains `capture` until shutdown and returns its shared owner.
    pub fn register<T: scrybe_core::capture::AudioCapture>(&self, capture: T) -> Arc<Mutex<T>> {
        let capture = Arc::new(Mutex::new(capture));
        let target = Arc::clone(&capture);
        self.register_stopper(move || {
            target
                .lock()
                .map_err(|_| {
                    CaptureError::Platform(Box::new(std::io::Error::other(
                        "capture registry adapter mutex poisoned",
                    )))
                })?
                .stop()
        });
        capture
    }

    /// Retains a custom teardown until shutdown.
    pub fn register_stopper(
        &self,
        stopper: impl FnMut() -> Result<(), CaptureError> + Send + 'static,
    ) {
        self.stoppers
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(Box::new(stopper));
    }

    /// Stops and deregisters every adapter, synchronously.
    ///
    /// Every teardown runs even if one fails, and the first failure is
    /// returned: stopping four adapters and giving up at the second
    /// would leave two devices open.
    ///
    /// # Errors
    ///
    /// The first [`CaptureError`] any adapter reported.
    pub fn stop_all(&self) -> Result<(), CaptureError> {
        let mut stoppers = {
            let mut registered = self.stoppers.lock().unwrap_or_else(PoisonError::into_inner);
            std::mem::take(&mut *registered)
        };
        let mut first = None;
        for stop in &mut stoppers {
            if let Err(error) = stop() {
                first.get_or_insert(error);
            }
        }
        first.map_or(Ok(()), Err)
    }
}

/// Frames from the platform's default input device.
///
/// Registered with `registry`, so the caller's one teardown reaches it.
///
/// # Errors
///
/// [`crate::error::ErrorCode::CaptureUnavailable`] when the device
/// cannot be opened — most often because the microphone permission has
/// not been granted, which is the moment the platform raises its own
/// prompt and the moment preflight said it could not check.
#[cfg(feature = "mic-capture")]
pub fn microphone_frames(registry: &CaptureRegistry) -> crate::Result<CaptureFrames> {
    use scrybe_core::capture::AudioCapture as _;

    let adapter = registry.register(scrybe_capture_mic::MicCapture::new());
    // The stream is `'static` and outlives this borrow; the adapter is
    // kept alive by the registry, which is also what stops it.
    let mut capture = adapter.lock().map_err(|_| {
        crate::error::ApplicationError::new(
            crate::error::ErrorCode::CaptureUnavailable,
            "the capture registry's adapter lock was poisoned",
        )
    })?;
    capture.start().map_err(|source| {
        crate::error::ApplicationError::new(
            crate::error::ErrorCode::CaptureUnavailable,
            "the default input device could not be opened; \
             grant the Microphone permission in System Settings if prompted",
        )
        .with_source(source)
    })?;
    Ok(Box::pin(capture.frames()))
}
