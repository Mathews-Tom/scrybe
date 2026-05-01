// Copyright 2026 Mathews Tom
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     https://www.apache.org/licenses/LICENSE-2.0

//! Runtime backend detection for the Windows capture adapter.
//!
//! Selection follows `.docs/development-plan.md` §10.2: WASAPI loopback
//! is the primary path on Windows Vista+ for system-wide capture, and
//! `AUDIOCLIENT_PROCESS_LOOPBACK_PARAMS` per-process loopback is the
//! Windows 10 build 20348+ refinement for capturing a single
//! application without bleed from other audio sources.
//!
//! `Backend::Auto` prefers per-process loopback when the host supports
//! it (i.e. when the OS build number is 20348 or newer): the per-process
//! API isolates the captured audio from the user's own playback chain
//! and avoids the "Windows is talking to me through my own meeting"
//! feedback that system-wide loopback exhibits when the user's mic
//! routing also feeds the speakers.

/// Audio backend selected at construction time. The `Auto` variant
/// resolves to a concrete backend at `WindowsCapture::start()`.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Backend {
    #[default]
    Auto,
    WasapiLoopback,
    WasapiProcessLoopback,
}

impl Backend {
    pub const AUTO_NAME: &'static str = "auto";
    pub const WASAPI_LOOPBACK_NAME: &'static str = "wasapi-loopback";
    pub const WASAPI_PROCESS_LOOPBACK_NAME: &'static str = "wasapi-process-loopback";

    /// Parse a backend name from the `windows.audio_backend` config string.
    /// Returns `None` for unknown values so the config loader can surface
    /// a useful error rather than silently defaulting.
    #[must_use]
    pub fn from_config_str(s: &str) -> Option<Self> {
        match s {
            Self::AUTO_NAME => Some(Self::Auto),
            Self::WASAPI_LOOPBACK_NAME => Some(Self::WasapiLoopback),
            Self::WASAPI_PROCESS_LOOPBACK_NAME => Some(Self::WasapiProcessLoopback),
            _ => None,
        }
    }

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Auto => Self::AUTO_NAME,
            Self::WasapiLoopback => Self::WASAPI_LOOPBACK_NAME,
            Self::WasapiProcessLoopback => Self::WASAPI_PROCESS_LOOPBACK_NAME,
        }
    }
}

/// Minimum Windows 10 build that supports `AUDIOCLIENT_PROCESS_LOOPBACK_PARAMS`.
/// Documented at <https://learn.microsoft.com/en-us/windows/win32/api/audioclientactivationparams/ns-audioclientactivationparams-audioclient_activation_params>.
pub const PROCESS_LOOPBACK_MIN_BUILD: u32 = 20_348;

/// Minimum Windows version that supports system-wide WASAPI loopback.
/// WASAPI was introduced in Windows Vista (NT 6.0); every supported
/// Windows release qualifies.
pub const WASAPI_LOOPBACK_MIN_BUILD: u32 = 6_000;

/// Result of probing the host for available backends.
///
/// This is a pure value: the raw OS-version data on a Windows host is
/// captured into `host_build`, and `wasapi_present` /
/// `process_loopback_supported` are derived from it. Tests construct
/// the struct directly to exercise the [`ProbeResult::resolve`] decision
/// table without requiring a live Windows host.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ProbeResult {
    pub host_build: u32,
    pub wasapi_present: bool,
    pub process_loopback_supported: bool,
}

impl ProbeResult {
    /// Construct a probe result from a raw OS build number.
    ///
    /// Builds at or above [`WASAPI_LOOPBACK_MIN_BUILD`] advertise WASAPI
    /// loopback; builds at or above [`PROCESS_LOOPBACK_MIN_BUILD`] also
    /// advertise per-process loopback. A build of zero means
    /// "non-Windows host", which collapses both flags to `false` so
    /// `Backend::Auto` resolves to `None` rather than promising an
    /// unreachable backend.
    #[must_use]
    pub const fn from_build(host_build: u32) -> Self {
        Self {
            host_build,
            wasapi_present: host_build >= WASAPI_LOOPBACK_MIN_BUILD,
            process_loopback_supported: host_build >= PROCESS_LOOPBACK_MIN_BUILD,
        }
    }

