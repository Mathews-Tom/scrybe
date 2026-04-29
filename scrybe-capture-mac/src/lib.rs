// Copyright 2026 Mathews Tom
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     https://www.apache.org/licenses/LICENSE-2.0

//! macOS audio capture adapter implementing `scrybe_core::capture::AudioCapture`.
//!
//! Two paths:
//!
//! - **Core Audio Taps** (macOS 14.4+, primary). Behind the
//!   `core-audio-tap` feature. Requires the Tap-creation prompt, no
//!   Screen Recording permission, no orange dot. Hardware validation
//!   is gated by `SCRYBE_TEST_CAPTURE=1` per `docs/system-design.md`
//!   §11 Tier 2.
//! - **`ScreenCaptureKit`** (macOS 13.0–14.3 fallback). Triggers
//!   Screen Recording permission and orange dot for audio-only
//!   capture; documented as a fallback path in `system-design.md` §3.
//!   Implementation lands in v0.2 — v0.1 of this adapter ships
//!   Core Audio Taps only and refuses to start on older macOS.
//!
//! Without the `core-audio-tap` feature, `MacCapture::start()` returns
//! `CaptureError::PermissionDenied("core-audio-tap feature disabled")`
//! so consumer code on Linux/Windows hosts can still link this crate
//! during cross-platform CI (the workspace runs `cargo check` on three
//! runners; only the macOS runner enables the feature).

pub mod error;

#[cfg(all(target_os = "macos", feature = "core-audio-tap"))]
mod coreaudio_tap;

pub use error::MacCaptureError;

use std::sync::{Arc, Mutex};

use futures::stream::{self, Stream};
use scrybe_core::capture::AudioCapture;
use scrybe_core::error::CaptureError;
use scrybe_core::types::{AudioFrame, Capabilities, PermissionModel};
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver};
use tokio_stream::wrappers::UnboundedReceiverStream;

mod tokio_stream {
    pub mod wrappers {
        use std::pin::Pin;
        use std::task::{Context, Poll};

        use futures::stream::Stream;
        use tokio::sync::mpsc::UnboundedReceiver;

        /// Minimal in-tree replacement for `tokio_stream::wrappers::UnboundedReceiverStream`
        /// so this adapter does not depend on the external `tokio-stream`
        /// crate. The wrapper closes the stream when the inner channel
        /// closes, which is the only behavior the pipeline relies on.
        pub struct UnboundedReceiverStream<T> {
            inner: UnboundedReceiver<T>,
        }

        impl<T> UnboundedReceiverStream<T> {
            pub const fn new(inner: UnboundedReceiver<T>) -> Self {
                Self { inner }
            }
        }

        impl<T> Stream for UnboundedReceiverStream<T> {
            type Item = T;

            fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<T>> {
                self.inner.poll_recv(cx)
            }
        }
    }
}

/// State shared between `MacCapture` and the platform callback.
struct SharedState {
    sender: Option<tokio::sync::mpsc::UnboundedSender<Result<AudioFrame, CaptureError>>>,
    receiver: Option<UnboundedReceiver<Result<AudioFrame, CaptureError>>>,
    started: bool,
    /// Live Core Audio Tap binding owned for the lifetime of the
    /// session. `None` until `start()` succeeds; cleared by `stop()`
    /// (which transitively destroys the tap, aggregate device, and IO
    /// proc id).
    #[cfg(all(target_os = "macos", feature = "core-audio-tap"))]
    tap: Option<crate::coreaudio_tap::TapStream>,
}

/// macOS `AudioCapture` adapter.
pub struct MacCapture {
    state: Arc<Mutex<SharedState>>,
    capabilities: Capabilities,
}

