// Copyright 2026 Mathews Tom
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     https://www.apache.org/licenses/LICENSE-2.0

//! Standalone runtime probe for Apple's `VoiceProcessingIO` audio unit.
//!
//! This module is diagnostic-only. It is **not** the `[record].aec`
//! product feature, does not touch [`crate::NativeMicCapture`] or any
//! microphone stream, and does not change capture behavior, config,
//! defaults, or lifecycle. It answers exactly one question: *is the
//! `auou`/`vpio`/`appl` audio component present and initializable on
//! this machine right now?*
//!
//! The probe performs Apple's documented component lifecycle and
//! nothing else:
//!
//! 1. [`AudioComponentFindNext`] searches for the component matching
//!    `componentType = auou`, `componentSubType = vpio`,
//!    `componentManufacturer = appl` — the exact triple Apple
//!    documents for `kAudioUnitSubType_VoiceProcessingIO`.
//! 2. [`AudioComponentInstanceNew`] creates an instance.
//! 3. [`AudioUnitInitialize`] fully initializes it — no properties or
//!    callbacks are configured, and no output/input stream is
//!    started.
//! 4. [`InstanceGuard::drop`] unconditionally tears the instance back
//!    down ([`AudioUnitUninitialize`] when initialized, then always
//!    [`AudioComponentInstanceDispose`]) on every path out of
//!    [`probe_voice_processing_io`], including early returns.
//!
//! A fully initialized unit establishes *runtime availability*, not
//! capture readiness or AEC quality — the probe never claims AEC
//! "works". Any failure at any step is reported as unsupported/not
//! found; there is no fallback path.

#![allow(unsafe_code)]

use std::ptr::NonNull;

use objc2_audio_toolbox::{
    kAudioUnitManufacturer_Apple, kAudioUnitSubType_VoiceProcessingIO, kAudioUnitType_Output,
    AudioComponentDescription, AudioComponentFindNext, AudioComponentInstance,
    AudioComponentInstanceDispose, AudioComponentInstanceNew, AudioUnit, AudioUnitInitialize,
    AudioUnitUninitialize,
};

use crate::error::MacCaptureError;

/// Absolute path to the `sw_vers` system utility.
///
/// Invoked by absolute path (rather than via `$PATH`) since this is a
/// security-relevant diagnostic that should not be influenced by an
/// attacker-controlled environment.
const SW_VERS_PATH: &str = "/usr/bin/sw_vers";

/// Outcome of [`AudioComponentFindNext`] for the `VoiceProcessingIO`
/// component triple.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ComponentLookup {
    /// A matching component was found in the system's component
    /// registry.
    Found,
    /// No component matches `auou`/`vpio`/`appl` on this machine.
    NotFound,
}

/// Outcome of attempting to create and fully initialize an instance
/// of the located component.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InitializationResult {
    /// `AudioComponentInstanceNew` and `AudioUnitInitialize` both
    /// returned `noErr`. The instance was disposed immediately
    /// afterward; this variant reports availability only, not an
    /// open capture session.
    Initialized,
    /// The component was not found, so instantiation was never
    /// attempted.
    NotAttempted,
    /// `AudioComponentInstanceNew` returned a nonzero `OSStatus`.
    InstantiationFailed { status: i32 },
    /// `AudioUnitInitialize` returned a nonzero `OSStatus`. The
    /// (uninitialized) instance was still disposed.
    InitializationFailed { status: i32 },
}

/// Report produced by [`probe_voice_processing_io`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VoiceProcessingIoProbeReport {
    /// Exact macOS product version, e.g. `"15.1 (24B83)"`, or a
    /// descriptive placeholder if `sw_vers` could not be run.
    pub macos_version: String,
    /// Whether the component itself was found.
    pub component_lookup: ComponentLookup,
    /// Whether a found component could be fully initialized.
    pub initialization: InitializationResult,
}

impl VoiceProcessingIoProbeReport {
    /// True only when the component was found and fully initialized.
    /// This is the sole condition establishing `VoiceProcessingIO`
    /// runtime availability; every other outcome is unsupported.
    #[must_use]
    pub const fn is_fully_initialized(&self) -> bool {
        matches!(self.initialization, InitializationResult::Initialized)
    }
}

