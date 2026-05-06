// Copyright 2026 Mathews Tom
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     https://www.apache.org/licenses/LICENSE-2.0

//! Live Core Audio Taps binding (macOS 14.4+).
//!
//! Wraps `CATapDescription`, `AudioHardwareCreateProcessTap`, an aggregate-
//! device sub-tap, and an IO block callback that drives the
//! [`AudioCapture`] frame stream. Everything `unsafe` lives in this
//! module; the rest of the crate stays under the workspace's
//! `unsafe_code = "deny"` lint.
//!
//! Lifecycle (Apple's documented sequence):
//!
//! 1. Construct a [`CATapDescription`] for a global stereo mixdown that
//!    excludes no processes (i.e., capture every audible process).
//! 2. [`AudioHardwareCreateProcessTap`] returns an [`AudioObjectID`] for
//!    the tap.
//! 3. Read the tap's UID via `kAudioTapPropertyUID` and the tap's stream
//!    format via `kAudioTapPropertyFormat`.
//! 4. Build an [`AudioHardwareCreateAggregateDevice`] dictionary with
//!    `kAudioAggregateDeviceTapListKey` containing the sub-tap's UID.
//!    The aggregate device is private (not visible system-wide) and
//!    not stacked.
//! 5. Install an IO block via [`AudioDeviceCreateIOProcIDWithBlock`] on
//!    the aggregate device. The block converts each
//!    [`AudioBufferList`] into an [`AudioFrame`] and forwards it
//!    through the shared [`UnboundedSender`].
//! 6. [`AudioDeviceStart`] kicks off the IO loop. [`Drop`] tears every
//!    allocation down in reverse order.
//!
//! [`AudioCapture`]: scrybe_core::capture::AudioCapture
//! [`UnboundedSender`]: tokio::sync::mpsc::UnboundedSender

#![allow(unsafe_code)]
#![allow(clippy::missing_safety_doc)]
// FFI shims trip pedantic and nursery lints that don't carry their
// usual signal here: every cast is documented against an Apple type,
// every `pub(crate)` is exposed only to in-tree tests, and the IO
// block's signature is fixed by `block2`. Allow at module scope rather
// than peppering the file.
#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss,
    clippy::doc_markdown,
    clippy::redundant_pub_crate,
    clippy::significant_drop_tightening,
    clippy::similar_names,
    clippy::too_many_lines,
    clippy::type_complexity,
    clippy::unnecessary_wraps,
    clippy::manual_inspect,
    clippy::needless_pass_by_value,
    clippy::missing_errors_doc,
    clippy::missing_panics_doc,
    dead_code
)]

use std::ffi::CStr;
use std::ptr::NonNull;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use block2::RcBlock;
use objc2::rc::Retained;
use objc2::AllocAnyThread;
use objc2_core_audio::{
    kAudioAggregateDeviceIsPrivateKey, kAudioAggregateDeviceIsStackedKey,
    kAudioAggregateDeviceMainSubDeviceKey, kAudioAggregateDeviceNameKey,
    kAudioAggregateDeviceSubDeviceListKey, kAudioAggregateDeviceTapAutoStartKey,
    kAudioAggregateDeviceTapListKey, kAudioAggregateDeviceUIDKey, kAudioDevicePropertyDeviceUID,
    kAudioHardwarePropertyDefaultSystemOutputDevice, kAudioObjectPropertyElementMain,
    kAudioObjectPropertyScopeGlobal, kAudioSubDeviceUIDKey, kAudioSubTapDriftCompensationKey,
    kAudioSubTapUIDKey, kAudioTapPropertyFormat, kAudioTapPropertyUID,
    AudioDeviceCreateIOProcIDWithBlock, AudioDeviceDestroyIOProcID, AudioDeviceIOProcID,
    AudioDeviceStart, AudioDeviceStop, AudioHardwareCreateAggregateDevice,
    AudioHardwareCreateProcessTap, AudioHardwareDestroyAggregateDevice,
    AudioHardwareDestroyProcessTap, AudioObjectGetPropertyData, AudioObjectID,
    AudioObjectPropertyAddress, CATapDescription,
};
use objc2_core_audio_types::{AudioBufferList, AudioStreamBasicDescription, AudioTimeStamp};
use objc2_core_foundation::{CFDictionary, CFRetained};
use objc2_foundation::{NSArray, NSDictionary, NSNumber, NSObject, NSString};
use tokio::sync::mpsc::UnboundedSender;
use tracing::{debug, warn};

use scrybe_core::error::CaptureError;
use scrybe_core::types::{AudioFrame, FrameSource};

use crate::error::MacCaptureError;

/// Channel for capture frames. Held inside an `Arc<Mutex<Option<...>>>`
/// so the IO block can detect a closed sender after `stop()` and stop
/// pushing frames without panicking.
type SharedSender = Arc<Mutex<Option<UnboundedSender<Result<AudioFrame, CaptureError>>>>>;

/// `kAudioObjectSystemObject`. Not re-exported by `objc2-core-audio` at
/// the time of writing (only the typedef and class IDs are), so we name
/// it ourselves. Apple's `CoreAudio/AudioHardwareBase.h` documents the
/// value as `1` since the API was introduced.
const SYSTEM_OBJECT_ID: AudioObjectID = 1;