impl MacCapture {
    /// Construct an adapter targeting Core Audio Taps. Capability
    /// reporting reflects whether the build can actually call the
    /// platform API: with the `core-audio-tap` feature enabled on
    /// macOS, system-audio capture is advertised as supported.
    #[must_use]
    pub fn new() -> Self {
        let (tx, rx) = unbounded_channel();
        let state = Arc::new(Mutex::new(SharedState {
            sender: Some(tx),
            receiver: Some(rx),
            started: false,
            #[cfg(all(target_os = "macos", feature = "core-audio-tap"))]
            tap: None,
        }));
        let supports_system_audio = cfg!(all(target_os = "macos", feature = "core-audio-tap"));
        let capabilities = Capabilities {
            supports_system_audio,
            supports_per_app_capture: false,
            native_sample_rates: vec![48_000],
            permission_model: PermissionModel::CoreAudioTap,
        };
        Self {
            state,
            capabilities,
        }
    }

    /// Inject a frame for tests. Synchronous, lock-bounded; never used
    /// in production builds. Public so integration tests in
    /// `scrybe-cli` can drive the adapter through the same surface
    /// the platform callback uses.
    pub fn inject_for_test(&self, frame: Result<AudioFrame, CaptureError>) {
        let guard = match self.state.lock() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        };
        if let Some(sender) = guard.sender.as_ref() {
            let _ = sender.send(frame);
        }
    }

    /// Close the capture stream. Public so tests can simulate the
    /// platform callback's "stream ended" signal.
    pub fn close_for_test(&self) {
        let mut guard = match self.state.lock() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        };
        guard.sender.take();
    }
}

impl Default for MacCapture {
    fn default() -> Self {
        Self::new()
    }
}

