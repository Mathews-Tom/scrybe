// Copyright 2026 Mathews Tom
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     https://www.apache.org/licenses/LICENSE-2.0

//! Runtime backend detection for the Linux capture adapter.
//!
//! Selection follows `.docs/development-plan.md` §9.2: `PipeWire` is the
//! primary path on modern distros (Ubuntu 22.10+, Fedora 34+, Arch),
//! `PulseAudio` is the documented fallback for RHEL 8 / Ubuntu 20.04 LTS.
//! `Backend::Auto` walks the host's `XDG_RUNTIME_DIR` and prefers
//! `PipeWire` when both sockets are present — `PipeWire`'s own
//! `pipewire-pulse` shim usually means the Pulse socket exists on a
//! `PipeWire`-default host, so naive Pulse-first selection would miss
//! the native `PipeWire` path.

use std::path::{Path, PathBuf};

/// Audio backend selected at construction time. The `Auto` variant
/// resolves to a concrete backend at `LinuxCapture::start()`.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Backend {
    #[default]
    Auto,
    PipeWire,
    Pulse,
}

impl Backend {
    pub const PIPEWIRE_NAME: &'static str = "pipewire";
    pub const PULSE_NAME: &'static str = "pulse";
    pub const AUTO_NAME: &'static str = "auto";

    /// Parse a backend name from the `linux.audio_backend` config string.
    /// Returns `None` for unknown values so the config loader can surface
    /// a useful error rather than silently defaulting.
    #[must_use]
    pub fn from_config_str(s: &str) -> Option<Self> {
        match s {
            Self::AUTO_NAME => Some(Self::Auto),
            Self::PIPEWIRE_NAME => Some(Self::PipeWire),
            Self::PULSE_NAME => Some(Self::Pulse),
            _ => None,
        }
    }

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Auto => Self::AUTO_NAME,
            Self::PipeWire => Self::PIPEWIRE_NAME,
            Self::Pulse => Self::PULSE_NAME,
        }
    }
}

/// Result of probing the host filesystem for available backend sockets.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ProbeResult {
    pub pipewire_present: bool,
    pub pulse_present: bool,
}

impl ProbeResult {
    /// Resolve the requested `Backend` against the probe outcome. Returns
    /// the concrete backend that should be wired up, or `None` if the
    /// requested backend is unavailable.
    ///
    /// `Auto` resolution prefers `PipeWire` over Pulse: `PipeWire`'s
    /// `pipewire-pulse` shim creates a Pulse socket on `PipeWire`-default
    /// hosts, so picking Pulse first would route through the shim
    /// instead of the native API.
    #[must_use]
    pub const fn resolve(self, requested: Backend) -> Option<Backend> {
        match requested {
            Backend::Auto => {
                if self.pipewire_present {
                    Some(Backend::PipeWire)
                } else if self.pulse_present {
                    Some(Backend::Pulse)
                } else {
                    None
                }
            }
            Backend::PipeWire if self.pipewire_present => Some(Backend::PipeWire),
            Backend::Pulse if self.pulse_present => Some(Backend::Pulse),
            Backend::PipeWire | Backend::Pulse => None,
        }
    }
}

/// Probe `xdg_runtime_dir` for the `PipeWire` and `PulseAudio` Unix-domain
/// sockets that those daemons expose to user-session clients.
///
/// `PipeWire` 0.3 sockets follow the pattern `pipewire-N`; we look for
/// `pipewire-0` because that is the well-known name the libpipewire
/// `PIPEWIRE_REMOTE` default resolves to. `PulseAudio` 14+ uses
/// `pulse/native`. Both checks are file-existence only — a present
/// socket may still refuse a connection at runtime, but absence is a
/// strong negative signal that lets us fail fast with
/// `LinuxCaptureError::NoBackendAvailable` instead of waiting for the
/// daemon-side handshake to time out.
#[must_use]
pub fn probe(xdg_runtime_dir: &Path) -> ProbeResult {
    ProbeResult {
        pipewire_present: xdg_runtime_dir.join("pipewire-0").exists(),
        pulse_present: xdg_runtime_dir.join("pulse").join("native").exists(),
    }
}

/// Resolve the configured backend against the live host.
///
/// Reads `XDG_RUNTIME_DIR` via the standard env var; if unset, defaults
/// to the `/run/user/<uid>` convention that systemd ships on every
/// modern distro.
#[must_use]
pub fn detect(requested: Backend) -> Option<Backend> {
    let xdg = std::env::var_os("XDG_RUNTIME_DIR").map_or_else(
        || PathBuf::from(format!("/run/user/{}", current_uid())),
        PathBuf::from,
    );
    probe(&xdg).resolve(requested)
}

/// Read the calling process's UID without pulling in `libc` here. Linux
/// exposes `geteuid` via `/proc/self/status`; failing that, fall back to
/// `1000`, the default first-user UID on every distro.
fn current_uid() -> u32 {
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .as_deref()
        .and_then(parse_uid_from_proc_status)
        .unwrap_or(1000)
}