/// Live Core Audio Taps binding driven by an IO block callback.
///
/// The struct owns three CoreAudio resources that must all be released
/// in reverse construction order: the aggregate-device IO proc id, the
/// aggregate device, and the process tap. [`Drop`] enforces this even
/// on panic; explicit [`stop`] is also available so callers can collect
/// errors instead of swallowing them in a destructor.
///
/// [`stop`]: TapStream::stop
pub(crate) struct TapStream {
    tap_id: AudioObjectID,
    aggregate_device_id: AudioObjectID,
    io_proc_id: AudioDeviceIOProcID,
    started: bool,
    sender: SharedSender,
    /// Negotiated sample rate from the live tap. Read by hardware
    /// tests in this module's `tests` submodule; not currently
    /// surfaced through `MacCapture::capabilities` because
    /// capabilities are reported pre-start.
    sample_rate: u32,
    channels: u16,
    /// Preserved so the block can be reconstructed across restarts.
    /// Held as a field rather than dropped immediately because
    /// `AudioDeviceCreateIOProcIDWithBlock` documents that the block is
    /// `Block_copy`'d, but holding our `RcBlock` keeps the inferred
    /// lifetime obvious to readers.
    _io_block: RcBlock<
        dyn Fn(
                NonNull<AudioTimeStamp>,
                NonNull<AudioBufferList>,
                NonNull<AudioTimeStamp>,
                NonNull<AudioBufferList>,
                NonNull<AudioTimeStamp>,
            ) + 'static,
    >,
}

// SAFETY: The CoreAudio HAL serializes access to `AudioObjectID`s via
// the audio server. The `RcBlock` is `!Send` by default, so we hand-roll
// the marker; the block is only ever invoked on the CoreAudio IO
// dispatch thread, which CoreAudio owns. We never touch `_io_block`
// from Rust after construction.
#[allow(clippy::non_send_fields_in_send_ty)]
unsafe impl Send for TapStream {}

/// Drop-on-error guard for a process tap. Holds the tap's
/// [`AudioObjectID`] and destroys it when dropped. Used to make
/// [`TapStream::create`]'s failure-cleanup linear: each fallible
/// step propagates with `?` and the guard's `Drop` releases the tap;
/// on success, [`TapGuard::release`] consumes the guard so the
/// returned [`TapStream`] takes ownership.
struct TapGuard(AudioObjectID);

impl TapGuard {
    const fn release(self) -> AudioObjectID {
        let id = self.0;
        std::mem::forget(self);
        id
    }
}

impl Drop for TapGuard {
    fn drop(&mut self) {
        unsafe {
            let _ = AudioHardwareDestroyProcessTap(self.0);
        }
    }
}

/// Drop-on-error guard for an aggregate device. See [`TapGuard`].
struct AggregateGuard(AudioObjectID);

impl AggregateGuard {
    const fn release(self) -> AudioObjectID {
        let id = self.0;
        std::mem::forget(self);
        id
    }
}

impl Drop for AggregateGuard {
    fn drop(&mut self) {
        unsafe {
            let _ = AudioHardwareDestroyAggregateDevice(self.0);
        }
    }
}