    /// Resolve the requested `Backend` against the probe outcome. Returns
    /// the concrete backend that should be wired up, or `None` if the
    /// requested backend is unavailable.
    ///
    /// `Auto` resolution prefers per-process loopback over system-wide
    /// loopback when the host is new enough: per-process capture
    /// isolates the target application from the user's own playback
    /// chain, which is the behavior `system-overview.md` §3 describes
    /// for the Windows path.
    #[must_use]
    pub const fn resolve(self, requested: Backend) -> Option<Backend> {
        match requested {
            Backend::Auto => {
                if self.process_loopback_supported {
                    Some(Backend::WasapiProcessLoopback)
                } else if self.wasapi_present {
                    Some(Backend::WasapiLoopback)
                } else {
                    None
                }
            }
            Backend::WasapiLoopback if self.wasapi_present => Some(Backend::WasapiLoopback),
            Backend::WasapiProcessLoopback if self.process_loopback_supported => {
                Some(Backend::WasapiProcessLoopback)
            }
            Backend::WasapiLoopback | Backend::WasapiProcessLoopback => None,
        }
    }
}

/// Probe the host for available backends.
///
/// On Windows the OS build number is read via `RtlGetVersion` (the
/// version-lying-suppression path that ignores the application manifest
/// shim Microsoft applies to `GetVersionExW`). On every other platform
/// the function returns `ProbeResult::default()` (build 0, both flags
/// false). The non-Windows path keeps `cargo check` and the workspace
/// test suite green on macOS / Linux developer hosts and CI runners
/// where this crate is type-checked but never linked into a running
/// process.
#[must_use]
pub fn probe() -> ProbeResult {
    ProbeResult::from_build(detect_host_build())
}

/// Read the Windows build number, or return `0` on non-Windows hosts.
///
/// On Windows, calls `RtlGetVersion` directly via `extern "system"`
/// rather than `GetVersionExW` because the latter is subject to
/// application-manifest version-lying that returns a Windows 8 build
/// number unless the binary's manifest opts in to newer reporting.
/// `RtlGetVersion` is a stable documented public API in `ntdll.dll`
/// that returns the actual host build.
///
/// The `OSVERSIONINFOW` struct is declared inline rather than pulled
/// in through `windows-sys`'s `Win32_System_SystemInformation` feature
/// to keep the workspace-shared `windows-sys` feature set narrow; the
/// shape is fixed by Win32 ABI and does not change across releases.
#[cfg(all(target_os = "windows", feature = "wasapi-loopback"))]
#[allow(unsafe_code, non_snake_case)]
fn detect_host_build() -> u32 {
    use core::mem::size_of;

    #[repr(C)]
    struct OsVersionInfoW {
        dwOSVersionInfoSize: u32,
        dwMajorVersion: u32,
        dwMinorVersion: u32,
        dwBuildNumber: u32,
        dwPlatformId: u32,
        szCSDVersion: [u16; 128],
    }

    extern "system" {
        fn RtlGetVersion(lpVersionInformation: *mut OsVersionInfoW) -> i32;
    }

    let mut info = OsVersionInfoW {
        dwOSVersionInfoSize: u32::try_from(size_of::<OsVersionInfoW>()).unwrap_or(0),
        dwMajorVersion: 0,
        dwMinorVersion: 0,
        dwBuildNumber: 0,
        dwPlatformId: 0,
        szCSDVersion: [0; 128],
    };
    // SAFETY: `info` is a stack-allocated, fully-initialized
    // `OsVersionInfoW`; `dwOSVersionInfoSize` is set per the
    // documented `RtlGetVersion` contract. The function writes only
    // into the pointed-at struct.
    let status = unsafe { RtlGetVersion(&mut info) };
    if status == 0 {
        info.dwBuildNumber
    } else {
        0
    }
}

#[cfg(not(all(target_os = "windows", feature = "wasapi-loopback")))]
const fn detect_host_build() -> u32 {
    0
}

