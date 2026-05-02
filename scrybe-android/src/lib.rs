// Copyright 2026 Mathews Tom
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     https://www.apache.org/licenses/LICENSE-2.0

//! Android audio capture adapter implementing `scrybe_core::capture::AudioCapture`.
//!
//! This release ships the trait surface, runtime backend detection
//! (Android API level via the placeholder probe), and the
//! `[android] audio_backend = "auto" | "media-projection" | "mic-only"`
//! configuration block. The live `MediaProjection` JNI binding (and the
//! uniffi-generated Kotlin facade for the Compose UI shell) is tracked
//! as a v0.5.x follow-up: `AndroidCapture::start()` resolves the
//! requested backend against the live host and surfaces a clear
//! `CaptureError::DeviceUnavailable` so a configurator can decide
//! whether to fall back to a different backend or wait on the live
//! binding work.
//!
//! See `.docs/development-plan.md` §11 for the dependency rationale and
//! `docs/system-design.md` §11 for the test tier matrix.

// `unnecessary_wraps`, `unused_self`, and `needless_pass_by_ref_mut`
// fire on the always-stub build because the inner match collapses to a
// single arm; the signatures match the parallel `scrybe-capture-mac`,
// `scrybe-capture-linux`, and `scrybe-capture-win` shapes so a future
// live binding can drop in without re-plumbing callers.
#![allow(
    clippy::unnecessary_wraps,
    clippy::unused_self,
    clippy::needless_pass_by_ref_mut
)]

pub mod backend;
pub mod error;

pub use backend::{detect, Backend, ProbeResult, MEDIA_PROJECTION_MIN_API};
pub use error::AndroidCaptureError;

use std::sync::{Arc, Mutex};

use futures::stream::{self, Stream};
use scrybe_core::capture::AudioCapture;
use scrybe_core::error::CaptureError;
use scrybe_core::types::{AudioFrame, Capabilities, PermissionModel};
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender};

/// Channel item: either a captured frame or a terminal capture error
/// surfaced in-band per `docs/system-design.md` §4.1.
type FrameItem = Result<AudioFrame, CaptureError>;

/// Live backend stream owned for the lifetime of a session. The
/// `AndroidCapture::start()` short-circuits before constructing one in
/// this release; the type is shaped to match the macOS / Linux /
/// Windows adapters so a future live binding can slot a real variant
/// in alongside `Stub` without re-plumbing the call sites.
#[allow(dead_code)]
enum LiveStream {
    Stub,
}

impl LiveStream {
    const fn stop(&mut self) -> Result<(), CaptureError> {
        match self {
            Self::Stub => Ok(()),
        }
    }
}

/// State shared between `AndroidCapture` and the platform callback.
struct SharedState {
    sender: Option<UnboundedSender<FrameItem>>,
    receiver: Option<UnboundedReceiver<FrameItem>>,
    started: bool,
    live: Option<LiveStream>,
}

/// Android `AudioCapture` adapter.
///
/// Construct with [`AndroidCapture::new`] (auto-select backend) or
/// [`AndroidCapture::with_backend`] (explicit). The selected backend is
/// recorded but resolution against the live host is deferred until
/// `start()` so the capabilities surface stays cheap.
pub struct AndroidCapture {
    state: Arc<Mutex<SharedState>>,
    requested_backend: Backend,
    capabilities: Capabilities,
}

impl AndroidCapture {
    /// Construct with `Backend::Auto`. Runtime backend resolution
    /// happens at `start()`.
    #[must_use]
    pub fn new() -> Self {
        Self::with_backend(Backend::Auto)
    }

    /// Construct with an explicit backend.
    #[must_use]
    pub fn with_backend(requested: Backend) -> Self {
        let (tx, rx) = unbounded_channel();
        let state = Arc::new(Mutex::new(SharedState {
            sender: Some(tx),
            receiver: Some(rx),
            started: false,
            live: None,
        }));
        let capabilities = Capabilities {
            // Android capture targets the user's playback mix via
            // `MediaProjection` when the live binding lands. The
            // capability is reported as `false` until the JNI binding
            // ships; per-process loopback (Android does not expose a
            // documented per-app system-audio API) stays `false`.
            supports_system_audio: false,
            supports_per_app_capture: false,
            native_sample_rates: vec![48_000],
            permission_model: PermissionModel::MediaProjection,
        };
        Self {
            state,
            requested_backend: requested,
            capabilities,
        }
    }

    /// Backend the consumer asked for. The actual resolved backend is
    /// available after `start()` via [`Self::resolved_backend`].
    #[must_use]
    pub const fn requested_backend(&self) -> Backend {
        self.requested_backend
    }

