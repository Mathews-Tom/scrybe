// Copyright 2026 Mathews Tom
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     https://www.apache.org/licenses/LICENSE-2.0

//! `AudioCapture` — the platform contract.
//!
//! Tier-1 stability: frozen at v1.0. The `Stream`-returning shape replaces
//! the original `mpsc::Receiver` draft (`.docs/development-plan.md` §2.2 H8)
//! so capture errors are in-band and the pipeline composes through
//! `futures::StreamExt`.

use futures::stream::Stream;

use crate::error::CaptureError;
use crate::types::{AudioFrame, Capabilities};

/// Source of platform audio frames.
///
/// Implementations are concrete adapter crates compiled in via cargo
/// features (`scrybe-capture-mac`, `scrybe-capture-win`,
/// `scrybe-capture-linux`, `scrybe-capture-android`).
///
/// `Send + 'static` so `start()` / `stop()` can be invoked from a tokio
/// task without lifetime constraints. The trait is **not** dyn-compatible
/// because `frames()` returns `impl Stream`; that is intentional. Adapter
/// types are concrete; static dispatch keeps the pipeline allocation-free.
pub trait AudioCapture: Send + 'static {
    /// Begin capture. Permission prompts (macOS Core Audio Taps tap-creation
    /// or Screen Recording, Android `MediaProjection`) happen here.
    ///
    /// # Errors
    ///
    /// Returns `CaptureError::PermissionDenied` if the OS denies access,
    /// `CaptureError::DeviceUnavailable` if no input device matches the
    /// configured selector, or `CaptureError::Platform` for any other
    /// platform-API failure.
    fn start(&mut self) -> Result<(), CaptureError>;

    /// Stop capture. Idempotent — calling on an already-stopped instance
    /// returns `Ok(())`.
    ///
    /// # Errors
    ///
    /// Returns `CaptureError::Platform` if the underlying platform API
    /// reports a teardown error.
    fn stop(&mut self) -> Result<(), CaptureError>;

    /// Stream of captured frames. The stream yields `Err` for transient or
    /// terminal capture errors and closes when `stop()` is called.
    ///
    /// Implementations typically build this from a tokio mpsc receiver
    /// stored behind interior mutability (`Mutex<Option<Receiver>>` or
    /// `OnceCell`). Calling `frames()` more than once on the same
    /// instance is permitted to return `stream::empty()` for subsequent
    /// calls; the contract is "exactly one consumer per session".
    fn frames(&self) -> impl Stream<Item = Result<AudioFrame, CaptureError>> + Send + 'static;

    /// Static metadata about what this implementation can do.
    fn capabilities(&self) -> Capabilities;
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use std::sync::{Arc, Mutex};

    use futures::stream::{self, StreamExt};

    use super::*;
    use crate::types::{AudioFrame, FrameSource, PermissionModel};

    struct FakeCapture {
        backlog: Mutex<Option<Vec<Result<AudioFrame, CaptureError>>>>,
        started: Mutex<bool>,
    }

    impl FakeCapture {
        const fn with_backlog(backlog: Vec<Result<AudioFrame, CaptureError>>) -> Self {
            Self {
                backlog: Mutex::new(Some(backlog)),
                started: Mutex::new(false),
            }
        }
    }

    impl AudioCapture for FakeCapture {
        fn start(&mut self) -> Result<(), CaptureError> {
            *self.started.lock().unwrap() = true;
            Ok(())
        }

        fn stop(&mut self) -> Result<(), CaptureError> {
            *self.started.lock().unwrap() = false;
            Ok(())
        }

        fn frames(&self) -> impl Stream<Item = Result<AudioFrame, CaptureError>> + Send + 'static {
            let items = self.backlog.lock().unwrap().take().unwrap_or_default();
            stream::iter(items)
        }

        fn capabilities(&self) -> Capabilities {
            Capabilities {
                supports_system_audio: true,
                supports_per_app_capture: false,
                native_sample_rates: vec![48_000],
                permission_model: PermissionModel::CoreAudioTap,
            }
        }
    }

    fn frame(timestamp_ns: u64) -> AudioFrame {
        AudioFrame {
            samples: Arc::from(vec![0.0_f32; 4]),
            channels: 1,
            sample_rate: 48_000,
            timestamp_ns,
            source: FrameSource::Mic,
        }
    }

    #[tokio::test]
    async fn test_capture_stream_yields_frames_in_order_then_closes() {
        let mut cap = FakeCapture::with_backlog(vec![Ok(frame(1)), Ok(frame(2))]);
        cap.start().unwrap();

        let collected: Vec<_> = cap.frames().collect().await;
        cap.stop().unwrap();

        assert_eq!(collected.len(), 2);
        assert_eq!(collected[0].as_ref().unwrap().timestamp_ns, 1);
        assert_eq!(collected[1].as_ref().unwrap().timestamp_ns, 2);
    }

    #[tokio::test]
    async fn test_capture_stream_propagates_capture_errors_in_band() {
        let mut cap = FakeCapture::with_backlog(vec![Err(CaptureError::DeviceChanged {
            was: "MacBook Pro Microphone".into(),
            now: "AirPods Pro".into(),
        })]);
        cap.start().unwrap();

        let mut stream = Box::pin(cap.frames());
        let next = stream.next().await.unwrap();

        assert!(matches!(next, Err(CaptureError::DeviceChanged { .. })));
    }

    #[tokio::test]
    async fn test_capture_stream_second_call_returns_empty_stream() {
        let cap = FakeCapture::with_backlog(vec![Ok(frame(7))]);

        let first: Vec<_> = cap.frames().collect().await;
        let second: Vec<_> = cap.frames().collect().await;

        assert_eq!(first.len(), 1);
        assert_eq!(second.len(), 0);
    }

    #[test]
    fn test_capabilities_default_for_fake_capture_reports_core_audio_tap() {
        let cap = FakeCapture::with_backlog(vec![]);

        let caps = cap.capabilities();

        assert!(caps.supports_system_audio);
        assert_eq!(caps.permission_model, PermissionModel::CoreAudioTap);
    }
}
