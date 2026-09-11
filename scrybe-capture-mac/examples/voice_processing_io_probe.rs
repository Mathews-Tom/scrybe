// Copyright 2026 Mathews Tom
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     https://www.apache.org/licenses/LICENSE-2.0

//! Manually invocable runtime probe for Apple's `VoiceProcessingIO`
//! audio unit.
//!
//! Run with:
//!
//! ```text
//! cargo run -p scrybe-meeting-capture-mac --features voice-processing-io-probe \
//!     --example voice_processing_io_probe
//! ```
//!
//! This is a standalone diagnostic. It never starts audio I/O, never
//! opens the microphone, and is unrelated to any `[record].aec`
//! capture feature or default. Exit code `0` means the component was
//! found and fully initialized (`AudioComponentInstanceNew` +
//! `AudioUnitInitialize`) before being torn back down. Any other
//! outcome — component not found, instantiation failed,
//! initialization failed — prints a clear report and exits nonzero;
//! it never falls back to another mechanism and never claims AEC
//! works.

#[cfg(all(target_os = "macos", feature = "voice-processing-io-probe"))]
fn main() {
    use scrybe_capture_mac::voice_processing_io_probe::probe_voice_processing_io;

    match probe_voice_processing_io() {
        Ok(report) => {
            println!("{report}");
            if report.is_fully_initialized() {
                std::process::exit(0);
            }
            eprintln!("VoiceProcessingIO is unsupported on this machine.");
            std::process::exit(1);
        }
        Err(err) => {
            eprintln!("VoiceProcessingIO probe failed unexpectedly: {err}");
            std::process::exit(1);
        }
    }
}

#[cfg(not(all(target_os = "macos", feature = "voice-processing-io-probe")))]
fn main() {
    eprintln!(
        "this example requires macOS and `cargo run -p scrybe-meeting-capture-mac \
         --features voice-processing-io-probe --example voice_processing_io_probe`"
    );
    std::process::exit(1);
}
