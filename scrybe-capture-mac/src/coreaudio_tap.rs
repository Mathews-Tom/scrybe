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
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use block2::RcBlock;
use objc2::rc::Retained;
use objc2::AllocAnyThread;
use objc2_core_audio::{
    kAudioAggregateDeviceIsPrivateKey, kAudioAggregateDeviceIsStackedKey,
    kAudioAggregateDeviceNameKey, kAudioAggregateDeviceTapListKey, kAudioAggregateDeviceUIDKey,
    kAudioObjectPropertyElementMain, kAudioObjectPropertyScopeGlobal, kAudioSubTapUIDKey,
    kAudioTapPropertyFormat, kAudioTapPropertyUID, AudioDeviceCreateIOProcIDWithBlock,
    AudioDeviceDestroyIOProcID, AudioDeviceIOProcID, AudioDeviceStart, AudioDeviceStop,
    AudioHardwareCreateAggregateDevice, AudioHardwareCreateProcessTap,
    AudioHardwareDestroyAggregateDevice, AudioHardwareDestroyProcessTap,
    AudioObjectGetPropertyData, AudioObjectID, AudioObjectPropertyAddress, CATapDescription,
};
use objc2_core_audio_types::{AudioBufferList, AudioStreamBasicDescription};
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
    /// tests; not currently surfaced through `MacCapture::capabilities`
    /// because capabilities are reported pre-start.
    pub(crate) sample_rate: u32,
    pub(crate) channels: u16,
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