impl TapStream {
    /// Construct, register, and prepare (but do not start) a process
    /// tap. The caller invokes [`start`] to begin pushing frames into
    /// `sender`.
    ///
    /// [`start`]: TapStream::start
    ///
    /// # Errors
    ///
    /// Returns [`MacCaptureError::CoreAudioTapUnsupported`] if any of
    /// the CoreAudio calls return a non-zero `OSStatus`. The error
    /// carries the `OSStatus` four-character code so a Mac developer
    /// can map it back via `otool`/`OSStatus.com`.
    pub fn create(
        sender: UnboundedSender<Result<AudioFrame, CaptureError>>,
    ) -> Result<Self, MacCaptureError> {
        let shared_sender: SharedSender = Arc::new(Mutex::new(Some(sender)));

        // 1. Tap description: global stereo mixdown of every audible
        // process. `initStereoGlobalTapButExcludeProcesses(@[])`
        // captures everything the user can hear.
        // SAFETY: `CATapDescription::alloc` is documented to return a
        // freshly-retained instance; we drop it into the init method as
        // required by Cocoa init semantics.
        let empty: Retained<NSArray<NSNumber>> = NSArray::new();
        let tap_description: Retained<CATapDescription> = unsafe {
            CATapDescription::initStereoGlobalTapButExcludeProcesses(
                CATapDescription::alloc(),
                &empty,
            )
        };
        // Mute behavior is unmuted by default; mark private so the tap
        // is not visible to other processes.
        unsafe {
            tap_description.setPrivate(true);
        }

        // 2. Create the process tap. Wrap it in a TapGuard so any
        // `?` between here and the end of the function tears the tap
        // down before propagating the error.
        let mut tap_id: AudioObjectID = 0;
        let status =
            unsafe { AudioHardwareCreateProcessTap(Some(&tap_description), &raw mut tap_id) };
        check_status(status, "AudioHardwareCreateProcessTap")?;
        debug!(tap_id, "process tap created");
        let tap_guard = TapGuard(tap_id);

        // 3a. Read the tap's UID for the aggregate-device dictionary.
        let tap_uid = read_object_uid(tap_id, kAudioTapPropertyUID)?;

        // 3b. Read the tap's stream format so we can advertise the
        // correct sample rate and channel count to scrybe-core.
        let format = read_tap_format(tap_id)?;
        let sample_rate = format.mSampleRate as u32;
        let channels = u16::try_from(format.mChannelsPerFrame).map_err(|_| {
            MacCaptureError::CoreAudioTapUnsupported {
                found: format!(
                    "implausible channel count from kAudioTapPropertyFormat: {}",
                    format.mChannelsPerFrame
                ),
            }
        })?;
        debug!(sample_rate, channels, "tap stream format negotiated");

        // 4. Aggregate device dictionary with the tap as a sub-tap.
        //
        // Apple's reference implementation (insidegui/AudioCap) wires
        // the aggregate to the user's actual default output device
        // through `kAudioAggregateDeviceMainSubDeviceKey` +
        // `kAudioAggregateDeviceSubDeviceListKey`. Without these the
        // aggregate has no rendering pipeline for the tap to mirror,
        // and the IO callback fires but the buffer it reads is
        // undriven — observed in v1.0.x as `frames=141 peak=0.0000`
        // in `scrybe doctor --check-tap` output even with TCC consent
        // granted. Read the default output's UID once at construction
        // so the aggregate can anchor to a concrete device.
        let output_device_uid = read_default_output_device_uid()?;
        debug!(output_device_uid, "default system output resolved");
        let aggregate_device_dict = build_aggregate_device_dict(&tap_uid, &output_device_uid)?;
        let mut aggregate_device_id: AudioObjectID = 0;
        let status = unsafe {
            AudioHardwareCreateAggregateDevice(
                &aggregate_device_dict,
                NonNull::from(&mut aggregate_device_id),
            )
        };
        check_status(status, "AudioHardwareCreateAggregateDevice")?;
        debug!(aggregate_device_id, "aggregate device created");
        let aggregate_guard = AggregateGuard(aggregate_device_id);

        // 5. Install the IO block. The block captures the shared sender
        // and the negotiated format so it can construct AudioFrames
        // without re-reading CoreAudio properties on every callback.
        let block_sender = Arc::clone(&shared_sender);
        let sample_counter = Arc::new(AtomicU64::new(0));
        let block_counter = Arc::clone(&sample_counter);

        // Diagnostic accumulators sampled once per second of audio so a
        // user running `RUST_LOG=scrybe_capture_mac=debug scrybe record`
        // can observe whether the tap is delivering real samples or
        // zero-filled buffers. Distinguishing those two failure shapes
        // is the v1.0.x → v1.1 follow-up for the system-tap-silent-frames
        // bug: macOS Core Audio Taps deliver silence to a binary whose
        // entitlement chain it cannot verify (see CHANGELOG `[Unreleased]`
        // known limitations).
        //
        // Atomics rather than a Mutex because the IO block runs on
        // CoreAudio's real-time dispatch thread and any blocking acquire
        // would risk audio glitches on the user's actual playback. Peak
        // amplitude is a non-negative f32 stored via `to_bits` so
        // `AtomicU32::fetch_max` produces a numerically correct max
        // (IEEE 754 non-negative f32 sorts identically to its bit
        // pattern as u32).
        let diag_window_samples = Arc::new(AtomicU64::new(0));
        let diag_window_peak_bits = Arc::new(AtomicU32::new(0));
        let block_diag_samples = Arc::clone(&diag_window_samples);
        let block_diag_peak = Arc::clone(&diag_window_peak_bits);
        let io_block: RcBlock<
            dyn Fn(
                    NonNull<AudioTimeStamp>,
                    NonNull<AudioBufferList>,
                    NonNull<AudioTimeStamp>,
                    NonNull<AudioBufferList>,
                    NonNull<AudioTimeStamp>,
                ) + 'static,
        > = RcBlock::new(
            move |_in_now: NonNull<AudioTimeStamp>,
                  in_input_data: NonNull<AudioBufferList>,
                  _in_input_time: NonNull<AudioTimeStamp>,
                  _out_output_data: NonNull<AudioBufferList>,
                  _in_output_time: NonNull<AudioTimeStamp>| {
                // SAFETY: CoreAudio guarantees the input buffer list is valid
                // for the duration of the callback. We copy out, never alias.
                let samples = unsafe { interleaved_f32_samples(in_input_data.as_ref()) };
                if samples.is_empty() {
                    return;
                }
                let frames_added =
                    u64::try_from(samples.len()).unwrap_or(0) / u64::from(channels.max(1));
                // `fetch_add` returns the prior value atomically — one
                // op instead of `load`+`fetch_add`. CoreAudio serializes
                // IO callbacks per device today, but using the atomic
                // primitive that matches the operation removes a
                // load-bearing assumption from the threading model.
                let prior = block_counter.fetch_add(frames_added, Ordering::Relaxed);
                let timestamp_ns = prior
                    .saturating_mul(1_000_000_000)
                    .checked_div(u64::from(sample_rate.max(1)))
                    .unwrap_or(0);

                // Diagnostic window. Find the loudest absolute sample in
                // this batch and fold it into the per-second peak. When
                // the running window crosses one second of audio, log
                // the peak/frame-count and reset. Absent
                // `RUST_LOG=scrybe_capture_mac=debug` this is roughly
                // free — `tracing::debug!` short-circuits on level
                // before formatting.
                let local_peak = samples
                    .iter()
                    .copied()
                    .map(f32::abs)
                    .fold(0.0_f32, f32::max);
                block_diag_peak.fetch_max(local_peak.to_bits(), Ordering::Relaxed);
                let prior_window = block_diag_samples.fetch_add(frames_added, Ordering::Relaxed);
                let new_window = prior_window.saturating_add(frames_added);
                let one_second = u64::from(sample_rate.max(1));
                if (new_window / one_second) > (prior_window / one_second) {
                    let peak_bits = block_diag_peak.swap(0, Ordering::Relaxed);
                    block_diag_samples.store(0, Ordering::Relaxed);
                    let peak = f32::from_bits(peak_bits);
                    if peak < 1e-6 {
                        // Surface the silence case at WARN so a user
                        // recording a meeting with audio actually
                        // playing notices the tap is dead even without
                        // RUST_LOG tuned. Throttled to once per second
                        // so it does not spam the terminal.
                        warn!(
                            sample_rate,
                            channels,
                            "core-audio-tap delivered one second of silent frames \
                             (peak={peak:.6}); likely TCC/entitlement or device-routing \
                             issue"
                        );
                    } else {
                        debug!(
                            sample_rate,
                            channels, peak, "core-audio-tap one-second diagnostic"
                        );
                    }
                }
                let frame = AudioFrame::from_slice(
                    &samples,
                    channels,
                    sample_rate,
                    timestamp_ns,
                    FrameSource::System,
                );
                match block_sender.lock() {
                    Ok(guard) => {
                        if let Some(tx) = guard.as_ref() {
                            // `send` returns Err only if the receiver
                            // was dropped — capture has been stopped;
                            // dropping the frame is the right behavior.
                            let _ = tx.send(Ok(frame));
                        }
                    }
                    Err(_) => {
                        tracing::error!("sender mutex poisoned in IO callback; frame dropped");
                    }
                }
            },
        );

        let mut io_proc_id: AudioDeviceIOProcID = None;
        let status = unsafe {
            AudioDeviceCreateIOProcIDWithBlock(
                NonNull::from(&mut io_proc_id),
                aggregate_device_id,
                None,
                RcBlock::as_ptr(&io_block),
            )
        };
        check_status(status, "AudioDeviceCreateIOProcIDWithBlock")?;
        debug!("IO proc id installed on aggregate device");

        // Success: hand the resource ids over to TapStream and disarm
        // the cleanup guards so Drop doesn't free what TapStream now
        // owns.
        Ok(Self {
            tap_id: tap_guard.release(),
            aggregate_device_id: aggregate_guard.release(),
            io_proc_id,
            started: false,
            sender: shared_sender,
            sample_rate,
            channels,
            _io_block: io_block,
        })
    }