impl std::fmt::Display for VoiceProcessingIoProbeReport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(f, "macOS version: {}", self.macos_version)?;
        let triple = format!(
            "{}/{}/{}",
            fourcc(kAudioUnitType_Output),
            fourcc(kAudioUnitSubType_VoiceProcessingIO),
            fourcc(kAudioUnitManufacturer_Apple)
        );
        match self.component_lookup {
            ComponentLookup::Found => {
                writeln!(f, "component lookup ({triple}): found")?;
            }
            ComponentLookup::NotFound => {
                writeln!(f, "component lookup ({triple}): not found")?;
            }
        }
        match self.initialization {
            InitializationResult::Initialized => {
                write!(f, "initialization: fully initialized, then torn down")
            }
            InitializationResult::NotAttempted => {
                write!(f, "initialization: not attempted (component not found)")
            }
            InitializationResult::InstantiationFailed { status } => write!(
                f,
                "initialization: unsupported — {}",
                describe_status(status, "AudioComponentInstanceNew")
            ),
            InitializationResult::InitializationFailed { status } => write!(
                f,
                "initialization: unsupported — {}",
                describe_status(status, "AudioUnitInitialize")
            ),
        }
    }
}

/// RAII guard that unconditionally tears the `AudioComponentInstance`
/// back down exactly once, regardless of which path leaves it in
/// scope. This is what makes "dispose on every post-create path"
/// structural rather than something each branch must remember to do.
struct InstanceGuard {
    instance: AudioUnit,
    initialized: bool,
}

impl Drop for InstanceGuard {
    fn drop(&mut self) {
        if self.initialized {
            // SAFETY: `self.instance` was returned by a successful
            // `AudioComponentInstanceNew` and successfully
            // initialized by `AudioUnitInitialize`, and has not been
            // disposed yet. Teardown is best-effort here — there is
            // no further path to surface a nonzero status from
            // `Drop`.
            let _ = unsafe { AudioUnitUninitialize(self.instance) };
        }
        // SAFETY: `self.instance` was returned by a successful
        // `AudioComponentInstanceNew` call and is disposed exactly
        // once, here.
        let _ = unsafe { AudioComponentInstanceDispose(self.instance) };
    }
}

/// Probes for Apple's `VoiceProcessingIO` audio unit component,
/// fully initializes an instance if found, then tears it down.
///
/// This never starts audio I/O and never takes ownership of the live
/// microphone: no properties (including the voice-processing bypass
/// or AGC properties) and no render/input callbacks are ever set.
///
/// # Errors
///
/// Returns [`MacCaptureError::CoreAudio`] only for conditions outside
/// the documented lifecycle — e.g. a successful `noErr` that still
/// hands back a null instance pointer. Ordinary unsupported outcomes
/// (component not found, instantiation failed, initialization
/// failed) are reported through [`VoiceProcessingIoProbeReport`], not
/// as an `Err`.
pub fn probe_voice_processing_io() -> Result<VoiceProcessingIoProbeReport, MacCaptureError> {
    let macos_version = macos_product_version();

    let description = AudioComponentDescription {
        componentType: kAudioUnitType_Output,
        componentSubType: kAudioUnitSubType_VoiceProcessingIO,
        componentManufacturer: kAudioUnitManufacturer_Apple,
        componentFlags: 0,
        componentFlagsMask: 0,
    };

    // SAFETY: `description` is a valid, fully-initialized, stack-local
    // `AudioComponentDescription`. Passing a null `in_component`
    // starts the search from the beginning of the component registry.
    let found =
        unsafe { AudioComponentFindNext(std::ptr::null_mut(), NonNull::from(&description)) };

    let Some(component) = NonNull::new(found) else {
        return Ok(VoiceProcessingIoProbeReport {
            macos_version,
            component_lookup: ComponentLookup::NotFound,
            initialization: InitializationResult::NotAttempted,
        });
    };

    let mut raw_instance: AudioComponentInstance = std::ptr::null_mut();
    // SAFETY: `component` was just returned by `AudioComponentFindNext`
    // and is non-null; `raw_instance` is a valid local out-pointer.
    let new_status =
        unsafe { AudioComponentInstanceNew(component.as_ptr(), NonNull::from(&mut raw_instance)) };
    if new_status != 0 {
        return Ok(VoiceProcessingIoProbeReport {
            macos_version,
            component_lookup: ComponentLookup::Found,
            initialization: InitializationResult::InstantiationFailed { status: new_status },
        });
    }
    let Some(instance) = NonNull::new(raw_instance) else {
        return Err(MacCaptureError::CoreAudio(
            "AudioComponentInstanceNew reported noErr but returned a null instance".to_string(),
        ));
    };

    // From this point on, `guard` guarantees disposal — and, if
    // initialization below succeeds, uninitialization first — on
    // every remaining return path.
    let mut guard = InstanceGuard {
        instance: instance.as_ptr(),
        initialized: false,
    };

    // SAFETY: `guard.instance` is the live instance created above and
    // has not yet been initialized or disposed.
    let init_status = unsafe { AudioUnitInitialize(guard.instance) };
    if init_status != 0 {
        return Ok(VoiceProcessingIoProbeReport {
            macos_version,
            component_lookup: ComponentLookup::Found,
            initialization: InitializationResult::InitializationFailed {
                status: init_status,
            },
        });
    }
    guard.initialized = true;

    Ok(VoiceProcessingIoProbeReport {
        macos_version,
        component_lookup: ComponentLookup::Found,
        initialization: InitializationResult::Initialized,
    })
}

