// Copyright 2026 Mathews Tom
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     https://www.apache.org/licenses/LICENSE-2.0

//! Small, shared Core Audio property helpers.
//!
//! This module contains the narrow unsafe boundary used by both the
//! system-audio tap and input-device catalog. It deliberately returns native
//! Core Audio identifiers without translating them through cpal.

#![allow(unsafe_code)]

use std::ptr::NonNull;

use objc2_core_audio::{
    kAudioObjectPropertyElementMain, kAudioObjectPropertyScopeGlobal, AudioObjectGetPropertyData,
    AudioObjectID, AudioObjectPropertyAddress,
};
use objc2_core_foundation::{CFRetained, CFString};

use crate::error::MacCaptureError;

/// Convert a non-zero Core Audio status into a contextual error.
pub fn check_status(status: i32, operation: &'static str) -> Result<(), MacCaptureError> {
    if status == 0 {
        return Ok(());
    }

    Err(MacCaptureError::CoreAudio(format!(
        "{operation} failed with OSStatus {status} (0x{:08x})",
        status.cast_unsigned()
    )))
}

/// Read a global `CFString` property as UTF-8.
pub fn read_string_property(
    object_id: AudioObjectID,
    selector: u32,
    property_name: &'static str,
) -> Result<String, MacCaptureError> {
    let address = AudioObjectPropertyAddress {
        mSelector: selector,
        mScope: kAudioObjectPropertyScopeGlobal,
        mElement: kAudioObjectPropertyElementMain,
    };
    let mut cfstring: *const CFString = std::ptr::null();
    let mut size = size_of_u32::<*const CFString>("CFString pointer")?;
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
    check_status(status, "AudioObjectGetPropertyData")?;
    let Some(cfstring) = NonNull::new(cfstring.cast_mut()) else {
        return Err(MacCaptureError::CoreAudio(format!(
            "{property_name} CFString was null"
        )));
    };
    // SAFETY: Core Audio returns a retained CFString for copy properties.
    let cf = unsafe { CFRetained::from_raw(cfstring) };
    Ok(cf.to_string())
}

/// Convert a Rust type size to the Core Audio `u32` size representation.
///
/// # Errors
///
/// Returns an error when the host type's size cannot fit in Core Audio's
/// `u32` property-size parameter.
pub fn size_of_u32<T>(type_name: &'static str) -> Result<u32, MacCaptureError> {
    u32::try_from(std::mem::size_of::<T>()).map_err(|_| {
        MacCaptureError::CoreAudio(format!(
            "{type_name} size does not fit Core Audio's u32 property-size parameter"
        ))
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn check_status_zero_returns_ok() {
        assert!(check_status(0, "test").is_ok());
    }

    #[test]
    fn check_status_renders_operation_and_hex_code() {
        let err = check_status(-1, "TestOp").unwrap_err();
        assert_eq!(
            err.to_string(),
            "Core Audio error: TestOp failed with OSStatus -1 (0xffffffff)"
        );
    }
}