/// Resolve the configured backend against the live host. Convenience
/// wrapper around [`probe`] + [`ProbeResult::resolve`].
#[must_use]
pub fn detect(requested: Backend) -> Option<Backend> {
    probe().resolve(requested)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn test_backend_from_config_str_parses_three_supported_names() {
        assert_eq!(Backend::from_config_str("auto"), Some(Backend::Auto));
        assert_eq!(
            Backend::from_config_str("wasapi-loopback"),
            Some(Backend::WasapiLoopback),
        );
        assert_eq!(
            Backend::from_config_str("wasapi-process-loopback"),
            Some(Backend::WasapiProcessLoopback),
        );
    }

    #[test]
    fn test_backend_from_config_str_returns_none_for_unknown_value() {
        assert_eq!(Backend::from_config_str("wasapi"), None);
        assert_eq!(Backend::from_config_str(""), None);
        assert_eq!(Backend::from_config_str("WASAPI-LOOPBACK"), None);
    }

    #[test]
    fn test_backend_as_str_round_trips_through_from_config_str() {
        for backend in [
            Backend::Auto,
            Backend::WasapiLoopback,
            Backend::WasapiProcessLoopback,
        ] {
            assert_eq!(Backend::from_config_str(backend.as_str()), Some(backend));
        }
    }

    #[test]
    fn test_backend_default_is_auto() {
        assert_eq!(Backend::default(), Backend::Auto);
    }

    #[test]
    fn test_probe_result_from_build_zero_flags_no_backends_available() {
        let probed = ProbeResult::from_build(0);

        assert_eq!(probed.host_build, 0);
        assert!(!probed.wasapi_present);
        assert!(!probed.process_loopback_supported);
    }

    #[test]
    fn test_probe_result_from_build_vista_advertises_only_system_wide_loopback() {
        let probed = ProbeResult::from_build(WASAPI_LOOPBACK_MIN_BUILD);

        assert!(probed.wasapi_present);
        assert!(!probed.process_loopback_supported);
    }

    #[test]
    fn test_probe_result_from_build_windows_10_19044_advertises_only_system_wide_loopback() {
        let probed = ProbeResult::from_build(19_044);

        assert!(probed.wasapi_present);
        assert!(!probed.process_loopback_supported);
    }

    #[test]
    fn test_probe_result_from_build_windows_11_advertises_both_backends() {
        let probed = ProbeResult::from_build(22_621);

        assert!(probed.wasapi_present);
        assert!(probed.process_loopback_supported);
    }

    #[test]
    fn test_probe_result_from_build_at_min_build_advertises_per_process_loopback() {
        let probed = ProbeResult::from_build(PROCESS_LOOPBACK_MIN_BUILD);

        assert!(probed.process_loopback_supported);
    }

    #[test]
    fn test_resolve_auto_with_only_wasapi_present_picks_wasapi_loopback() {
        let probed = ProbeResult::from_build(19_044);

        assert_eq!(probed.resolve(Backend::Auto), Some(Backend::WasapiLoopback));
    }

    #[test]
    fn test_resolve_auto_with_process_loopback_supported_prefers_per_process_path() {
        let probed = ProbeResult::from_build(22_621);

        assert_eq!(
            probed.resolve(Backend::Auto),
            Some(Backend::WasapiProcessLoopback),
        );
    }

    #[test]
    fn test_resolve_auto_on_non_windows_host_returns_none() {
        let probed = ProbeResult::default();

        assert_eq!(probed.resolve(Backend::Auto), None);
    }

    #[test]
    fn test_resolve_explicit_wasapi_loopback_when_unavailable_returns_none() {
        let probed = ProbeResult::default();

        assert_eq!(probed.resolve(Backend::WasapiLoopback), None);
    }

    #[test]
    fn test_resolve_explicit_process_loopback_when_only_system_wide_supported_returns_none() {
        let probed = ProbeResult::from_build(19_044);

        assert_eq!(probed.resolve(Backend::WasapiProcessLoopback), None);
    }

    #[test]
    fn test_resolve_explicit_wasapi_loopback_when_supported_returns_wasapi_loopback() {
        let probed = ProbeResult::from_build(19_044);

        assert_eq!(
            probed.resolve(Backend::WasapiLoopback),
            Some(Backend::WasapiLoopback),
        );
    }

    #[test]
    fn test_resolve_explicit_process_loopback_when_supported_returns_per_process_path() {
        let probed = ProbeResult::from_build(22_621);

        assert_eq!(
            probed.resolve(Backend::WasapiProcessLoopback),
            Some(Backend::WasapiProcessLoopback),
        );
    }

    #[test]
    fn test_detect_on_non_windows_or_feature_disabled_returns_none() {
        // The non-windows / feature-disabled `detect_host_build` returns 0,
        // which collapses every requested backend to `None`.
        if cfg!(not(all(target_os = "windows", feature = "wasapi-loopback"))) {
            assert_eq!(detect(Backend::Auto), None);
            assert_eq!(detect(Backend::WasapiLoopback), None);
            assert_eq!(detect(Backend::WasapiProcessLoopback), None);
        }
    }

    #[test]
    fn test_backend_from_config_str_agrees_with_scrybe_core_constants() {
        assert_eq!(
            Backend::from_config_str(scrybe_core::config::WINDOWS_AUDIO_BACKEND_AUTO),
            Some(Backend::Auto),
        );
        assert_eq!(
            Backend::from_config_str(scrybe_core::config::WINDOWS_AUDIO_BACKEND_WASAPI_LOOPBACK),
            Some(Backend::WasapiLoopback),
        );
        assert_eq!(
            Backend::from_config_str(
                scrybe_core::config::WINDOWS_AUDIO_BACKEND_WASAPI_PROCESS_LOOPBACK
            ),
            Some(Backend::WasapiProcessLoopback),
        );
    }
}