/// Render a nonzero `OSStatus` the same way `scrybe-capture-mac`'s
/// other Core Audio call sites do: decimal plus zero-padded hex.
fn describe_status(status: i32, operation: &'static str) -> String {
    format!(
        "{operation} failed with OSStatus {status} (0x{:08x})",
        status.cast_unsigned()
    )
}

/// Render the component-triple `u32` codes Core Audio uses as their
/// four-character ASCII form, e.g. `0x6170_706c` -> `"appl"`.
fn fourcc(code: u32) -> String {
    code.to_be_bytes()
        .into_iter()
        .map(|byte| {
            if byte.is_ascii_graphic() {
                byte as char
            } else {
                '.'
            }
        })
        .collect()
}

/// Exact macOS product version, e.g. `"15.1 (24B83)"`.
///
/// Shells out to `sw_vers` rather than binding `NSProcessInfo`,
/// keeping this probe's dependency footprint limited to
/// `objc2-audio-toolbox`. Degrades to a descriptive placeholder
/// (never panics or fails the probe) if `sw_vers` is unavailable.
fn macos_product_version() -> String {
    match (
        run_command_trim(SW_VERS_PATH, "-productVersion"),
        run_command_trim(SW_VERS_PATH, "-buildVersion"),
    ) {
        (Some(product), Some(build)) => format!("{product} ({build})"),
        (Some(product), None) => product,
        (None, _) => format!("unknown ({SW_VERS_PATH} unavailable)"),
    }
}

/// Run `program arg`, returning its trimmed stdout on a zero exit
/// status with non-empty output, `None` otherwise.
fn run_command_trim(program: &str, arg: &str) -> Option<String> {
    let output = std::process::Command::new(program).arg(arg).output().ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8(output.stdout).ok()?;
    let trimmed = text.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::*;

    #[test]
    fn fourcc_decodes_the_voice_processing_io_component_triple() {
        assert_eq!(fourcc(kAudioUnitType_Output), "auou");
        assert_eq!(fourcc(kAudioUnitSubType_VoiceProcessingIO), "vpio");
        assert_eq!(fourcc(kAudioUnitManufacturer_Apple), "appl");
    }

    #[test]
    fn fourcc_renders_non_printable_bytes_as_dots() {
        assert_eq!(fourcc(0x0000_00FF), "....");
    }

    #[test]
    fn describe_status_renders_operation_decimal_and_hex() {
        let rendered = describe_status(-10877, "AudioUnitInitialize");
        assert_eq!(
            rendered,
            "AudioUnitInitialize failed with OSStatus -10877 (0xffffd583)"
        );
    }

    #[test]
    fn report_is_fully_initialized_only_for_the_initialized_variant() {
        let base = VoiceProcessingIoProbeReport {
            macos_version: "15.1 (24B83)".to_string(),
            component_lookup: ComponentLookup::Found,
            initialization: InitializationResult::Initialized,
        };
        assert!(base.is_fully_initialized());

        let not_found = VoiceProcessingIoProbeReport {
            component_lookup: ComponentLookup::NotFound,
            initialization: InitializationResult::NotAttempted,
            ..base.clone()
        };
        assert!(!not_found.is_fully_initialized());

        let init_failed = VoiceProcessingIoProbeReport {
            initialization: InitializationResult::InitializationFailed { status: -10877 },
            ..base
        };
        assert!(!init_failed.is_fully_initialized());
    }

    #[test]
    fn report_display_renders_every_field() {
        let report = VoiceProcessingIoProbeReport {
            macos_version: "15.1 (24B83)".to_string(),
            component_lookup: ComponentLookup::NotFound,
            initialization: InitializationResult::NotAttempted,
        };
        let rendered = report.to_string();
        assert!(rendered.contains("macOS version: 15.1 (24B83)"));
        assert!(rendered.contains("component lookup (auou/vpio/appl): not found"));
        assert!(rendered.contains("initialization: not attempted (component not found)"));
    }

    #[test]
    fn run_command_trim_returns_none_for_a_nonexistent_program() {
        assert_eq!(
            run_command_trim("/definitely/does/not/exist/on/any/machine", "-x"),
            None
        );
    }

    #[test]
    fn run_command_trim_returns_none_on_nonzero_exit() {
        assert_eq!(run_command_trim("/usr/bin/false", ""), None);
    }

    #[test]
    fn run_command_trim_returns_trimmed_stdout_on_success() {
        assert_eq!(
            run_command_trim("/bin/echo", "  hello  "),
            Some("hello".to_string())
        );
    }
}
