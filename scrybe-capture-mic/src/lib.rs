// Copyright 2026 Mathews Tom
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     https://www.apache.org/licenses/LICENSE-2.0

//! Cross-platform microphone capture adapter implementing
//! `scrybe_core::capture::AudioCapture` via cpal.
//!
//! Closes the v0.1 mic-only path documented in
//! `.docs/development-plan.md` §7.2 ("scrybe record — start session;
//! press hotkey or --title flag; mic-only capture; live append to
//! transcript.md; run Whisper after each chunk") that shipped under
//! the synthetic 440 Hz sine generator and stub providers through v1.0.
//!
//! The crate uses cpal because cpal already wraps each platform's
//! input-capture API (`CoreAudio` on macOS, ALSA/JACK on Linux, WASAPI
//! on Windows) and the v1.0 mic-only path does not need anything
//! beyond default-input-device. The system-audio path on each
//! platform continues to use the dedicated adapter
//! (`scrybe-capture-mac` for Core Audio Taps, `scrybe-capture-linux`
//! for `PipeWire`, `scrybe-capture-win` for WASAPI loopback).
//!
//! Without the `live-mic` feature, `MicCapture::start()` returns
//! `CaptureError::PermissionDenied("live-mic feature disabled ...")`
//! so non-feature-gated builds (cross-platform CI, hosts without
//! ALSA / `CoreAudio` / WASAPI headers) can still link this crate.

pub mod error;

pub use error::MicCaptureError;

use std::sync::{Arc, Mutex};

use futures::stream::{self, Stream};
use scrybe_core::capture::AudioCapture;
use scrybe_core::error::CaptureError;
use scrybe_core::types::{AudioFrame, Capabilities, PermissionModel};
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender};

mod tokio_stream {
    pub mod wrappers {
        use std::pin::Pin;
        use std::task::{Context, Poll};

        use futures::stream::Stream;
        use tokio::sync::mpsc::UnboundedReceiver;

        /// In-tree replacement for `tokio_stream::wrappers::UnboundedReceiverStream`
        /// so this adapter does not depend on the external `tokio-stream`
        /// crate. Mirrors the same shim already vendored in
        /// `scrybe-capture-mac` and `scrybe-capture-linux`.
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

/// State shared between `MicCapture` and the dedicated capture thread.
struct SharedState {
    sender: Option<UnboundedSender<Result<AudioFrame, CaptureError>>>,
    receiver: Option<UnboundedReceiver<Result<AudioFrame, CaptureError>>>,
    started: bool,
    /// Signal channel: dropping the sender end signals the dedicated
    /// capture thread to drop its `cpal::Stream` and exit. The cpal
    /// stream is `!Send` on most platforms so it cannot be stored in
    /// `SharedState` directly.
    #[cfg(feature = "live-mic")]
    stop_tx: Option<std::sync::mpsc::Sender<()>>,
    #[cfg(feature = "live-mic")]
    thread_handle: Option<std::thread::JoinHandle<()>>,
}

/// Microphone-only `AudioCapture` adapter.
pub struct MicCapture {
    state: Arc<Mutex<SharedState>>,
    capabilities: Capabilities,
}

impl MicCapture {
    /// Construct an adapter targeting the host's default input device.
    /// Capabilities advertise mic-only — `supports_system_audio` is
    /// false; the system-audio path lives in the per-platform adapter
    /// (`scrybe-capture-mac`, `scrybe-capture-linux`, `scrybe-capture-win`).
    #[must_use]
    pub fn new() -> Self {
        let (tx, rx) = unbounded_channel();
        let state = Arc::new(Mutex::new(SharedState {
            sender: Some(tx),
            receiver: Some(rx),
            started: false,
            #[cfg(feature = "live-mic")]
            stop_tx: None,
            #[cfg(feature = "live-mic")]
            thread_handle: None,
        }));
        let capabilities = Capabilities {
            supports_system_audio: false,
            supports_per_app_capture: false,
            // cpal negotiates the device's native rate at start-time; the
            // pipeline resamples to STT_SAMPLE_RATE (16 kHz) so the
            // declared list is informational. 48 kHz is the modal native
            // rate across CoreAudio / ALSA / WASAPI default inputs.
            native_sample_rates: vec![48_000],
            permission_model: default_permission_model(),
        };
        Self {
            state,
            capabilities,
        }
    }