    /// Backend that `start()` resolved to. Returns `None` before
    /// `start()` succeeds or after `stop()` clears live state.
    #[must_use]
    pub fn resolved_backend(&self) -> Option<Backend> {
        let guard = match self.state.lock() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        };
        guard.live.as_ref().and_then(|live| match live {
            LiveStream::Stub => None,
        })
    }

    /// Inject a frame for tests. Synchronous, lock-bounded; never used
    /// in production builds. Public so integration tests can drive the
    /// adapter through the same surface a platform callback uses.
    pub fn inject_for_test(&self, frame: FrameItem) {
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

impl Default for AndroidCapture {
    fn default() -> Self {
        Self::new()
    }
}

impl AudioCapture for AndroidCapture {
    #[allow(clippy::significant_drop_tightening)]
    fn start(&mut self) -> Result<(), CaptureError> {
        let mut guard = self.state.lock().map_err(|_| {
            CaptureError::Platform(Box::new(std::io::Error::other(
                "AndroidCapture state mutex poisoned",
            )))
        })?;
        if guard.started {
            return Ok(());
        }
        let sender = guard.sender.clone().ok_or_else(|| {
            CaptureError::DeviceUnavailable(
                "AndroidCapture::start called after stop; construct a new \
                 AndroidCapture for a new session"
                    .to_string(),
            )
        })?;

        let resolved = backend::detect(self.requested_backend).ok_or_else(|| {
            let err = match self.requested_backend {
                Backend::Auto => AndroidCaptureError::NoBackendAvailable,
                Backend::MediaProjection => AndroidCaptureError::RequestedBackendUnavailable {
                    requested: Backend::MEDIA_PROJECTION_NAME,
                },
                Backend::MicOnly => AndroidCaptureError::RequestedBackendUnavailable {
                    requested: Backend::MIC_ONLY_NAME,
                },
            };
            CaptureError::from(err)
        })?;

        let live = Self::start_resolved(resolved, sender)?;
        guard.live = Some(live);
        guard.started = true;
        Ok(())
    }

    #[allow(clippy::significant_drop_tightening)]
    fn stop(&mut self) -> Result<(), CaptureError> {
        let mut guard = self.state.lock().map_err(|_| {
            CaptureError::Platform(Box::new(std::io::Error::other(
                "AndroidCapture state mutex poisoned",
            )))
        })?;
        guard.started = false;
        guard.sender.take();
        if let Some(mut live) = guard.live.take() {
            live.stop()?;
        }
        Ok(())
    }

    fn frames(&self) -> impl Stream<Item = FrameItem> + Send + 'static {
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

impl AndroidCapture {
    fn start_resolved(
        resolved: Backend,
        _sender: UnboundedSender<FrameItem>,
    ) -> Result<LiveStream, CaptureError> {
        let err = match resolved {
            Backend::MediaProjection => AndroidCaptureError::MediaProjectionDisabled,
            Backend::MicOnly => AndroidCaptureError::MicOnlyDisabled,
            Backend::Auto => AndroidCaptureError::NoBackendAvailable,
        };
        Err(CaptureError::from(err))
    }
}

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

use tokio_stream::wrappers::UnboundedReceiverStream;

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
            source: FrameSource::Mic,
        }
    }

    #[tokio::test]
    async fn test_android_capture_inject_for_test_yields_frames_through_stream() {
        let cap = AndroidCapture::new();
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
    async fn test_android_capture_propagates_capture_errors_via_stream() {
        let cap = AndroidCapture::new();
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
    fn test_android_capture_capabilities_reports_media_projection_permission_model() {
        let cap = AndroidCapture::new();

        let caps = cap.capabilities();

        assert_eq!(caps.permission_model, PermissionModel::MediaProjection);
        assert_eq!(caps.native_sample_rates, vec![48_000]);
        assert!(!caps.supports_per_app_capture);
        // Live binding not yet shipped; the adapter advertises no
        // system-audio support until the JNI binding lands.
        assert!(!caps.supports_system_audio);
    }

    #[test]
    fn test_android_capture_with_backend_records_requested_choice() {
        let cap = AndroidCapture::with_backend(Backend::MediaProjection);

        assert_eq!(cap.requested_backend(), Backend::MediaProjection);
    }

    #[test]
    fn test_android_capture_default_uses_auto_backend() {
        let cap = AndroidCapture::default();

        assert_eq!(cap.requested_backend(), Backend::Auto);
    }

    #[test]
    fn test_android_capture_resolved_backend_returns_none_before_start() {
        let cap = AndroidCapture::new();

        assert_eq!(cap.resolved_backend(), None);
    }

    #[test]
    fn test_android_capture_start_without_live_backend_returns_device_unavailable() {
        let mut cap = AndroidCapture::new();

        let err = cap.start().unwrap_err();

        assert!(matches!(err, CaptureError::DeviceUnavailable(_)));
    }

    #[test]
    fn test_android_capture_explicit_media_projection_without_live_backend_returns_device_unavailable(
    ) {
        let mut cap = AndroidCapture::with_backend(Backend::MediaProjection);

        let err = cap.start().unwrap_err();

        assert!(matches!(err, CaptureError::DeviceUnavailable(_)));
    }

    #[test]
    fn test_android_capture_explicit_mic_only_without_live_backend_returns_device_unavailable() {
        let mut cap = AndroidCapture::with_backend(Backend::MicOnly);

        let err = cap.start().unwrap_err();

        assert!(matches!(err, CaptureError::DeviceUnavailable(_)));
    }

    #[test]
    fn test_android_capture_stop_after_failed_start_is_idempotent() {
        let mut cap = AndroidCapture::new();
        let _ = cap.start();

        cap.stop().unwrap();
        cap.stop().unwrap();
    }

    #[test]
    fn test_android_capture_start_after_stop_returns_device_unavailable_for_reuse() {
        let mut cap = AndroidCapture::new();
        let _ = cap.start();
        cap.stop().unwrap();

        let err = cap.start().unwrap_err();

        assert!(matches!(err, CaptureError::DeviceUnavailable(_)));
    }

    #[tokio::test]
    async fn test_android_capture_second_frames_call_returns_empty_stream() {
        let cap = AndroidCapture::new();

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