    /// Begin pushing frames through the sender. Idempotent — calling
    /// twice returns `Ok(())` without re-issuing `AudioDeviceStart`.
    ///
    /// # Errors
    ///
    /// Wraps any non-zero `OSStatus` returned by `AudioDeviceStart`.
    pub fn start(&mut self) -> Result<(), MacCaptureError> {
        if self.started {
            return Ok(());
        }
        let status = unsafe { AudioDeviceStart(self.aggregate_device_id, self.io_proc_id) };
        check_status(status, "AudioDeviceStart")?;
        self.started = true;
        debug!("audio device started");
        Ok(())
    }

    /// Stop pushing frames and detach the sender. Idempotent.
    ///
    /// # Errors
    ///
    /// Wraps any non-zero `OSStatus` returned by `AudioDeviceStop`. The
    /// teardown of the IO proc, aggregate device, and tap continues
    /// regardless via [`Drop`].
    pub fn stop(&mut self) -> Result<(), MacCaptureError> {
        if !self.started {
            // Even when not started, drop the sender so the receiver
            // sees an end-of-stream. Recover from poisoning so a
            // panicking IO callback cannot leave the receiver hung.
            drop_sender(&self.sender);
            return Ok(());
        }
        let status = unsafe { AudioDeviceStop(self.aggregate_device_id, self.io_proc_id) };
        self.started = false;
        drop_sender(&self.sender);
        check_status(status, "AudioDeviceStop")
    }
}

impl Drop for TapStream {
    fn drop(&mut self) {
        // Stop ignores its own error; we cannot return one from Drop.
        // We log instead so a maintainer reading `RUST_LOG=warn` traces
        // can see the OSStatus.
        if self.started {
            let status = unsafe { AudioDeviceStop(self.aggregate_device_id, self.io_proc_id) };
            if status != 0 {
                warn!(
                    status,
                    "AudioDeviceStop returned non-zero status during Drop"
                );
            }
        }
        let status =
            unsafe { AudioDeviceDestroyIOProcID(self.aggregate_device_id, self.io_proc_id) };
        if status != 0 {
            warn!(
                status,
                "AudioDeviceDestroyIOProcID returned non-zero status"
            );
        }
        let status = unsafe { AudioHardwareDestroyAggregateDevice(self.aggregate_device_id) };
        if status != 0 {
            warn!(
                status,
                "AudioHardwareDestroyAggregateDevice returned non-zero status"
            );
        }
        let status = unsafe { AudioHardwareDestroyProcessTap(self.tap_id) };
        if status != 0 {
            warn!(
                status,
                "AudioHardwareDestroyProcessTap returned non-zero status"
            );
        }
    }
}

/// Walk an [`AudioBufferList`] and copy its f32 PCM samples into a
/// `Vec<f32>`. Handles both the interleaved (`mNumberBuffers == 1`)
/// and non-interleaved (`mNumberBuffers == channels`) layouts that the
/// HAL produces. Non-f32 buffer formats produce an empty vec — the
/// caller treats an empty result as "no audio this cycle" rather than
/// fabricating samples.
///
/// # Safety
///
/// `list` must reference a CoreAudio-owned [`AudioBufferList`] whose
/// `mBuffers` flexible array has at least `mNumberBuffers` elements
/// and whose `mData` pointers are valid for `mDataByteSize` bytes for
/// the duration of this call.
pub(crate) unsafe fn interleaved_f32_samples(list: &AudioBufferList) -> Vec<f32> {
    let n_buffers = list.mNumberBuffers as usize;
    if n_buffers == 0 {
        return Vec::new();
    }
    // SAFETY: `mBuffers` is a flexible array — CoreAudio places
    // `n_buffers` `AudioBuffer` records contiguously starting at
    // `mBuffers.as_ptr()`. We honour the length read from
    // `mNumberBuffers` and never read past it.
    let buffers = unsafe { std::slice::from_raw_parts(list.mBuffers.as_ptr(), n_buffers) };
    if n_buffers == 1 {
        let buf = &buffers[0];
        if buf.mData.is_null() || buf.mDataByteSize == 0 {
            return Vec::new();
        }
        let n_samples = (buf.mDataByteSize as usize) / std::mem::size_of::<f32>();
        // SAFETY: caller's invariant. `buf.mData` is non-null and points
        // at `buf.mDataByteSize` bytes of f32 PCM.
        let slice =
            unsafe { std::slice::from_raw_parts(buf.mData.cast::<f32>().cast_const(), n_samples) };
        return slice.to_vec();
    }
    // Non-interleaved path: each buffer is one channel's planar data.
    // Interleave into a single Vec so AudioFrame::from_slice has a
    // single sample stream.
    let frames_per_channel = (buffers[0].mDataByteSize as usize) / std::mem::size_of::<f32>();
    let mut out = vec![0.0_f32; frames_per_channel * n_buffers];
    for (channel_index, buf) in buffers.iter().enumerate() {
        if buf.mData.is_null() {
            continue;
        }
        let n = (buf.mDataByteSize as usize) / std::mem::size_of::<f32>();
        // SAFETY: caller's invariant.
        let slice = unsafe { std::slice::from_raw_parts(buf.mData.cast::<f32>().cast_const(), n) };
        for (sample_index, sample) in slice.iter().enumerate().take(frames_per_channel) {
            out[sample_index * n_buffers + channel_index] = *sample;
        }
    }
    out
}