fn parse_uid_from_proc_status(text: &str) -> Option<u32> {
    text.lines()
        .find_map(|line| line.strip_prefix("Uid:"))
        .and_then(|tail| tail.split_whitespace().next())
        .and_then(|s| s.parse::<u32>().ok())
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use std::fs;

    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn test_backend_from_config_str_parses_three_supported_names() {
        assert_eq!(Backend::from_config_str("auto"), Some(Backend::Auto));
        assert_eq!(
            Backend::from_config_str("pipewire"),
            Some(Backend::PipeWire)
        );
        assert_eq!(Backend::from_config_str("pulse"), Some(Backend::Pulse));
    }

    #[test]
    fn test_backend_from_config_str_returns_none_for_unknown_value() {
        assert_eq!(Backend::from_config_str("alsa"), None);
        assert_eq!(Backend::from_config_str(""), None);
        assert_eq!(Backend::from_config_str("PIPEWIRE"), None);
    }

    #[test]
    fn test_backend_as_str_round_trips_through_from_config_str() {
        for backend in [Backend::Auto, Backend::PipeWire, Backend::Pulse] {
            assert_eq!(Backend::from_config_str(backend.as_str()), Some(backend));
        }
    }

    #[test]
    fn test_backend_default_is_auto() {
        assert_eq!(Backend::default(), Backend::Auto);
    }

    #[test]
    fn test_probe_returns_pipewire_present_when_socket_file_exists() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("pipewire-0"), b"").unwrap();

        let result = probe(dir.path());

        assert!(result.pipewire_present);
        assert!(!result.pulse_present);
    }

    #[test]
    fn test_probe_returns_pulse_present_when_pulse_native_socket_exists() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir(dir.path().join("pulse")).unwrap();
        fs::write(dir.path().join("pulse").join("native"), b"").unwrap();

        let result = probe(dir.path());

        assert!(!result.pipewire_present);
        assert!(result.pulse_present);
    }

    #[test]
    fn test_probe_returns_both_present_when_both_sockets_exist() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("pipewire-0"), b"").unwrap();
        fs::create_dir(dir.path().join("pulse")).unwrap();
        fs::write(dir.path().join("pulse").join("native"), b"").unwrap();

        let result = probe(dir.path());

        assert!(result.pipewire_present);
        assert!(result.pulse_present);
    }

    #[test]
    fn test_probe_returns_neither_present_on_empty_directory() {
        let dir = tempfile::tempdir().unwrap();

        let result = probe(dir.path());

        assert!(!result.pipewire_present);
        assert!(!result.pulse_present);
    }

    #[test]
    fn test_resolve_auto_with_only_pipewire_present_picks_pipewire() {
        let probed = ProbeResult {
            pipewire_present: true,
            pulse_present: false,
        };

        assert_eq!(probed.resolve(Backend::Auto), Some(Backend::PipeWire));
    }

    #[test]
    fn test_resolve_auto_with_only_pulse_present_picks_pulse() {
        let probed = ProbeResult {
            pipewire_present: false,
            pulse_present: true,
        };

        assert_eq!(probed.resolve(Backend::Auto), Some(Backend::Pulse));
    }

    #[test]
    fn test_resolve_auto_with_both_sockets_prefers_pipewire_over_pulse_shim() {
        let probed = ProbeResult {
            pipewire_present: true,
            pulse_present: true,
        };

        assert_eq!(probed.resolve(Backend::Auto), Some(Backend::PipeWire));
    }

    #[test]
    fn test_resolve_auto_with_neither_present_returns_none() {
        let probed = ProbeResult::default();

        assert_eq!(probed.resolve(Backend::Auto), None);
    }

    #[test]
    fn test_resolve_explicit_pipewire_when_socket_missing_returns_none() {
        let probed = ProbeResult {
            pipewire_present: false,
            pulse_present: true,
        };

        assert_eq!(probed.resolve(Backend::PipeWire), None);
    }

    #[test]
    fn test_resolve_explicit_pulse_when_socket_missing_returns_none() {
        let probed = ProbeResult {
            pipewire_present: true,
            pulse_present: false,
        };

        assert_eq!(probed.resolve(Backend::Pulse), None);
    }

    #[test]
    fn test_resolve_explicit_pipewire_when_socket_present_returns_pipewire() {
        let probed = ProbeResult {
            pipewire_present: true,
            pulse_present: false,
        };

        assert_eq!(probed.resolve(Backend::PipeWire), Some(Backend::PipeWire));
    }

    #[test]
    fn test_resolve_explicit_pulse_when_socket_present_returns_pulse() {
        let probed = ProbeResult {
            pipewire_present: false,
            pulse_present: true,
        };

        assert_eq!(probed.resolve(Backend::Pulse), Some(Backend::Pulse));
    }

    #[test]
    fn test_parse_uid_from_proc_status_extracts_real_uid_field() {
        let sample = "Name:\tcat\nUmask:\t0022\nState:\tR (running)\n\
                      Uid:\t1000\t1000\t1000\t1000\nGid:\t1000\t1000\t1000\t1000\n";

        assert_eq!(parse_uid_from_proc_status(sample), Some(1000));
    }

    #[test]
    fn test_parse_uid_from_proc_status_returns_none_when_uid_field_missing() {
        let sample = "Name:\tinit\nState:\tS\n";

        assert_eq!(parse_uid_from_proc_status(sample), None);
    }

    #[test]
    fn test_parse_uid_from_proc_status_returns_none_for_non_numeric_uid() {
        let sample = "Uid:\troot\troot\troot\troot\n";

        assert_eq!(parse_uid_from_proc_status(sample), None);
    }
}
