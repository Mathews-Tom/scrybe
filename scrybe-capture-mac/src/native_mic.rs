// Copyright 2026 Mathews Tom
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     https://www.apache.org/licenses/LICENSE-2.0

//! UID-pinned Core Audio microphone capture, with an optional
//! `VoiceProcessingIO` echo-cancellation path.
//!
//! [`NativeMicCapture::start`] opens the ordinary [`MicStream`] by
//! default. When constructed with `aec = true` (mirrors
//! `[record].aec`), it first attempts a [`VoiceProcessingStream`] —
//! Apple's built-in AEC/NS voice-processing `AudioUnit` bound to the
//! same exact device UID — and falls back to the ordinary stream on
//! any creation, configuration, initialization, or start failure. The
//! plain stream's success is `start`'s success. Once a stream is
//! running, its own runtime errors (e.g. a failing `AudioUnitRender`)
//! surface through the frame channel rather than triggering a silent
//! mid-session fallback.

#![allow(unsafe_code)]

use std::ffi::c_void;
use std::ptr::NonNull;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use block2::RcBlock;
use futures::stream::{self, Stream};
use objc2_audio_toolbox::{
    kAUVoiceIOProperty_BypassVoiceProcessing, kAUVoiceIOProperty_OtherAudioDuckingConfiguration,
    kAudioOutputUnitProperty_CurrentDevice, kAudioOutputUnitProperty_EnableIO,
    kAudioOutputUnitProperty_SetInputCallback, kAudioUnitManufacturer_Apple,
    kAudioUnitProperty_StreamFormat, kAudioUnitScope_Global, kAudioUnitScope_Input,
    kAudioUnitScope_Output, kAudioUnitSubType_VoiceProcessingIO, kAudioUnitType_Output,
    AURenderCallbackStruct, AUVoiceIOOtherAudioDuckingConfiguration,
    AUVoiceIOOtherAudioDuckingLevel, AudioComponentDescription, AudioComponentFindNext,
    AudioComponentInstance, AudioComponentInstanceDispose, AudioComponentInstanceNew,
    AudioOutputUnitStart, AudioOutputUnitStop, AudioUnit, AudioUnitInitialize, AudioUnitRender,
    AudioUnitRenderActionFlags, AudioUnitSetProperty, AudioUnitUninitialize,
};
use objc2_core_audio::{
    kAudioDevicePropertyNominalSampleRate, kAudioObjectPropertyElementMain,
    kAudioObjectPropertyScopeGlobal, AudioDeviceCreateIOProcIDWithBlock,
    AudioDeviceDestroyIOProcID, AudioDeviceIOProcID, AudioDeviceStart, AudioDeviceStop,
    AudioObjectGetPropertyData, AudioObjectID, AudioObjectPropertyAddress,
};
use objc2_core_audio_types::{
    kAudioFormatFlagIsFloat, kAudioFormatFlagIsPacked, kAudioFormatLinearPCM, AudioBuffer,
    AudioBufferList, AudioStreamBasicDescription, AudioTimeStamp,
};
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
    stream: Option<NativeStream>,
    started: bool,
}

/// Microphone capture opened from an exact Core Audio UID.
pub struct NativeMicCapture {
    uid: String,
    aec: bool,
    state: Arc<Mutex<SharedState>>,
    capabilities: Capabilities,
}