/// Read a CFString-valued property and return its UTF-8 form.
fn read_object_uid(object_id: AudioObjectID, selector: u32) -> Result<String, MacCaptureError> {
    let address = AudioObjectPropertyAddress {
        mSelector: selector,
        mScope: kAudioObjectPropertyScopeGlobal,
        mElement: kAudioObjectPropertyElementMain,
    };
    let mut cfstring: *const objc2_core_foundation::CFString = std::ptr::null();
    let mut size = std::mem::size_of::<*const objc2_core_foundation::CFString>() as u32;
    let status = unsafe {
        AudioObjectGetPropertyData(
            object_id,
            NonNull::from(&address),
            0,
            std::ptr::null(),
            NonNull::from(&mut size),
            NonNull::from(&mut cfstring).cast(),
        )
    };
    check_status(status, "AudioObjectGetPropertyData(uid)")?;
    if cfstring.is_null() {
        return Err(MacCaptureError::CoreAudioTapUnsupported {
            found: "uid CFString was null".to_string(),
        });
    }
    // SAFETY: CoreAudio returned us a +1 retain count CFString for a
    // copy property; convert via CFRetained::from_raw to take ownership
    // and let it release on drop.
    let cf = unsafe { CFRetained::from_raw(NonNull::new_unchecked(cfstring.cast_mut())) };
    Ok(cf.to_string())
}

/// Read the tap's negotiated `AudioStreamBasicDescription`.
fn read_tap_format(tap_id: AudioObjectID) -> Result<AudioStreamBasicDescription, MacCaptureError> {
    let address = AudioObjectPropertyAddress {
        mSelector: kAudioTapPropertyFormat,
        mScope: kAudioObjectPropertyScopeGlobal,
        mElement: kAudioObjectPropertyElementMain,
    };
    let mut format = AudioStreamBasicDescription {
        mSampleRate: 0.0,
        mFormatID: 0,
        mFormatFlags: 0,
        mBytesPerPacket: 0,
        mFramesPerPacket: 0,
        mBytesPerFrame: 0,
        mChannelsPerFrame: 0,
        mBitsPerChannel: 0,
        mReserved: 0,
    };
    let mut size = std::mem::size_of::<AudioStreamBasicDescription>() as u32;
    let status = unsafe {
        AudioObjectGetPropertyData(
            tap_id,
            NonNull::from(&address),
            0,
            std::ptr::null(),
            NonNull::from(&mut size),
            NonNull::from(&mut format).cast(),
        )
    };
    check_status(status, "AudioObjectGetPropertyData(format)")?;
    Ok(format)
}