impl AudioCapture for MacCapture {
    #[allow(
        unused_variables,
        unused_mut,
        clippy::needless_return,
        clippy::significant_drop_tightening
    )]
    fn start(&mut self) -> Result<(), CaptureError> {
        let mut guard = self.state.lock().map_err(|_| {
            CaptureError::Platform(Box::new(std::io::Error::new(
                std::io::ErrorKind::Other,
                "MacCapture state mutex poisoned",
            )))
        })?;
        if guard.started {
            return Ok(());
        }
        // Adapter is single-use across `start`→`stop`→`start` in v0.1:
        // stop() drops the channel sender. Callers construct a fresh
        // `MacCapture` for a new session.
        let sender = guard.sender.clone().ok_or_else(|| {
            CaptureError::PermissionDenied(
                "MacCapture::start called after stop; construct a new \
                 MacCapture for a new session"
                    .to_string(),
            )
        })?;

        #[cfg(all(target_os = "macos", feature = "core-audio-tap"))]
        {
            let mut tap =
                crate::coreaudio_tap::TapStream::create(sender).map_err(CaptureError::from)?;
            tap.start().map_err(CaptureError::from)?;
            guard.tap = Some(tap);
            guard.started = true;
            return Ok(());
        }
        #[cfg(not(all(target_os = "macos", feature = "core-audio-tap")))]
        {
            Err(CaptureError::PermissionDenied(
                "core-audio-tap feature disabled at compile time".to_string(),
            ))
        }
    }

    #[allow(clippy::significant_drop_tightening)]
    fn stop(&mut self) -> Result<(), CaptureError> {
        let mut guard = self.state.lock().map_err(|_| {
            CaptureError::Platform(Box::new(std::io::Error::new(
                std::io::ErrorKind::Other,
                "MacCapture state mutex poisoned",
            )))
        })?;
        guard.started = false;
        guard.sender.take();
        #[cfg(all(target_os = "macos", feature = "core-audio-tap"))]
        {
            if let Some(mut tap) = guard.tap.take() {
                tap.stop().map_err(CaptureError::from)?;
            }
        }
        Ok(())
    }

    fn frames(&self) -> impl Stream<Item = Result<AudioFrame, CaptureError>> + Send + 'static {
        let mut guard = match self.state.lock() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        };
        guard.receiver.take().map_or_else(
            || Box::pin(stream::empty()) as std::pin::Pin<Box<dyn Stream<Item = _> + Send>>,
            |rx| Box::pin(UnboundedReceiverStream::new(rx)),
        )
    }

    fn capabilities(&self) -> Capabilities {
        self.capabilities.clone()
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use std::sync::Arc;

    use futures::stream::StreamExt;
    use scrybe_core::types::{AudioFrame, FrameSource};

    use super::*;
    use pretty_assertions::assert_eq;

    fn frame(timestamp_ns: u64) -> AudioFrame {
        AudioFrame {
            samples: Arc::from(vec![0.0_f32; 48]),
            channels: 1,
            sample_rate: 48_000,
            timestamp_ns,
            source: FrameSource::System,
        }
    }

    #[tokio::test]
    async fn test_mac_capture_inject_for_test_yields_frames_through_stream() {
        let cap = MacCapture::new();
        let stream = cap.frames();

        cap.inject_for_test(Ok(frame(1)));
        cap.inject_for_test(Ok(frame(2)));
        cap.close_for_test();

        let collected: Vec<_> = Box::pin(stream).collect().await;

        assert_eq!(collected.len(), 2);
        assert_eq!(collected[0].as_ref().unwrap().timestamp_ns, 1);
        assert_eq!(collected[1].as_ref().unwrap().timestamp_ns, 2);
    }

    #[tokio::test]
    async fn test_mac_capture_propagates_capture_errors_via_stream() {
        let cap = MacCapture::new();
        let stream = cap.frames();

        cap.inject_for_test(Err(CaptureError::SystemSlept { at_secs: 30 }));
        cap.close_for_test();

        let collected: Vec<_> = Box::pin(stream).collect().await;

        assert_eq!(collected.len(), 1);
        assert!(matches!(
            collected[0],
            Err(CaptureError::SystemSlept { at_secs: 30 })
        ));
    }

    #[test]
    fn test_mac_capture_capabilities_reports_core_audio_tap_permission_model() {
        let cap = MacCapture::new();

        let caps = cap.capabilities();

        assert_eq!(caps.permission_model, PermissionModel::CoreAudioTap);
        assert_eq!(caps.native_sample_rates, vec![48_000]);
        assert!(!caps.supports_per_app_capture);
        // System-audio capability tracks the live binding: enabled iff
        // the build is macOS-targeted with the `core-audio-tap`
        // feature on.
        let expects_system_audio = cfg!(all(target_os = "macos", feature = "core-audio-tap"));
        assert_eq!(caps.supports_system_audio, expects_system_audio);
    }

    #[cfg(not(all(target_os = "macos", feature = "core-audio-tap")))]
    #[test]
    fn test_mac_capture_start_without_feature_returns_permission_denied() {
        let mut cap = MacCapture::new();

        let err = cap.start().unwrap_err();

        assert!(matches!(err, CaptureError::PermissionDenied(_)));
    }

    #[cfg(not(all(target_os = "macos", feature = "core-audio-tap")))]
    #[test]
    fn test_mac_capture_start_after_stop_returns_permission_denied_for_reuse() {
        let mut cap = MacCapture::new();
        let _ = cap.start();
        cap.stop().unwrap();

        let err = cap.start().unwrap_err();

        assert!(matches!(err, CaptureError::PermissionDenied(_)));
    }

    #[cfg(not(all(target_os = "macos", feature = "core-audio-tap")))]
    #[test]
    fn test_mac_capture_stop_after_start_is_idempotent() {
        let mut cap = MacCapture::new();
        let _ = cap.start();

        cap.stop().unwrap();
        cap.stop().unwrap();
    }

    #[tokio::test]
    async fn test_mac_capture_second_frames_call_returns_empty_stream() {
        let cap = MacCapture::new();

        let first = cap.frames();
        cap.inject_for_test(Ok(frame(7)));
        cap.close_for_test();
        let collected_first: Vec<_> = Box::pin(first).collect().await;

        let second = cap.frames();
        let collected_second: Vec<_> = Box::pin(second).collect().await;

        assert_eq!(collected_first.len(), 1);
        assert_eq!(collected_second.len(), 0);
    }
}