impl NativeMicCapture {
    /// Construct a capture adapter for `uid` returned by
    /// [`crate::input_devices`]. `aec` mirrors `[record].aec`: when
    /// `true`, `start` attempts a
    /// `VoiceProcessingIO` stream before falling back to the ordinary
    /// microphone stream on any failure.
    #[must_use]
    pub fn new(uid: String, aec: bool) -> Self {
        let (sender, receiver) = unbounded_channel();
        Self {
            uid,
            aec,
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
        let voice_sender = sender.clone();
        let uid = &self.uid;
        let outcome = acquire_stream(
            self.aec,
            || {
                let mut voice = VoiceProcessingStream::create(uid, voice_sender)?;
                voice.start()?;
                Ok(voice)
            },
            || {
                let mut plain = MicStream::create(uid, sender)?;
                plain.start()?;
                Ok(plain)
            },
        )
        .map_err(CaptureError::from)?;
        state.stream = Some(match outcome {
            Attempt::Voice(voice) => NativeStream::Voice(voice),
            Attempt::Plain(plain) => NativeStream::Plain(plain),
        });
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

/// Which concrete stream backs a running [`NativeMicCapture`].
enum NativeStream {
    Voice(VoiceProcessingStream),
    Plain(MicStream),
}

impl NativeStream {
    fn stop(&mut self) -> Result<(), MacCaptureError> {
        match self {
            Self::Voice(stream) => stream.stop(),
            Self::Plain(stream) => stream.stop(),
        }
    }
}

/// Which attempt in [`acquire_stream`] ultimately produced the
/// running stream.
#[derive(Debug)]
enum Attempt<V, P> {
    Voice(V),
    Plain(P),
}

/// Attempts a `VoiceProcessingIO` stream when `aec` is set, falling
/// back to the ordinary microphone stream on any failure — creation,
/// configuration, initialization, or start. The plain stream's
/// success is this function's success; a failed voice attempt is
/// never retried, and the voice path is never attempted at all when
/// `aec` is `false`.
///
/// Generic over the two attempt closures (rather than concrete
/// [`VoiceProcessingStream`]/[`MicStream`] types) so unit tests can
/// force the voice path to fail — or confirm it was never called —
/// without touching real Core Audio hardware.
fn acquire_stream<V, P>(
    aec: bool,
    try_voice: impl FnOnce() -> Result<V, MacCaptureError>,
    try_plain: impl FnOnce() -> Result<P, MacCaptureError>,
) -> Result<Attempt<V, P>, MacCaptureError> {
    if aec {
        match try_voice() {
            Ok(stream) => return Ok(Attempt::Voice(stream)),
            Err(error) => {
                warn!(
                    error = %error,
                    "VoiceProcessingIO capture failed to start; falling back to ordinary microphone capture"
                );
            }
        }
    }
    try_plain().map(Attempt::Plain)
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

/// `VoiceProcessingIO` capture channel count. Mono matches Apple's
/// primary validated use case for the unit's AEC/NS quality; the unit
/// itself performs any downmix/rate conversion needed from the bound
/// device's native format to this client format.
const VOICE_PROCESSING_CHANNELS: u16 = 1;

/// Live `VoiceProcessingIO` capture stream: Apple's built-in AEC/NS
/// audio unit bound to an exact Core Audio device, alongside
/// [`MicStream`]. Used only when `[record].aec` is `true`; see
/// [`NativeMicCapture::start`] and [`acquire_stream`] for the
/// create/configure/initialize/start failure-fallback contract.
struct VoiceProcessingStream {
    unit: AudioUnit,
    initialized: bool,
    started: bool,
    sender: SharedSender,
    /// Raw pointer to the boxed [`VoiceCallbackContext`]; owned by
    /// this stream and reclaimed exactly once in `Drop`.
    context: NonNull<VoiceCallbackContext>,
}

// CoreAudio owns and invokes the render callback via the `context`
// pointer (transferred out of a `Box` via `Box::into_raw`, reclaimed
// via `Box::from_raw` in `Drop`) until `Drop` disposes the unit. All
// state behind it is `Send`; the raw `AudioUnit`/`NonNull` pointers
// reflect FFI ownership rather than genuine cross-thread access by
// this adapter — the same rationale `MicStream` documents for
// `NativeIoBlock`.
#[allow(clippy::non_send_fields_in_send_ty)]
unsafe impl Send for VoiceProcessingStream {}

impl VoiceProcessingStream {
    /// Finds, instantiates, binds, configures, and fully initializes a
    /// `VoiceProcessingIO` audio unit for `uid` — everything short of
    /// starting it. `self` is also this function's teardown guard:
    /// after component creation succeeds, any early `?` return leaves
    /// the partially-built value to go out of scope, which `Drop`
    /// tears down exactly once (uninitializing only if
    /// `AudioUnitInitialize` had already succeeded, then always
    /// disposing the component and reclaiming the boxed callback
    /// context) — mirroring the runtime probe's `InstanceGuard`.
    fn create(uid: &str, sender: FrameSender) -> Result<Self, MacCaptureError> {
        let device_id = input_device_id(uid)?;
        let sample_rate = nominal_sample_rate(device_id)?;

        let description = AudioComponentDescription {
            componentType: kAudioUnitType_Output,
            componentSubType: kAudioUnitSubType_VoiceProcessingIO,
            componentManufacturer: kAudioUnitManufacturer_Apple,
            componentFlags: 0,
            componentFlagsMask: 0,
        };
        // SAFETY: `description` is a valid, fully-initialized,
        // stack-local `AudioComponentDescription`. A null
        // `in_component` starts the search from the beginning of the
        // component registry — identical to the runtime probe.
        let found =
            unsafe { AudioComponentFindNext(std::ptr::null_mut(), NonNull::from(&description)) };
        let component = NonNull::new(found).ok_or_else(|| {
            MacCaptureError::CoreAudio("VoiceProcessingIO component not found".to_string())
        })?;

        let mut raw_instance: AudioComponentInstance = std::ptr::null_mut();
        // SAFETY: `component` was just returned by
        // `AudioComponentFindNext` and is non-null; `raw_instance` is
        // a valid local out-pointer.
        let new_status = unsafe {
            AudioComponentInstanceNew(component.as_ptr(), NonNull::from(&mut raw_instance))
        };
        check_status(new_status, "AudioComponentInstanceNew(VoiceProcessingIO)")?;
        let instance = NonNull::new(raw_instance).ok_or_else(|| {
            MacCaptureError::CoreAudio(
                "AudioComponentInstanceNew reported noErr but returned a null instance".to_string(),
            )
        })?;
        let unit = instance.as_ptr();

        let context = Box::new(VoiceCallbackContext {
            sender: Arc::new(Mutex::new(Some(sender))),
            unit,
            channels: VOICE_PROCESSING_CHANNELS,
            sample_rate,
            sample_counter: AtomicU64::new(0),
            scratch: Mutex::new(Vec::new()),
        });
        let sender_handle = Arc::clone(&context.sender);
        let raw_context: *mut VoiceCallbackContext = Box::into_raw(context);
        // SAFETY: `Box::into_raw` never returns a null pointer — the
        // `Box` it consumed could not have held one.
        let context = unsafe { NonNull::new_unchecked(raw_context) };

        let mut stream = Self {
            unit,
            initialized: false,
            started: false,
            sender: sender_handle,
            context,
        };

        stream.configure(device_id, sample_rate)?;
        // SAFETY: `stream.unit` is the live, fully-configured instance
        // created above and has not yet been initialized or disposed.
        check_status(
            unsafe { AudioUnitInitialize(stream.unit) },
            "AudioUnitInitialize(VoiceProcessingIO)",
        )?;
        stream.initialized = true;

        Ok(stream)
    }

    /// Binds `device_id`, the client-side stream format, the input
    /// callback, voice-processing bypass, and other-audio ducking.
    /// Every property here must be set before `AudioUnitInitialize`.
    fn configure(&self, device_id: AudioObjectID, sample_rate: u32) -> Result<(), MacCaptureError> {
        let enable_input: u32 = 1;
        set_unit_property(
            self.unit,
            kAudioOutputUnitProperty_EnableIO,
            kAudioUnitScope_Input,
            1,
            &enable_input,
            "AudioUnitSetProperty(EnableIO input)",
        )?;
        let disable_output: u32 = 0;
        set_unit_property(
            self.unit,
            kAudioOutputUnitProperty_EnableIO,
            kAudioUnitScope_Output,
            0,
            &disable_output,
            "AudioUnitSetProperty(EnableIO output)",
        )?;
        set_unit_property(
            self.unit,
            kAudioOutputUnitProperty_CurrentDevice,
            kAudioUnitScope_Global,
            0,
            &device_id,
            "AudioUnitSetProperty(CurrentDevice)",
        )?;

        let bytes_per_frame = u32::from(VOICE_PROCESSING_CHANNELS) * 4;
        let format = AudioStreamBasicDescription {
            mSampleRate: f64::from(sample_rate),
            mFormatID: kAudioFormatLinearPCM,
            mFormatFlags: kAudioFormatFlagIsFloat | kAudioFormatFlagIsPacked,
            mBytesPerPacket: bytes_per_frame,
            mFramesPerPacket: 1,
            mBytesPerFrame: bytes_per_frame,
            mChannelsPerFrame: u32::from(VOICE_PROCESSING_CHANNELS),
            mBitsPerChannel: 32,
            mReserved: 0,
        };
        set_unit_property(
            self.unit,
            kAudioUnitProperty_StreamFormat,
            kAudioUnitScope_Output,
            1,
            &format,
            "AudioUnitSetProperty(StreamFormat)",
        )?;

        let bypass_voice_processing: u32 = 0;
        set_unit_property(
            self.unit,
            kAUVoiceIOProperty_BypassVoiceProcessing,
            kAudioUnitScope_Global,
            0,
            &bypass_voice_processing,
            "AudioUnitSetProperty(BypassVoiceProcessing)",
        )?;

        let ducking = ducking_configuration();
        set_unit_property(
            self.unit,
            kAUVoiceIOProperty_OtherAudioDuckingConfiguration,
            kAudioUnitScope_Global,
            0,
            &ducking,
            "AudioUnitSetProperty(OtherAudioDuckingConfiguration)",
        )?;

        let callback = AURenderCallbackStruct {
            inputProc: Some(voice_processing_input_callback),
            inputProcRefCon: self.context.as_ptr().cast::<c_void>(),
        };
        set_unit_property(
            self.unit,
            kAudioOutputUnitProperty_SetInputCallback,
            kAudioUnitScope_Global,
            0,
            &callback,
            "AudioUnitSetProperty(SetInputCallback)",
        )?;

        Ok(())
    }

    fn start(&mut self) -> Result<(), MacCaptureError> {
        if self.started {
            return Ok(());
        }
        // SAFETY: `self.unit` was initialized by `create` and has not
        // yet been started.
        check_status(
            unsafe { AudioOutputUnitStart(self.unit) },
            "AudioOutputUnitStart(VoiceProcessingIO)",
        )?;
        self.started = true;
        Ok(())
    }

    fn stop(&mut self) -> Result<(), MacCaptureError> {
        if self.started {
            self.started = false;
            // SAFETY: `self.unit` was started by `start` and has not
            // been disposed.
            check_status(
                unsafe { AudioOutputUnitStop(self.unit) },
                "AudioOutputUnitStop(VoiceProcessingIO)",
            )?;
        }
        drop_sender(&self.sender);
        Ok(())
    }
}

impl Drop for VoiceProcessingStream {
    fn drop(&mut self) {
        if self.started {
            // SAFETY: best-effort teardown; `self.unit` was started by
            // `start`. There is no further path to surface a nonzero
            // status from `Drop`.
            let status = unsafe { AudioOutputUnitStop(self.unit) };
            if status != 0 {
                warn!(
                    status,
                    "AudioOutputUnitStop returned non-zero status during VoiceProcessingIO drop"
                );
            }
        }
        if self.initialized {
            // SAFETY: `self.unit` was successfully initialized by
            // `create` and has not been disposed.
            let status = unsafe { AudioUnitUninitialize(self.unit) };
            if status != 0 {
                warn!(
                    status,
                    "AudioUnitUninitialize returned non-zero status during VoiceProcessingIO drop"
                );
            }
        }
        // SAFETY: `self.unit` was returned by a successful
        // `AudioComponentInstanceNew` call in `create` and is disposed
        // exactly once, here.
        let status = unsafe { AudioComponentInstanceDispose(self.unit) };
        if status != 0 {
            warn!(
                status,
                "AudioComponentInstanceDispose returned non-zero status during VoiceProcessingIO drop"
            );
        }
        // SAFETY: `self.context` was produced by `Box::into_raw` in
        // `create` and has not been freed; `VoiceProcessingStream` is
        // its sole owner and `Drop` runs exactly once, after the
        // component above is fully disposed so CoreAudio can no
        // longer invoke the callback that reads it.
        drop(unsafe { Box::from_raw(self.context.as_ptr()) });
    }
}

/// State shared between [`VoiceProcessingStream`] and its render
/// callback. Owned via `Box::into_raw` for the unit's lifetime;
/// reclaimed exactly once by [`VoiceProcessingStream`]'s `Drop`.
struct VoiceCallbackContext {
    sender: SharedSender,
    unit: AudioUnit,
    channels: u16,
    sample_rate: u32,
    sample_counter: AtomicU64,
    /// Render scratch buffer, grown lazily to fit the largest
    /// `inNumberFrames` Core Audio has requested so far. Guarded by a
    /// `Mutex` rather than sized upfront via
    /// `kAudioUnitProperty_MaximumFramesPerSlice`: Core Audio invokes
    /// this unit's render callback strictly serially, so the lock is
    /// uncontended, and the actual `inNumberFrames` value is always
    /// authoritative regardless of what that property reports.
    scratch: Mutex<Vec<f32>>,
}

/// The exact other-audio ducking configuration
/// [`VoiceProcessingStream::configure`] applies: advanced ducking
/// disabled, default ducking level. Extracted as a pure function so
/// its field values are independently testable without touching Core
/// Audio.
const fn ducking_configuration() -> AUVoiceIOOtherAudioDuckingConfiguration {
    AUVoiceIOOtherAudioDuckingConfiguration {
        mEnableAdvancedDucking: 0,
        mDuckingLevel: AUVoiceIOOtherAudioDuckingLevel::Default,
    }
}

/// Sets an `AudioUnit` property, translating Core Audio's
/// property-size/status protocol into [`size_of_u32`]/[`check_status`].
/// Shared by every [`VoiceProcessingStream::configure`] property call.
fn set_unit_property<T>(
    unit: AudioUnit,
    property_id: u32,
    scope: u32,
    element: u32,
    value: &T,
    operation: &'static str,
) -> Result<(), MacCaptureError> {
    let size = size_of_u32::<T>(operation)?;
    // SAFETY: `unit` is a live `AudioUnit` instance; `value` is a
    // valid, correctly-sized reference for the property being set,
    // for the duration of this call.
    let status = unsafe {
        AudioUnitSetProperty(
            unit,
            property_id,
            scope,
            element,
            std::ptr::from_ref(value).cast::<c_void>(),
            size,
        )
    };
    check_status(status, operation)
}

/// `VoiceProcessingIO` input render callback. Pulls the just-captured
/// audio via [`AudioUnitRender`] into the context's scratch buffer and
/// forwards it as an [`AudioFrame`] — the same contract `MicStream`'s
/// IO block uses. A nonzero `AudioUnitRender` status surfaces as a
/// channel error via [`report_render_failure`] instead of silently
/// dropping the frame or falling back to the plain stream
/// mid-session.
///
/// # Safety
///
/// Core Audio guarantees `in_ref_con` is the exact pointer registered
/// via `kAudioOutputUnitProperty_SetInputCallback` in
/// [`VoiceProcessingStream::configure`], and does not invoke this
/// callback after `AudioComponentInstanceDispose` returns, so the
/// `VoiceCallbackContext` it points to remains valid for the duration
/// of this call.
unsafe extern "C-unwind" fn voice_processing_input_callback(
    in_ref_con: NonNull<c_void>,
    io_action_flags: NonNull<AudioUnitRenderActionFlags>,
    in_time_stamp: NonNull<AudioTimeStamp>,
    in_bus_number: u32,
    in_number_frames: u32,
    _io_data: *mut AudioBufferList,
) -> i32 {
    // SAFETY: see the function's `# Safety` section.
    let context = unsafe { in_ref_con.cast::<VoiceCallbackContext>().as_ref() };

    let channels = usize::from(context.channels);
    let needed = usize::try_from(in_number_frames).unwrap_or(0) * channels;
    let mut scratch = match context.scratch.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };
    if scratch.len() < needed {
        scratch.resize(needed, 0.0);
    }

    let buffer = AudioBuffer {
        mNumberChannels: u32::from(context.channels),
        mDataByteSize: u32::try_from(needed * std::mem::size_of::<f32>()).unwrap_or(0),
        mData: scratch.as_mut_ptr().cast::<c_void>(),
    };
    let mut buffer_list = AudioBufferList {
        mNumberBuffers: 1,
        mBuffers: [buffer],
    };

    // SAFETY: `context.unit` is the live `VoiceProcessingIO` unit that
    // owns this callback; `buffer_list` describes a writable buffer
    // sized for exactly `in_number_frames` frames of
    // `context.channels`-channel interleaved f32 PCM.
    // `in_time_stamp`/`io_action_flags` are the exact values CoreAudio
    // passed into this callback.
    let render_status = unsafe {
        AudioUnitRender(
            context.unit,
            io_action_flags.as_ptr(),
            in_time_stamp,
            in_bus_number,
            in_number_frames,
            NonNull::from(&mut buffer_list),
        )
    };
    if render_status != 0 {
        report_render_failure(&context.sender, render_status);
        return render_status;
    }

    let samples = &scratch[..needed];
    if samples.is_empty() {
        return 0;
    }
    let frames = u64::from(in_number_frames);
    let prior = context.sample_counter.fetch_add(frames, Ordering::Relaxed);
    let timestamp_ns = prior
        .saturating_mul(1_000_000_000)
        .checked_div(u64::from(context.sample_rate))
        .unwrap_or(0);
    let frame = AudioFrame::from_slice(
        samples,
        context.channels,
        context.sample_rate,
        timestamp_ns,
        FrameSource::Mic,
    );
    drop(scratch);
    if let Ok(guard) = context.sender.lock() {
        if let Some(sender) = guard.as_ref() {
            let _ = sender.send(Ok(frame));
        }
    } else {
        tracing::error!("VoiceProcessingIO sender mutex poisoned");
    }
    0
}

/// Surfaces a nonzero `AudioUnitRender` status as a channel error
/// instead of silently dropping the frame — a running
/// `VoiceProcessingIO` stream's failures must reach the consumer, not
/// trigger a silent runtime fallback to the plain stream mid-session.
fn report_render_failure(sender: &SharedSender, status: i32) {
    if let Err(error) = check_status(status, "AudioUnitRender(VoiceProcessingIO)") {
        let guard = match sender.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        if let Some(sender) = guard.as_ref() {
            let _ = sender.send(Err(CaptureError::from(error)));
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
    use std::cell::Cell;

    use super::*;

    #[test]
    fn native_capture_retains_the_exact_uid() {
        let capture = NativeMicCapture::new("BuiltInMicrophoneDevice".to_string(), false);
        assert_eq!(capture.uid(), "BuiltInMicrophoneDevice");
    }

    #[test]
    fn test_acquire_stream_default_false_never_attempts_voice_path() {
        let voice_attempted = Cell::new(false);

        let outcome = acquire_stream(
            false,
            || {
                voice_attempted.set(true);
                Ok::<_, MacCaptureError>("voice")
            },
            || Ok::<_, MacCaptureError>("plain"),
        );

        assert!(matches!(outcome, Ok(Attempt::Plain("plain"))));
        assert!(!voice_attempted.get());
    }

    #[test]
    fn test_acquire_stream_forced_voice_failure_falls_back_to_plain_stream() {
        let plain_attempted = Cell::new(false);

        let outcome = acquire_stream(
            true,
            || {
                Err::<&str, _>(MacCaptureError::CoreAudio(
                    "forced test failure".to_string(),
                ))
            },
            || {
                plain_attempted.set(true);
                Ok::<_, MacCaptureError>("plain live")
            },
        );

        assert!(matches!(outcome, Ok(Attempt::Plain("plain live"))));
        assert!(plain_attempted.get());
    }

    #[test]
    fn test_acquire_stream_surfaces_plain_error_when_both_paths_fail() {
        let outcome = acquire_stream(
            true,
            || Err::<(), _>(MacCaptureError::CoreAudio("voice failed".to_string())),
            || Err::<(), _>(MacCaptureError::CoreAudio("plain failed".to_string())),
        );

        match outcome {
            Err(MacCaptureError::CoreAudio(message)) => assert_eq!(message, "plain failed"),
            other => panic!("expected the plain-stream failure to surface, got {other:?}"),
        }
    }

    #[test]
    fn test_ducking_configuration_disables_advanced_ducking_with_default_level() {
        let configuration = ducking_configuration();

        assert_eq!(configuration.mEnableAdvancedDucking, 0);
        assert_eq!(
            configuration.mDuckingLevel,
            AUVoiceIOOtherAudioDuckingLevel::Default
        );
    }

    #[test]
    fn test_report_render_failure_surfaces_nonzero_status_through_channel() {
        let (tx, mut rx) = unbounded_channel::<Result<AudioFrame, CaptureError>>();
        let shared: SharedSender = Arc::new(Mutex::new(Some(tx)));

        report_render_failure(&shared, -1);

        let received = rx.try_recv().expect("render failure should be forwarded");
        assert!(matches!(received, Err(CaptureError::Platform(_))));
    }
}