/// Construct the CFDictionary that
/// `AudioHardwareCreateAggregateDevice` consumes. The dictionary
/// shape mirrors the one Apple's reference implementation ships in
/// `insidegui/AudioCap` (`AudioCap/ProcessTap/ProcessTap.swift`):
///
/// ```text
/// {
///   kAudioAggregateDeviceUIDKey:           "dev.scrybe.tap-aggregate.<tap-uid>"
///   kAudioAggregateDeviceNameKey:          "scrybe-tap-aggregate"
///   kAudioAggregateDeviceIsPrivateKey:     1
///   kAudioAggregateDeviceIsStackedKey:     0
///   kAudioAggregateDeviceTapAutoStartKey:  1
///   kAudioAggregateDeviceMainSubDeviceKey: <output device UID>
///   kAudioAggregateDeviceSubDeviceListKey: [ { kAudioSubDeviceUIDKey: <output UID> } ]
///   kAudioAggregateDeviceTapListKey:       [
///       { kAudioSubTapUIDKey: <tap UID>,
///         kAudioSubTapDriftCompensationKey: 1 }
///   ]
/// }
/// ```
///
/// The main sub-device + sub-device list keys are critical: without
/// them the aggregate is "an aggregate of nothing plus a tap" — the
/// IO callback fires but reads from an undriven buffer and produces
/// the zero-filled frames we observed in v1.0.x.
///
/// We build everything as `NSDictionary` / `NSArray` and toll-free
/// bridge to `CFDictionary` at the end (Apple-documented bridging path
/// on macOS).
fn build_aggregate_device_dict(
    tap_uid: &str,
    output_device_uid: &str,
) -> Result<CFRetained<CFDictionary>, MacCaptureError> {
    let aggregate_uid_value = NSString::from_str(&format!("dev.scrybe.tap-aggregate.{tap_uid}"));
    let aggregate_name_value = NSString::from_str("scrybe-tap-aggregate");
    let true_num = NSNumber::new_i32(1);
    let false_num = NSNumber::new_i32(0);

    // Sub-tap entry: tap UID + drift compensation. Drift compensation
    // resamples the tap's clock to match the main device's clock so
    // the IO callback receives sample-aligned audio without periodic
    // overruns/underruns. AudioCap sets this to true unconditionally;
    // we follow suit.
    let sub_tap_dict: Retained<NSDictionary<NSString, NSObject>> = {
        let key_uid = NSString::from_str(c_str_to_str(kAudioSubTapUIDKey));
        let key_drift = NSString::from_str(c_str_to_str(kAudioSubTapDriftCompensationKey));
        let val_uid = NSString::from_str(tap_uid);
        let val_drift = NSNumber::new_i32(1);
        let keys: [&NSString; 2] = [&key_uid, &key_drift];
        let v_uid: Retained<NSObject> = val_uid.into_super();
        let v_drift: Retained<NSObject> = val_drift.into_super().into_super();
        let values: [&NSObject; 2] = [&v_uid, &v_drift];
        NSDictionary::from_slices(&keys, &values)
    };
    let tap_list: Retained<NSArray<NSDictionary<NSString, NSObject>>> =
        NSArray::from_retained_slice(&[sub_tap_dict]);

    // Sub-device entry: pin the aggregate's "main" sub-device to the
    // user's current default output. The aggregate mirrors this
    // device's stream and the tap captures the result.
    let sub_device_dict: Retained<NSDictionary<NSString, NSString>> = {
        let key = NSString::from_str(c_str_to_str(kAudioSubDeviceUIDKey));
        let val = NSString::from_str(output_device_uid);
        let keys: [&NSString; 1] = [&key];
        let values: [&NSString; 1] = [&val];
        NSDictionary::from_slices(&keys, &values)
    };
    let sub_device_list: Retained<NSArray<NSDictionary<NSString, NSString>>> =
        NSArray::from_retained_slice(&[sub_device_dict]);

    let key_uid = NSString::from_str(c_str_to_str(kAudioAggregateDeviceUIDKey));
    let key_name = NSString::from_str(c_str_to_str(kAudioAggregateDeviceNameKey));
    let key_private = NSString::from_str(c_str_to_str(kAudioAggregateDeviceIsPrivateKey));
    let key_stacked = NSString::from_str(c_str_to_str(kAudioAggregateDeviceIsStackedKey));
    let key_auto_start = NSString::from_str(c_str_to_str(kAudioAggregateDeviceTapAutoStartKey));
    let key_main_sub = NSString::from_str(c_str_to_str(kAudioAggregateDeviceMainSubDeviceKey));
    let key_sub_devices = NSString::from_str(c_str_to_str(kAudioAggregateDeviceSubDeviceListKey));
    let key_taps = NSString::from_str(c_str_to_str(kAudioAggregateDeviceTapListKey));

    let main_sub_device_value = NSString::from_str(output_device_uid);

    let keys: [&NSString; 8] = [
        &key_uid,
        &key_name,
        &key_private,
        &key_stacked,
        &key_auto_start,
        &key_main_sub,
        &key_sub_devices,
        &key_taps,
    ];
    let v_uid: Retained<NSObject> = aggregate_uid_value.into_super();
    let v_name: Retained<NSObject> = aggregate_name_value.into_super();
    let v_private: Retained<NSObject> = true_num.clone().into_super().into_super();
    let v_stacked: Retained<NSObject> = false_num.into_super().into_super();
    let v_auto_start: Retained<NSObject> = true_num.into_super().into_super();
    let v_main_sub: Retained<NSObject> = main_sub_device_value.into_super();
    let v_sub_devices: Retained<NSObject> = sub_device_list.into_super();
    let v_taps: Retained<NSObject> = tap_list.into_super();
    let value_refs: [&NSObject; 8] = [
        &v_uid,
        &v_name,
        &v_private,
        &v_stacked,
        &v_auto_start,
        &v_main_sub,
        &v_sub_devices,
        &v_taps,
    ];
    let ns_dict: Retained<NSDictionary<NSString, NSObject>> =
        NSDictionary::from_slices(&keys, &value_refs);

    // Toll-free bridge NSDictionary → CFDictionary.
    // SAFETY: NSDictionary and CFDictionary are documented to be
    // toll-free bridged on Apple platforms. The cast preserves the +1
    // retain count already held by `ns_dict`.
    unsafe {
        let raw: *mut NSDictionary<NSString, NSObject> = Retained::into_raw(ns_dict);
        let cf_raw: NonNull<CFDictionary> = NonNull::new_unchecked(raw.cast::<CFDictionary>());
        Ok(CFRetained::from_raw(cf_raw))
    }
}

/// Read the system's current default-output device's UID. Used to
/// anchor the aggregate device's main sub-device. This is the API
/// flow Apple's reference implementation follows for the global
/// audio capture path.
///
/// # Errors
///
/// Returns `MacCaptureError::CoreAudioTapUnsupported` if either of
/// the two `AudioObjectGetPropertyData` calls fails — typically
/// indicates no audio output device is currently configured.
fn read_default_output_device_uid() -> Result<String, MacCaptureError> {
    let address = AudioObjectPropertyAddress {
        mSelector: kAudioHardwarePropertyDefaultSystemOutputDevice,
        mScope: kAudioObjectPropertyScopeGlobal,
        mElement: kAudioObjectPropertyElementMain,
    };
    let mut device_id: AudioObjectID = 0;
    let mut size = std::mem::size_of::<AudioObjectID>() as u32;
    let status = unsafe {
        AudioObjectGetPropertyData(
            SYSTEM_OBJECT_ID,
            NonNull::from(&address),
            0,
            std::ptr::null(),
            NonNull::from(&mut size),
            NonNull::from(&mut device_id).cast(),
        )
    };
    check_status(
        status,
        "AudioObjectGetPropertyData(DefaultSystemOutputDevice)",
    )?;
    if device_id == 0 {
        return Err(MacCaptureError::CoreAudioTapUnsupported {
            found: "no default system output device is configured".to_string(),
        });
    }
    read_object_uid(device_id, kAudioDevicePropertyDeviceUID)
}

/// Map an `OSStatus` into a [`MacCaptureError`]. Zero is success; every
/// other value is rendered as a hex-encoded four-character code so a
/// developer can grep against `OSStatus.com`.
fn check_status(status: i32, op: &'static str) -> Result<(), MacCaptureError> {
    if status == 0 {
        return Ok(());
    }
    Err(MacCaptureError::CoreAudioTapUnsupported {
        found: format!("{op} returned OSStatus {status:#010x}"),
    })
}

