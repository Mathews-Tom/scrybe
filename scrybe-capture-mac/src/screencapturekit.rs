// Copyright 2026 Mathews Tom
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     https://www.apache.org/licenses/LICENSE-2.0
//
//! `ScreenCaptureKit` system-audio adapter for macOS 13.0 and later.
//!
//! `ScreenCaptureKit` still requires a display-backed content filter even when
//! only audio is consumed. The stream therefore asks for a 2×2, 1 fps video
//! surface while registering only an audio output handler. That keeps the
//! required video configuration at a negligible cost while the configuration
//! requests the 16 kHz mono PCM stream consumed by the speech pipeline.

use std::sync::{Arc, Mutex};
use std::time::Instant;

use futures::stream::{self, Stream};
use screencapturekit::prelude::*;
use scrybe_core::capture::AudioCapture;
use scrybe_core::error::CaptureError;
use scrybe_core::types::{AudioFrame, Capabilities, FrameSource, PermissionModel};
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender};

use crate::error::MacCaptureError;
use crate::tokio_stream::wrappers::UnboundedReceiverStream;

const SAMPLE_RATE: u32 = 16_000;
const CHANNELS: u16 = 1;
const SCK_SAMPLE_RATE: i32 = 16_000;
const SCK_CHANNELS: i32 = 1;
const VIDEO_WIDTH: u32 = 2;
const VIDEO_HEIGHT: u32 = 2;
const VIDEO_FPS: u32 = 1;

type FrameItem = Result<AudioFrame, CaptureError>;

type SharedSender = Arc<Mutex<Option<UnboundedSender<FrameItem>>>>;

struct SharedState {
    sender: SharedSender,
    receiver: Option<UnboundedReceiver<FrameItem>>,
    stream: Option<SckStream>,
    started: bool,
}

/// ScreenCaptureKit-backed `AudioCapture` implementation.
///
/// Construct a fresh instance per session. `stop()` closes the frame stream,
/// so a stopped instance deliberately cannot start another session.
pub struct SckCapture {
    state: Arc<Mutex<SharedState>>,
    capabilities: Capabilities,
}

impl SckCapture {
    #[must_use]
    pub fn new() -> Self {
        let (sender, receiver) = unbounded_channel();
        Self {
            state: Arc::new(Mutex::new(SharedState {
                sender: Arc::new(Mutex::new(Some(sender))),
                receiver: Some(receiver),
                stream: None,
                started: false,
            })),
            capabilities: Capabilities {
                supports_system_audio: true,
                supports_per_app_capture: false,
                native_sample_rates: vec![SAMPLE_RATE],
                permission_model: PermissionModel::ScreenRecording,
            },
        }
    }
}

impl Default for SckCapture {
    fn default() -> Self {
        Self::new()
    }
}

impl AudioCapture for SckCapture {
    #[allow(clippy::significant_drop_tightening)]
    fn start(&mut self) -> Result<(), CaptureError> {
        let mut state = self.state.lock().map_err(poisoned_state)?;
        if state.started {
            return Ok(());
        }
        let stream = SckStream::start(&state.sender).map_err(CaptureError::from)?;
        state.stream = Some(stream);
        state.started = true;
        Ok(())
    }

    #[allow(clippy::significant_drop_tightening)]
    fn stop(&mut self) -> Result<(), CaptureError> {
        let mut state = self.state.lock().map_err(poisoned_state)?;
        if let Some(stream) = state.stream.take() {
            stream.stop().map_err(CaptureError::from)?;
        }
        state.started = false;
        drop_sender(&state.sender);
        Ok(())
    }

    fn frames(&self) -> impl Stream<Item = FrameItem> + Send + 'static {
        let mut state = match self.state.lock() {
            Ok(state) => state,
            Err(poisoned) => poisoned.into_inner(),
        };
        state.receiver.take().map_or_else(
            || Box::pin(stream::empty()) as std::pin::Pin<Box<dyn Stream<Item = _> + Send>>,
            |receiver| Box::pin(UnboundedReceiverStream::new(receiver)),
        )
    }

    fn capabilities(&self) -> Capabilities {
        self.capabilities.clone()
    }
}

struct SckStream {
    stream: SCStream,
}