    /// Inject a frame for tests. Synchronous; never used in production.
    /// Public so integration tests in `scrybe-cli` can drive the adapter
    /// through the same surface the cpal callback uses.
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
    /// dedicated thread's "stream ended" signal.
    pub fn close_for_test(&self) {
        let mut guard = match self.state.lock() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        };
        guard.sender.take();
    }
}

impl Default for MicCapture {
    fn default() -> Self {
        Self::new()
    }
}

/// `PermissionModel` is Tier-1 frozen at v1.0 (`docs/system-design.md`
/// §12.1) and currently has no `Microphone` variant. The closest
/// existing variant per platform is reused; the `permission_model`
/// field is informational (drives onboarding copy) and does not gate
/// behavior. A future `PermissionModel::Microphone` variant is a v2.0
/// breaking-change candidate.
const fn default_permission_model() -> PermissionModel {
    #[cfg(target_os = "macos")]
    {
        PermissionModel::CoreAudioTap
    }
    #[cfg(target_os = "windows")]
    {
        PermissionModel::WasapiLoopback
    }
    #[cfg(target_os = "linux")]
    {
        PermissionModel::PipeWirePortal
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
    {
        PermissionModel::CoreAudioTap
    }
}

impl AudioCapture for MicCapture {
    #[allow(
        unused_mut,
        unused_variables,
        clippy::significant_drop_tightening,
        clippy::needless_return
    )]
    fn start(&mut self) -> Result<(), CaptureError> {
        let mut guard = self.state.lock().map_err(|_| {
            CaptureError::Platform(Box::new(std::io::Error::other(
                "MicCapture state mutex poisoned",
            )))
        })?;
        if guard.started {
            return Ok(());
        }
        let sender = guard.sender.clone().ok_or_else(|| {
            CaptureError::PermissionDenied(
                "MicCapture::start called after stop; construct a new \
                 MicCapture for a new session"
                    .to_string(),
            )
        })?;

        #[cfg(feature = "live-mic")]
        {
            let (stop_tx, stop_rx) = std::sync::mpsc::channel::<()>();
            let handle = std::thread::Builder::new()
                .name("scrybe-mic-capture".to_string())
                .spawn(move || {
                    live_mic::run_capture_thread(&sender, &stop_rx);
                })
                .map_err(|e| {
                    CaptureError::Platform(Box::new(std::io::Error::other(format!(
                        "failed to spawn scrybe-mic-capture thread: {e}"
                    ))))
                })?;
            guard.stop_tx = Some(stop_tx);
            guard.thread_handle = Some(handle);
            guard.started = true;
            Ok(())
        }
        #[cfg(not(feature = "live-mic"))]
        {
            let _ = sender;
            Err(CaptureError::from(MicCaptureError::FeatureDisabled))
        }
    }

