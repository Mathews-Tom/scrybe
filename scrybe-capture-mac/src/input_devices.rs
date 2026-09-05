// Copyright 2026 Mathews Tom
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     https://www.apache.org/licenses/LICENSE-2.0

//! Native Core Audio input-device discovery.
//!
//! `uid` is the stable identifier accepted by M4's `--input-device` flag.
//! The display name is presentation-only: duplicate names remain distinct
//! because the catalog never resolves a device back from its name.

#![allow(unsafe_code)]

use std::ptr::NonNull;

use objc2_core_audio::{
    kAudioDevicePropertyDeviceUID, kAudioDevicePropertyStreams,
    kAudioHardwarePropertyDefaultInputDevice, kAudioHardwarePropertyDevices,
    kAudioObjectPropertyElementMain, kAudioObjectPropertyName, kAudioObjectPropertyScopeGlobal,
    kAudioObjectPropertyScopeInput, kAudioObjectSystemObject, AudioObjectGetPropertyData,
    AudioObjectGetPropertyDataSize, AudioObjectID, AudioObjectPropertyAddress,
};

use crate::coreaudio::{check_status, read_string_property, size_of_u32};
use crate::error::MacCaptureError;

/// A Core Audio input device identified independently of its display name.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InputDevice {
    /// Persistent Core Audio device UID. Pass this value unmodified to
    /// `scrybe rec --input-device`.
    pub uid: String,
    /// User-facing Core Audio device name. It is not a selector.
    pub name: String,
    /// Whether this is the current Core Audio default input device.
    pub is_default: bool,
}

/// Enumerate every Core Audio device with one or more input streams.
///
/// The result is sorted by UID so command output is stable. A property failure
/// is returned rather than dropping a device from the catalog: partial output
/// could make a valid UID look unavailable.
///
/// # Errors
///
/// Returns an error when Core Audio cannot enumerate a property completely.
pub fn input_devices() -> Result<Vec<InputDevice>, MacCaptureError> {
    let default_device = default_input_device()?;
    let mut devices = all_devices()?
        .into_iter()
        .filter_map(|device_id| match device_has_input_stream(device_id) {
            Ok(true) => Some(Ok(device_id)),
            Ok(false) => None,
            Err(error) => Some(Err(error)),
        })
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .map(|device_id| {
            Ok(InputDevice {
                uid: read_string_property(device_id, kAudioDevicePropertyDeviceUID, "device UID")?,
                name: read_string_property(device_id, kAudioObjectPropertyName, "device name")?,
                is_default: device_id == default_device,
            })
        })
        .collect::<Result<Vec<_>, MacCaptureError>>()?;
    devices.sort_by(|left, right| left.uid.cmp(&right.uid));
    Ok(devices)
}

fn default_input_device() -> Result<AudioObjectID, MacCaptureError> {
    let address = AudioObjectPropertyAddress {
        mSelector: kAudioHardwarePropertyDefaultInputDevice,
        mScope: kAudioObjectPropertyScopeGlobal,
        mElement: kAudioObjectPropertyElementMain,
    };
    let mut device_id = 0;
    let mut size = size_of_u32::<AudioObjectID>("AudioObjectID")?;
    let status = unsafe {
        AudioObjectGetPropertyData(
            kAudioObjectSystemObject as AudioObjectID,
            NonNull::from(&address),
            0,
            std::ptr::null(),
            NonNull::from(&mut size),
            NonNull::from(&mut device_id).cast(),
        )
    };
    check_status(status, "AudioObjectGetPropertyData(DefaultInputDevice)")?;
    if device_id == 0 {
        return Err(MacCaptureError::CoreAudio(
            "no default input device is configured".to_string(),
        ));
    }
    Ok(device_id)
}

fn all_devices() -> Result<Vec<AudioObjectID>, MacCaptureError> {
    let address = AudioObjectPropertyAddress {
        mSelector: kAudioHardwarePropertyDevices,
        mScope: kAudioObjectPropertyScopeGlobal,
        mElement: kAudioObjectPropertyElementMain,
    };
    let mut size = 0;
    let status = unsafe {
        AudioObjectGetPropertyDataSize(
            kAudioObjectSystemObject as AudioObjectID,
            NonNull::from(&address),
            0,
            std::ptr::null(),
            NonNull::from(&mut size),
        )
    };
    check_status(status, "AudioObjectGetPropertyDataSize(Devices)")?;
    let id_size = size_of_u32::<AudioObjectID>("AudioObjectID")?;
    if size % id_size != 0 {
        return Err(MacCaptureError::CoreAudio(format!(
            "Core Audio reported {size} bytes for an AudioObjectID list"
        )));
    }

    if size == 0 {
        return Ok(Vec::new());
    }
    let mut devices = vec![0; (size / id_size) as usize];
    let Some(data) = NonNull::new(devices.as_mut_ptr()) else {
        return Err(MacCaptureError::CoreAudio(
            "allocation for Core Audio device list returned a null pointer".to_string(),
        ));
    };
    let status = unsafe {
        AudioObjectGetPropertyData(
            kAudioObjectSystemObject as AudioObjectID,
            NonNull::from(&address),
            0,
            std::ptr::null(),
            NonNull::from(&mut size),
            data.cast(),
        )
    };
    check_status(status, "AudioObjectGetPropertyData(Devices)")?;
    Ok(devices)
}

fn device_has_input_stream(device_id: AudioObjectID) -> Result<bool, MacCaptureError> {
    let address = AudioObjectPropertyAddress {
        mSelector: kAudioDevicePropertyStreams,
        mScope: kAudioObjectPropertyScopeInput,
        mElement: kAudioObjectPropertyElementMain,
    };
    let mut size = 0;
    let status = unsafe {
        AudioObjectGetPropertyDataSize(
            device_id,
            NonNull::from(&address),
            0,
            std::ptr::null(),
            NonNull::from(&mut size),
        )
    };
    check_status(status, "AudioObjectGetPropertyDataSize(InputStreams)")?;
    Ok(size > 0)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn input_device_retains_uid_when_display_names_match() {
        let first = InputDevice {
            uid: "AppleHDAEngineInput:1F,3,0,1,0:1".to_string(),
            name: "MacBook Pro Microphone".to_string(),
            is_default: true,
        };
        let second = InputDevice {
            uid: "USB:0x1234:0x5678:1".to_string(),
            name: "MacBook Pro Microphone".to_string(),
            is_default: false,
        };

        assert_ne!(first.uid, second.uid);
        assert_eq!(first.name, second.name);
    }

    #[test]
    fn input_device_default_marker_belongs_to_device_not_name() {
        let device = InputDevice {
            uid: "uid-a".to_string(),
            name: "Shared Name".to_string(),
            is_default: true,
        };

        assert!(device.is_default);
        assert_eq!(device.uid, "uid-a");
    }
}