/// Drop the channel sender so the receiver observes end-of-stream,
/// recovering from a poisoned mutex if a previous IO callback panicked.
/// Mirrors the recovery pattern in [`MacCapture::frames`] so a poisoned
/// sender mutex never leaves the pipeline blocked on a stream that will
/// never close.
fn drop_sender(sender: &SharedSender) {
    let mut guard = match sender.lock() {
        Ok(g) => g,
        Err(poisoned) => {
            warn!("sender mutex poisoned in TapStream::stop; recovering inner state");
            poisoned.into_inner()
        }
    };
    guard.take();
}

fn c_str_to_str(c: &CStr) -> &str {
    // Apple's `kAudioAggregateDevice*Key` and `kAudioSubTapUIDKey`
    // constants are documented as ASCII C strings; failing decode would
    // mean Apple shipped a non-ASCII framework constant, which would
    // break far more than this binding. Fail loudly so a bisect lands
    // on the offending toolchain bump rather than on a silently
    // malformed CFDictionary.
    #[allow(clippy::expect_used)]
    {
        c.to_str()
            .expect("Apple framework key constants are guaranteed ASCII/UTF-8")
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, unsafe_code)]
mod tests {
    use std::ffi::c_void;

    use objc2_core_audio_types::AudioBuffer;

    use super::*;

    fn make_buffer(samples: &mut Vec<f32>) -> AudioBuffer {
        AudioBuffer {
            mNumberChannels: 1,
            mDataByteSize: u32::try_from(samples.len() * std::mem::size_of::<f32>()).unwrap(),
            mData: samples.as_mut_ptr().cast::<c_void>(),
        }
    }

    #[test]
    fn test_interleaved_f32_samples_single_buffer_returns_pcm_in_order() {
        let mut buf_samples: Vec<f32> = vec![0.1, 0.2, 0.3, 0.4];
        let list = AudioBufferList {
            mNumberBuffers: 1,
            mBuffers: [make_buffer(&mut buf_samples)],
        };

        let out = unsafe { interleaved_f32_samples(&list) };

        assert_eq!(out, vec![0.1, 0.2, 0.3, 0.4]);
    }

    #[test]
    fn test_interleaved_f32_samples_zero_buffers_returns_empty_vec() {
        let list = AudioBufferList {
            mNumberBuffers: 0,
            mBuffers: [AudioBuffer {
                mNumberChannels: 0,
                mDataByteSize: 0,
                mData: std::ptr::null_mut(),
            }],
        };

        let out = unsafe { interleaved_f32_samples(&list) };

        assert!(out.is_empty());
    }

    #[test]
    fn test_interleaved_f32_samples_null_data_pointer_returns_empty_vec() {
        let list = AudioBufferList {
            mNumberBuffers: 1,
            mBuffers: [AudioBuffer {
                mNumberChannels: 1,
                mDataByteSize: 16,
                mData: std::ptr::null_mut(),
            }],
        };

        let out = unsafe { interleaved_f32_samples(&list) };

        assert!(out.is_empty());
    }

    #[test]
    fn test_interleaved_f32_samples_stereo_interleaved_preserves_channel_order() {
        // Stereo interleaved layout: one buffer, alternating L/R samples.
        let mut buf_samples: Vec<f32> = vec![0.1, -0.1, 0.2, -0.2];
        let buffer = AudioBuffer {
            mNumberChannels: 2,
            mDataByteSize: u32::try_from(buf_samples.len() * std::mem::size_of::<f32>()).unwrap(),
            mData: buf_samples.as_mut_ptr().cast::<c_void>(),
        };
        let list = AudioBufferList {
            mNumberBuffers: 1,
            mBuffers: [buffer],
        };

        let out = unsafe { interleaved_f32_samples(&list) };

        assert_eq!(out, vec![0.1, -0.1, 0.2, -0.2]);
    }

    #[test]
    #[allow(clippy::cast_ptr_alignment)]
    fn test_interleaved_f32_samples_planar_two_channels_interleaves_lr_pairs() {
        // CoreAudio lays out a non-interleaved buffer list as
        // `mNumberBuffers (u32) | padding | AudioBuffer[N]` with the
        // `[AudioBuffer; 1]` `mBuffers` slot acting as the head of the
        // flexible array. Reproducing that layout from safe Rust is
        // not possible because the public struct fixes the array to
        // length 1, so we allocate raw bytes that satisfy
        // `AudioBufferList`'s alignment and write the header followed
        // by two `AudioBuffer` records. This is exactly the layout the
        // function under test will see at runtime.
        let mut left: Vec<f32> = vec![1.0, 2.0];
        let mut right: Vec<f32> = vec![10.0, 20.0];
        let buf_l = AudioBuffer {
            mNumberChannels: 1,
            mDataByteSize: u32::try_from(left.len() * std::mem::size_of::<f32>()).unwrap(),
            mData: left.as_mut_ptr().cast::<c_void>(),
        };
        let buf_r = AudioBuffer {
            mNumberChannels: 1,
            mDataByteSize: u32::try_from(right.len() * std::mem::size_of::<f32>()).unwrap(),
            mData: right.as_mut_ptr().cast::<c_void>(),
        };

        let total = std::mem::offset_of!(AudioBufferList, mBuffers)
            + 2 * std::mem::size_of::<AudioBuffer>();
        let alignment = std::mem::align_of::<AudioBufferList>();
        let layout = std::alloc::Layout::from_size_align(total, alignment).unwrap();
        // SAFETY: layout has nonzero size; alloc_zeroed returns a
        // valid pointer or null. We assert non-null and then
        // initialize every field that interleaved_f32_samples reads.
        let raw = unsafe { std::alloc::alloc_zeroed(layout) };
        assert!(!raw.is_null(), "allocator returned null");
        let out = unsafe {
            let header = raw.cast::<AudioBufferList>();
            (*header).mNumberBuffers = 2;
            let buffers_start = raw
                .add(std::mem::offset_of!(AudioBufferList, mBuffers))
                .cast::<AudioBuffer>();
            buffers_start.write(buf_l);
            buffers_start.add(1).write(buf_r);
            let result = interleaved_f32_samples(&*header);
            std::alloc::dealloc(raw, layout);
            result
        };

        assert_eq!(out, vec![1.0, 10.0, 2.0, 20.0]);
    }