    #[allow(clippy::significant_drop_tightening)]
    fn stop(&mut self) -> Result<(), CaptureError> {
        let handle_to_join: Option<std::thread::JoinHandle<()>>;
        {
            let mut guard = self.state.lock().map_err(|_| {
                CaptureError::Platform(Box::new(std::io::Error::other(
                    "MicCapture state mutex poisoned",
                )))
            })?;
            guard.started = false;
            guard.sender.take();
            #[cfg(feature = "live-mic")]
            {
                guard.stop_tx.take();
                handle_to_join = guard.thread_handle.take();
            }
            #[cfg(not(feature = "live-mic"))]
            {
                handle_to_join = None;
            }
        }
        if let Some(handle) = handle_to_join {
            let _ = handle.join();
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

#[cfg(feature = "live-mic")]
mod live_mic {
    use std::sync::mpsc::{Receiver, RecvTimeoutError};
    use std::time::{Duration, Instant};

    use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
    use cpal::{SampleFormat, StreamConfig};
    use scrybe_core::error::CaptureError;
    use scrybe_core::types::{AudioFrame, FrameSource};
    use tokio::sync::mpsc::UnboundedSender;
    use tracing::warn;

    use crate::error::MicCaptureError;

    type FrameSender = UnboundedSender<Result<AudioFrame, CaptureError>>;

    /// Owns the `cpal::Stream` for the lifetime of the capture session.
    /// Runs on a dedicated OS thread because `cpal::Stream` is `!Send`
    /// on every platform we support.
    pub fn run_capture_thread(sender: &FrameSender, stop_rx: &Receiver<()>) {
        let host = cpal::default_host();
        let Some(device) = host.default_input_device() else {
            let _ = sender.send(Err(MicCaptureError::NoDefaultInputDevice.into()));
            return;
        };
        let supported = match device.default_input_config() {
            Ok(c) => c,
            Err(e) => {
                let _ = sender.send(Err(MicCaptureError::Cpal(e.to_string()).into()));
                return;
            }
        };

        let sample_format = supported.sample_format();
        let sample_rate = supported.sample_rate().0;
        let channels = supported.channels();
        let stream_config: StreamConfig = supported.into();
        let started = Instant::now();

        let stream_result = match sample_format {
            SampleFormat::F32 => build_stream::<f32>(
                &device,
                &stream_config,
                sample_rate,
                channels,
                started,
                sender.clone(),
            ),
            SampleFormat::I16 => build_stream::<i16>(
                &device,
                &stream_config,
                sample_rate,
                channels,
                started,
                sender.clone(),
            ),
            SampleFormat::U16 => build_stream::<u16>(
                &device,
                &stream_config,
                sample_rate,
                channels,
                started,
                sender.clone(),
            ),
            other => {
                let _ = sender.send(Err(MicCaptureError::Cpal(format!(
                    "unsupported cpal sample format: {other:?}"
                ))
                .into()));
                return;
            }
        };

        let stream = match stream_result {
            Ok(s) => s,
            Err(e) => {
                let _ = sender.send(Err(MicCaptureError::Cpal(e.to_string()).into()));
                return;
            }
        };

        if let Err(e) = stream.play() {
            let _ = sender.send(Err(MicCaptureError::Cpal(e.to_string()).into()));
            return;
        }

        // Block until the stop signal arrives or the parent drops the
        // sender end. Polling timeout keeps the thread responsive to
        // ctrl-c shutdown via SharedState::stop_tx being dropped.
        loop {
            match stop_rx.recv_timeout(Duration::from_millis(250)) {
                Ok(()) | Err(RecvTimeoutError::Disconnected) => break,
                Err(RecvTimeoutError::Timeout) => {}
            }
        }
        // `stream` drops here, which tears down the cpal callback.
        drop(stream);
    }

    fn build_stream<T>(
        device: &cpal::Device,
        config: &StreamConfig,
        sample_rate: u32,
        channels: u16,
        started: Instant,
        sender: FrameSender,
    ) -> Result<cpal::Stream, cpal::BuildStreamError>
    where
        T: cpal::SizedSample + cpal::Sample<Float = f32>,
        f32: cpal::FromSample<T>,
    {
        let err_sender = sender.clone();
        device.build_input_stream(
            config,
            move |data: &[T], _info: &cpal::InputCallbackInfo| {
                let samples: Vec<f32> = data
                    .iter()
                    .map(|s| <f32 as cpal::FromSample<T>>::from_sample_(*s))
                    .collect();
                let timestamp_ns = u64::try_from(started.elapsed().as_nanos()).unwrap_or(u64::MAX);
                let frame = AudioFrame {
                    samples: std::sync::Arc::from(samples),
                    channels,
                    sample_rate,
                    timestamp_ns,
                    source: FrameSource::Mic,
                };
                if sender.send(Ok(frame)).is_err() {
                    // Receiver dropped — pipeline tore down. Subsequent
                    // callback invocations no-op until the stream is
                    // dropped by the capture thread.
                }
            },
            move |err| {
                warn!(target: "scrybe_capture_mic", error = %err, "cpal input stream error");
                let _ = err_sender.send(Err(CaptureError::Platform(Box::new(
                    std::io::Error::other(err.to_string()),
                ))));
            },
            None,
        )
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use std::sync::Arc;

    use futures::stream::StreamExt;
    use scrybe_core::types::FrameSource;

    use super::*;

    fn sample_frame() -> AudioFrame {
        AudioFrame {
            samples: Arc::from(vec![0.1_f32; 16]),
            channels: 1,
            sample_rate: 16_000,
            timestamp_ns: 0,
            source: FrameSource::Mic,
        }
    }

    #[test]
    fn test_capabilities_advertise_mic_only_path() {
        let cap = MicCapture::new();
        let caps = cap.capabilities();
        assert!(!caps.supports_system_audio);
        assert!(!caps.supports_per_app_capture);
        assert!(!caps.native_sample_rates.is_empty());
    }

    #[test]
    fn test_default_constructor_matches_new() {
        let a = MicCapture::default();
        let b = MicCapture::new();
        assert_eq!(a.capabilities(), b.capabilities());
    }

    #[tokio::test]
    async fn test_inject_for_test_round_trips_through_frames_stream() {
        let cap = MicCapture::new();
        cap.inject_for_test(Ok(sample_frame()));
        cap.close_for_test();
        let mut s = cap.frames();
        let item = s.next().await.expect("first frame");
        let frame = item.expect("ok frame");
        assert_eq!(frame.source, FrameSource::Mic);
        assert_eq!(frame.samples.len(), 16);
        // Stream closes after sender drops.
        assert!(s.next().await.is_none());
    }

    #[tokio::test]
    async fn test_frames_returns_empty_after_second_call() {
        // Mirror MacCapture's single-consumer guarantee: the receiver
        // is consumed on the first frames() call; a second call yields
        // an empty stream rather than panicking.
        let cap = MicCapture::new();
        let _ = cap.frames();
        let mut s2 = cap.frames();
        assert!(s2.next().await.is_none());
    }

    #[cfg(not(feature = "live-mic"))]
    #[test]
    fn test_start_returns_permission_denied_without_feature() {
        let mut cap = MicCapture::new();
        let err = cap.start().expect_err("feature-gated start must fail");
        match err {
            CaptureError::PermissionDenied(msg) => {
                assert!(msg.contains("live-mic feature disabled"));
            }
            other => panic!("expected PermissionDenied, got {other:?}"),
        }
    }

    #[cfg(not(feature = "live-mic"))]
    #[test]
    fn test_stop_is_idempotent_without_feature() {
        let mut cap = MicCapture::new();
        cap.stop().expect("first stop");
        cap.stop().expect("second stop");
    }
}