use objc2_core_audio_types::AudioTimeStamp;

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

        // 2. Create the process tap.
        let mut tap_id: AudioObjectID = 0;
        let status =
            unsafe { AudioHardwareCreateProcessTap(Some(&tap_description), &raw mut tap_id) };
        check_status(status, "AudioHardwareCreateProcessTap")?;
        debug!(tap_id, "process tap created");

        // 3a. Read the tap's UID for the aggregate-device dictionary.
        let tap_uid = read_object_uid(tap_id, kAudioTapPropertyUID).map_err(|e| {
            // tap leaked if we early-return; release it before returning
            unsafe {
                let _ = AudioHardwareDestroyProcessTap(tap_id);
            }
            e
        })?;

        // 3b. Read the tap's stream format so we can advertise the
        // correct sample rate and channel count to scrybe-core.
        let format = read_tap_format(tap_id).map_err(|e| {
            unsafe {
                let _ = AudioHardwareDestroyProcessTap(tap_id);
            }
            e
        })?;
        let sample_rate = format.mSampleRate as u32;
        let channels = u16::try_from(format.mChannelsPerFrame).unwrap_or(2);
        debug!(sample_rate, channels, "tap stream format negotiated");

        // 4. Aggregate device dictionary with the tap as a sub-tap.
        let aggregate_device_dict = build_aggregate_device_dict(&tap_uid).map_err(|e| {
            unsafe {
                let _ = AudioHardwareDestroyProcessTap(tap_id);
            }
            e
        })?;
        let mut aggregate_device_id: AudioObjectID = 0;
        let status = unsafe {
            AudioHardwareCreateAggregateDevice(
                &aggregate_device_dict,
                NonNull::from(&mut aggregate_device_id),
            )
        };
        if let Err(e) = check_status(status, "AudioHardwareCreateAggregateDevice") {
            unsafe {
                let _ = AudioHardwareDestroyProcessTap(tap_id);
            }
            return Err(e);
        }
        debug!(aggregate_device_id, "aggregate device created");

        // 5. Install the IO block. The block captures the shared sender
        // and the negotiated format so it can construct AudioFrames
        // without re-reading CoreAudio properties on every callback.
        let block_sender = Arc::clone(&shared_sender);
        let sample_counter = Arc::new(AtomicU64::new(0));
        let block_counter = Arc::clone(&sample_counter);
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
                let prior = block_counter.load(Ordering::Relaxed);
                let timestamp_ns = prior
                    .saturating_mul(1_000_000_000)
                    .checked_div(u64::from(sample_rate.max(1)))
                    .unwrap_or(0);
                let frames_added =
                    u64::try_from(samples.len()).unwrap_or(0) / u64::from(channels.max(1));
                block_counter.fetch_add(frames_added, Ordering::Relaxed);
                let frame = AudioFrame::from_slice(
                    &samples,
                    channels,
                    sample_rate,
                    timestamp_ns,
                    FrameSource::System,
                );
                if let Ok(guard) = block_sender.lock() {
                    if let Some(tx) = guard.as_ref() {
                        if tx.send(Ok(frame)).is_err() {
                            // Receiver was dropped — capture has been
                            // stopped; nothing else to do.
                        }
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
        if let Err(e) = check_status(status, "AudioDeviceCreateIOProcIDWithBlock") {
            unsafe {
                let _ = AudioHardwareDestroyAggregateDevice(aggregate_device_id);
                let _ = AudioHardwareDestroyProcessTap(tap_id);
            }
            return Err(e);
        }
        debug!("IO proc id installed on aggregate device");

        Ok(Self {
            tap_id,
            aggregate_device_id,
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
            // sees an end-of-stream.
            if let Ok(mut guard) = self.sender.lock() {
                guard.take();
            }
            return Ok(());
        }
        let status = unsafe { AudioDeviceStop(self.aggregate_device_id, self.io_proc_id) };
        self.started = false;
        if let Ok(mut guard) = self.sender.lock() {
            guard.take();
        }
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
/// `list` must point at a CoreAudio-owned [`AudioBufferList`] whose
/// `mBuffers` array has at least `mNumberBuffers` elements and whose
/// `mData` pointers are valid for `mDataByteSize` bytes.
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
/// `AudioHardwareCreateAggregateDevice` consumes. We build it as an
/// `NSDictionary` because CFDictionary toll-free bridges with
/// NSDictionary on Apple platforms; the cast at the bottom is the
/// documented bridging path.
fn build_aggregate_device_dict(tap_uid: &str) -> Result<CFRetained<CFDictionary>, MacCaptureError> {
    let aggregate_uid_value = NSString::from_str(&format!("dev.scrybe.tap-aggregate.{tap_uid}"));
    let aggregate_name_value = NSString::from_str("scrybe-tap-aggregate");
    let one = NSNumber::new_i32(1);
    let zero = NSNumber::new_i32(0);

    let sub_tap_dict: Retained<NSDictionary<NSString, NSString>> = {
        let sub_key = NSString::from_str(c_str_to_str(kAudioSubTapUIDKey));
        let sub_value = NSString::from_str(tap_uid);
        let keys: [&NSString; 1] = [&sub_key];
        let values: [&NSString; 1] = [&sub_value];
        NSDictionary::from_slices(&keys, &values)
    };
    let tap_list: Retained<NSArray<NSDictionary<NSString, NSString>>> =
        NSArray::from_retained_slice(&[sub_tap_dict]);

    let key_uid = NSString::from_str(c_str_to_str(kAudioAggregateDeviceUIDKey));
    let key_name = NSString::from_str(c_str_to_str(kAudioAggregateDeviceNameKey));
    let key_private = NSString::from_str(c_str_to_str(kAudioAggregateDeviceIsPrivateKey));
    let key_stacked = NSString::from_str(c_str_to_str(kAudioAggregateDeviceIsStackedKey));
    let key_taps = NSString::from_str(c_str_to_str(kAudioAggregateDeviceTapListKey));

    let keys: [&NSString; 5] = [&key_uid, &key_name, &key_private, &key_stacked, &key_taps];
    // Promote each value to its NSObject super so the dictionary value
    // type is homogeneous. NSNumber → NSValue → NSObject;
    // NSString/NSArray inherit from NSObject directly.
    let v_uid: Retained<NSObject> = aggregate_uid_value.into_super();
    let v_name: Retained<NSObject> = aggregate_name_value.into_super();
    let v_private: Retained<NSObject> = one.into_super().into_super();
    let v_stacked: Retained<NSObject> = zero.into_super().into_super();
    let v_taps: Retained<NSObject> = tap_list.into_super();
    let value_refs: [&NSObject; 5] = [&v_uid, &v_name, &v_private, &v_stacked, &v_taps];
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

fn c_str_to_str(c: &CStr) -> &str {
    c.to_str().unwrap_or("")
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
}