impl SckStream {
    fn start(sender: &SharedSender) -> Result<Self, MacCaptureError> {
        let content = SCShareableContent::get().map_err(screen_capture_error_source)?;
        let display = content.displays().into_iter().next().ok_or_else(|| {
            screen_capture_error("ScreenCaptureKit reported no available displays")
        })?;
        let filter = SCContentFilter::create()
            .with_display(&display)
            .with_excluding_windows(&[])
            .build();
        let configuration = stream_configuration();
        let mut stream = SCStream::new(&filter, &configuration);
        let started_at = Instant::now();
        let handler_sender = Arc::clone(sender);
        let handler_registered = stream.add_output_handler(
            move |sample: CMSampleBuffer, output_type: SCStreamOutputType| {
                if output_type != SCStreamOutputType::Audio {
                    return;
                }
                let Some(buffers) = sample.audio_buffer_list() else {
                    return;
                };
                for buffer in &buffers {
                    let samples = f32_samples(buffer.data());
                    if samples.is_empty() {
                        continue;
                    }
                    let frame = AudioFrame {
                        samples: Arc::from(samples),
                        channels: CHANNELS,
                        sample_rate: SAMPLE_RATE,
                        timestamp_ns: elapsed_ns(started_at),
                        source: FrameSource::System,
                    };
                    send_frame(&handler_sender, frame);
                }
            },
            SCStreamOutputType::Audio,
        );
        if handler_registered.is_none() {
            return Err(screen_capture_error(
                "ScreenCaptureKit rejected audio output-handler registration",
            ));
        }
        stream
            .start_capture()
            .map_err(screen_capture_error_source)?;
        Ok(Self { stream })
    }

    fn stop(&self) -> Result<(), MacCaptureError> {
        self.stream
            .stop_capture()
            .map_err(screen_capture_error_source)
    }
}

fn stream_configuration() -> SCStreamConfiguration {
    SCStreamConfiguration::new()
        .with_width(VIDEO_WIDTH)
        .with_height(VIDEO_HEIGHT)
        .with_fps(VIDEO_FPS)
        .with_captures_audio(true)
        .with_sample_rate(SCK_SAMPLE_RATE)
        .with_channel_count(SCK_CHANNELS)
}

fn send_frame(sender: &SharedSender, frame: AudioFrame) {
    let guard = match sender.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };
    if let Some(sender) = guard.as_ref() {
        let _ = sender.send(Ok(frame));
    }
}

fn drop_sender(sender: &SharedSender) {
    let mut guard = match sender.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };
    guard.take();
}

fn elapsed_ns(started_at: Instant) -> u64 {
    u64::try_from(started_at.elapsed().as_nanos()).unwrap_or(u64::MAX)
}

fn f32_samples(bytes: &[u8]) -> Vec<f32> {
    bytes
        .chunks_exact(std::mem::size_of::<f32>())
        .map(|bytes| f32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
        .collect()
}

fn poisoned_state(
    _: std::sync::PoisonError<std::sync::MutexGuard<'_, SharedState>>,
) -> CaptureError {
    CaptureError::Platform(Box::new(std::io::Error::other(
        "SckCapture state mutex poisoned",
    )))
}

fn screen_capture_error(message: impl Into<String>) -> MacCaptureError {
    MacCaptureError::ScreenCaptureKit(Box::new(std::io::Error::other(message.into())))
}

fn screen_capture_error_source(
    error: impl std::error::Error + Send + Sync + 'static,
) -> MacCaptureError {
    MacCaptureError::ScreenCaptureKit(Box::new(error))
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::float_cmp
)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn test_stream_configuration_requests_minimum_video_and_speech_audio() {
        let configuration = stream_configuration();

        assert_eq!(configuration.width(), VIDEO_WIDTH);
        assert_eq!(configuration.height(), VIDEO_HEIGHT);
        assert_eq!(configuration.fps(), VIDEO_FPS);
        assert!(configuration.captures_audio());
        assert_eq!(configuration.sample_rate(), SCK_SAMPLE_RATE);
        assert_eq!(configuration.channel_count(), SCK_CHANNELS);
    }

    #[test]
    fn test_f32_samples_decodes_little_endian_pcm_and_ignores_partial_tail() {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&0.25_f32.to_le_bytes());
        bytes.extend_from_slice(&(-0.5_f32).to_le_bytes());
        bytes.extend_from_slice(&[0xAA, 0xBB]);

        assert_eq!(f32_samples(&bytes), vec![0.25, -0.5]);
    }

    #[test]
    fn test_sck_capture_capabilities_declare_screen_recording_mono_audio() {
        let capture = SckCapture::new();

        assert_eq!(
            capture.capabilities().permission_model,
            PermissionModel::ScreenRecording
        );
        assert_eq!(
            capture.capabilities().native_sample_rates,
            vec![SAMPLE_RATE]
        );
        assert!(capture.capabilities().supports_system_audio);
        assert!(!capture.capabilities().supports_per_app_capture);
    }
}