    #[test]
    fn test_check_status_zero_returns_ok() {
        assert!(check_status(0, "test").is_ok());
    }

    #[test]
    fn test_check_status_nonzero_returns_unsupported_with_op_and_hex_code() {
        let err = check_status(-1, "TestOp").unwrap_err();

        let MacCaptureError::CoreAudioTapUnsupported { found } = err else {
            panic!("expected CoreAudioTapUnsupported, got {err:?}");
        };
        assert!(found.contains("TestOp"), "missing op name: {found}");
        assert!(found.contains("0xff"), "missing hex code: {found}");
    }

    /// Hardware-validation test. Requires macOS 14.4+, a logged-in
    /// session with audio routing, and `SCRYBE_TEST_CAPTURE=1` in the
    /// environment. CI runners cannot grant Core Audio Tap permission;
    /// see `system-design.md` §11 Tier 3.
    #[test]
    #[ignore = "requires macOS 14.4+ hardware and SCRYBE_TEST_CAPTURE=1"]
    fn test_tap_stream_create_and_drop_on_real_hardware() {
        if std::env::var("SCRYBE_TEST_CAPTURE").ok().as_deref() != Some("1") {
            eprintln!("skipping: SCRYBE_TEST_CAPTURE=1 not set");
            return;
        }
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let stream = TapStream::create(tx).expect("create live tap");
        // Only assert the sample rate is plausible; we do not assert on
        // captured frames because that requires audio routing.
        assert!(
            stream.sample_rate >= 8_000 && stream.sample_rate <= 192_000,
            "implausible sample rate {}",
            stream.sample_rate
        );
        drop(stream);
    }

    /// Hardware-validation test that proves end-to-end audio data flow
    /// under TCC: spawns `afplay` against a system-shipped audio
    /// fixture, captures from the live tap for a bounded window, and
    /// asserts both that frames arrived AND that the captured peak
    /// amplitude exceeds a noise floor.
    ///
    /// The combined assertion uniquely identifies "TCC for Audio
    /// Capture is granted AND the tap is routed to the default output
    /// device". The two failure signatures distinguish:
    ///
    /// - Zero frames in the capture window → IOProc never fired
    ///   (`AudioDeviceStart` may have returned a delayed error or the
    ///   tap was constructed against the wrong device).
    /// - Frames arrived but peak ≈ 0 → TCC was denied (macOS delivers
    ///   zero-filled buffers in some versions instead of an explicit
    ///   error) OR the tap is mirroring a different device than the
    ///   one `afplay` writes to.
    /// - Frames arrived AND peak > 0.01 → TCC granted, tap routed
    ///   correctly, audio data flows end-to-end.
    ///
    /// Closes the verification gap left by
    /// `test_tap_stream_create_and_drop_on_real_hardware`, which only
    /// exercises the lifecycle (create/drop) and never starts the
    /// IOProc — so it would pass even with TCC denied.
    ///
    /// Per `.docs/development-plan.md` §7.3.3 this is the E-1
    /// scenario. Runs only on the self-hosted Tier-3 macOS runner per
    /// `docs/system-design.md` §11.
    const E1_FIXTURE_PATH: &str = "/System/Library/Sounds/Ping.aiff";
    const E1_CAPTURE_WINDOW: std::time::Duration = std::time::Duration::from_millis(1_500);
    const E1_NOISE_FLOOR: f32 = 0.01;

    #[tokio::test(flavor = "current_thread", start_paused = false)]
    #[ignore = "requires macOS 14.4+ hardware, audio output, and SCRYBE_TEST_CAPTURE=1"]
    async fn test_tap_captures_nonzero_frames_during_known_audio_playback() {
        if std::env::var("SCRYBE_TEST_CAPTURE").ok().as_deref() != Some("1") {
            eprintln!("skipping: SCRYBE_TEST_CAPTURE=1 not set");
            return;
        }
        assert!(
            std::path::Path::new(E1_FIXTURE_PATH).exists(),
            "fixture {E1_FIXTURE_PATH} missing — expected on every macOS install"
        );

        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let mut stream = TapStream::create(tx).expect("create live tap");
        stream.start().expect("start tap");

        // Spawn afplay synchronously (std::process); the test does not
        // need to await it — only kill/wait once the capture window
        // closes — so pulling tokio's `process` feature into dev-deps
        // would be overhead with no benefit.
        let mut afplay = std::process::Command::new("/usr/bin/afplay")
            .arg(E1_FIXTURE_PATH)
            .spawn()
            .expect("spawn afplay");

        let deadline = tokio::time::Instant::now() + E1_CAPTURE_WINDOW;
        let mut frames = Vec::new();
        loop {
            match tokio::time::timeout_at(deadline, rx.recv()).await {
                Ok(Some(Ok(frame))) => frames.push(frame),
                Ok(Some(Err(e))) => panic!("capture error mid-stream: {e}"),
                Ok(None) | Err(_) => break,
            }
        }

        let _ = afplay.kill();
        let _ = afplay.wait();
        let _ = stream.stop();

        let frame_count = frames.len();
        assert!(
            frame_count > 0,
            "no frames received in {E1_CAPTURE_WINDOW:?} — IOProc not firing"
        );
        let peak: f32 = frames
            .iter()
            .flat_map(|f| f.samples.iter().copied().map(f32::abs))
            .fold(0.0_f32, f32::max);
        assert!(
            peak > E1_NOISE_FLOOR,
            "captured peak {peak:.4} below noise floor {E1_NOISE_FLOOR} — \
             TCC denied or tap misrouted ({frame_count} frames received)"
        );
    }
}
