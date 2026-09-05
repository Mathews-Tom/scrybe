// Copyright 2026 Mathews Tom
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     https://www.apache.org/licenses/LICENSE-2.0

//! UID-pinned Core Audio microphone capture.

#![allow(unsafe_code)]

use std::ptr::NonNull;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use block2::RcBlock;
use futures::stream::{self, Stream};
use objc2_core_audio::{
    kAudioDevicePropertyNominalSampleRate, kAudioObjectPropertyElementMain,
    kAudioObjectPropertyScopeGlobal, AudioDeviceCreateIOProcIDWithBlock,
    AudioDeviceDestroyIOProcID, AudioDeviceIOProcID, AudioDeviceStart, AudioDeviceStop,
    AudioObjectGetPropertyData, AudioObjectID, AudioObjectPropertyAddress,
};
use objc2_core_audio_types::{AudioBufferList, AudioTimeStamp};
use scrybe_core::capture::AudioCapture;
use scrybe_core::error::CaptureError;
use scrybe_core::types::{AudioFrame, Capabilities, FrameSource, PermissionModel};
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender};
use tracing::warn;

use crate::coreaudio::{check_status, size_of_u32};
use crate::coreaudio_tap::interleaved_f32_samples;
use crate::error::MacCaptureError;
use crate::input_devices::input_device_id;
use crate::tokio_stream::wrappers::UnboundedReceiverStream;

type FrameSender = UnboundedSender<Result<AudioFrame, CaptureError>>;

type NativeIoBlock = RcBlock<
    dyn Fn(
            NonNull<AudioTimeStamp>,
            NonNull<AudioBufferList>,
            NonNull<AudioTimeStamp>,
            NonNull<AudioBufferList>,
            NonNull<AudioTimeStamp>,
        ) + 'static,
>;
type SharedSender = Arc<Mutex<Option<FrameSender>>>;

struct SharedState {
    sender: Option<FrameSender>,
    receiver: Option<UnboundedReceiver<Result<AudioFrame, CaptureError>>>,
    stream: Option<MicStream>,
    started: bool,
}

/// Microphone capture opened from an exact Core Audio UID.
pub struct NativeMicCapture {
    uid: String,
    state: Arc<Mutex<SharedState>>,
    capabilities: Capabilities,
}

impl NativeMicCapture {
    /// Construct a capture adapter for `uid` returned by [`crate::input_devices`].
    #[must_use]
    pub fn new(uid: String) -> Self {
        let (sender, receiver) = unbounded_channel();
        Self {
            uid,
            state: Arc::new(Mutex::new(SharedState {
                sender: Some(sender),
                receiver: Some(receiver),
                stream: None,
                started: false,
            })),
            capabilities: Capabilities {
                supports_system_audio: false,
                supports_per_app_capture: false,
                native_sample_rates: vec![48_000],
                permission_model: PermissionModel::CoreAudioTap,
            },
        }
    }

    /// The unchanged Core Audio selector this adapter will open.
    #[must_use]
    pub fn uid(&self) -> &str {
        &self.uid
    }
}

impl AudioCapture for NativeMicCapture {
    fn start(&mut self) -> Result<(), CaptureError> {
        let mut state = self.state.lock().map_err(poisoned_state)?;
        if state.started {
            return Ok(());
        }
        let sender = state.sender.clone().ok_or_else(|| {
            CaptureError::PermissionDenied(
                "NativeMicCapture::start called after stop; construct a new capture for a new session"
                    .to_string(),
            )
        })?;
        let mut stream = MicStream::create(&self.uid, sender).map_err(CaptureError::from)?;
        stream.start().map_err(CaptureError::from)?;
        state.stream = Some(stream);
        state.started = true;
        drop(state);
        Ok(())
    }

    fn stop(&mut self) -> Result<(), CaptureError> {
        let mut state = self.state.lock().map_err(poisoned_state)?;
        state.started = false;
        state.sender.take();
        if let Some(mut stream) = state.stream.take() {
            stream.stop().map_err(CaptureError::from)?;
        }
        drop(state);
        Ok(())
    }

    fn frames(&self) -> impl Stream<Item = Result<AudioFrame, CaptureError>> + Send + 'static {
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

struct MicStream {
    device_id: AudioObjectID,
    io_proc_id: AudioDeviceIOProcID,
    sender: SharedSender,
    _block: NativeIoBlock,
    started: bool,
}

// Core Audio owns and invokes the copied block until `Drop` destroys the IO
// proc. All captured state is `Send`; the non-Send marker on `RcBlock` reflects
// Objective-C ownership rather than cross-thread access by this adapter.
#[allow(clippy::non_send_fields_in_send_ty)]
unsafe impl Send for MicStream {}

impl MicStream {
    fn create(uid: &str, sender: FrameSender) -> Result<Self, MacCaptureError> {
        let device_id = input_device_id(uid)?;
        let sample_rate = nominal_sample_rate(device_id)?;
        let shared_sender = Arc::new(Mutex::new(Some(sender)));
        let callback_sender = Arc::clone(&shared_sender);
        let sample_counter = Arc::new(AtomicU64::new(0));
        let callback_counter = Arc::clone(&sample_counter);
        let io_block: NativeIoBlock = RcBlock::new(
            move |_now: NonNull<AudioTimeStamp>,
                  input_data: NonNull<AudioBufferList>,
                  _input_time: NonNull<AudioTimeStamp>,
                  _output_data: NonNull<AudioBufferList>,
                  _output_time: NonNull<AudioTimeStamp>| {
                // SAFETY: Core Audio owns this input buffer list for the
                // callback duration. The helper copies its samples.
                let buffers = unsafe { input_data.as_ref() };
                let samples = unsafe { interleaved_f32_samples(buffers) };
                if samples.is_empty() {
                    return;
                }
                let channels = input_channels(buffers).unwrap_or(1);
                let frames = u64::try_from(samples.len()).unwrap_or(u64::MAX) / u64::from(channels);
                let prior = callback_counter.fetch_add(frames, Ordering::Relaxed);
                let timestamp_ns = prior
                    .saturating_mul(1_000_000_000)
                    .checked_div(u64::from(sample_rate))
                    .unwrap_or(0);
                let frame = AudioFrame::from_slice(
                    &samples,
                    channels,
                    sample_rate,
                    timestamp_ns,
                    FrameSource::Mic,
                );
                if let Ok(guard) = callback_sender.lock() {
                    if let Some(sender) = guard.as_ref() {
                        let _ = sender.send(Ok(frame));
                    }
                } else {
                    tracing::error!("native microphone sender mutex poisoned");
                }
            },
        );
        let mut io_proc_id = None;
        let status = unsafe {
            AudioDeviceCreateIOProcIDWithBlock(
                NonNull::from(&mut io_proc_id),
                device_id,
                None,
                RcBlock::as_ptr(&io_block),
            )
        };
        check_status(
            status,
            "AudioDeviceCreateIOProcIDWithBlock(native microphone)",
        )?;
        Ok(Self {
            device_id,
            io_proc_id,
            sender: shared_sender,
            _block: io_block,
            started: false,
        })
    }

    fn start(&mut self) -> Result<(), MacCaptureError> {
        if self.started {
            return Ok(());
        }
        check_status(
            unsafe { AudioDeviceStart(self.device_id, self.io_proc_id) },
            "AudioDeviceStart(native microphone)",
        )?;
        self.started = true;
        Ok(())
    }

    fn stop(&mut self) -> Result<(), MacCaptureError> {
        if self.started {
            self.started = false;
            check_status(
                unsafe { AudioDeviceStop(self.device_id, self.io_proc_id) },
                "AudioDeviceStop(native microphone)",
            )?;
        }
        drop_sender(&self.sender);
        Ok(())
    }
}

impl Drop for MicStream {
    fn drop(&mut self) {
        if self.started {
            let status = unsafe { AudioDeviceStop(self.device_id, self.io_proc_id) };
            if status != 0 {
                warn!(
                    status,
                    "AudioDeviceStop returned non-zero status during native microphone drop"
                );
            }
        }
        let status = unsafe { AudioDeviceDestroyIOProcID(self.device_id, self.io_proc_id) };
        if status != 0 {
            warn!(
                status,
                "AudioDeviceDestroyIOProcID returned non-zero status during native microphone drop"
            );
        }
    }
}
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn nominal_sample_rate(device_id: AudioObjectID) -> Result<u32, MacCaptureError> {
    let address = AudioObjectPropertyAddress {
        mSelector: kAudioDevicePropertyNominalSampleRate,
        mScope: kAudioObjectPropertyScopeGlobal,
        mElement: kAudioObjectPropertyElementMain,
    };
    let mut rate = 0.0_f64;
    let mut size = size_of_u32::<f64>("nominal sample rate")?;
    let status = unsafe {
        AudioObjectGetPropertyData(
            device_id,
            NonNull::from(&address),
            0,
            std::ptr::null(),
            NonNull::from(&mut size),
            NonNull::from(&mut rate).cast(),
        )
    };
    check_status(status, "AudioObjectGetPropertyData(NominalSampleRate)")?;
    if !(1.0..=f64::from(u32::MAX)).contains(&rate) {
        return Err(MacCaptureError::CoreAudio(format!(
            "invalid native microphone sample rate {rate}"
        )));
    }
    Ok(rate.round() as u32)
}

fn input_channels(buffers: &AudioBufferList) -> Result<u16, MacCaptureError> {
    let raw = buffers.mBuffers.as_ptr();
    let count = buffers.mNumberBuffers as usize;
    let channels = (0..count).try_fold(0_u32, |total, index| {
        let buffer = unsafe { raw.add(index).read() };
        total.checked_add(buffer.mNumberChannels).ok_or_else(|| {
            MacCaptureError::CoreAudio("native microphone channel count overflow".to_string())
        })
    })?;
    u16::try_from(channels).map_err(|_| {
        MacCaptureError::CoreAudio(format!(
            "native microphone has unsupported {channels} channels"
        ))
    })
}

fn drop_sender(sender: &SharedSender) {
    let mut guard = match sender.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };
    guard.take();
}

fn poisoned_state<T>(_error: std::sync::PoisonError<T>) -> CaptureError {
    CaptureError::Platform(Box::new(std::io::Error::other(
        "NativeMicCapture state mutex poisoned",
    )))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn native_capture_retains_the_exact_uid() {
        let capture = NativeMicCapture::new("BuiltInMicrophoneDevice".to_string());
        assert_eq!(capture.uid(), "BuiltInMicrophoneDevice");
    }
}
