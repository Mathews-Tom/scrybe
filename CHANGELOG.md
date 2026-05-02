# Changelog

All notable changes to scrybe are documented here. The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html) within the stability tiers documented in `docs/system-design.md` §12.

## [0.5.0] — 2026-05-02

Android capture trait surface and the neural diarizer fallback. v0.5.0 delivers two architectural seams per `.docs/development-plan.md` §11: the new `scrybe-android` crate (cdylib + rlib) implementing `scrybe-core`'s `AudioCapture` trait via the `MediaProjection` primary path with a `MicOnly` fallback, and the `PyannoteOnnxDiarizer` neural-diarizer fallback in `scrybe-core` for multi-party / in-room calls that the binary-channel heuristic cannot resolve. Both follow the macOS-first / Linux-first / Windows-first scaffold pattern: trait surface, runtime detection, and config wiring ship in this release; the live `MediaProjection` JNI binding (and the uniffi-generated Kotlin facade for the Compose UI shell) plus the live ONNX runtime binding are tracked as v0.5.x follow-ups.

The publish posture from v0.1.0 / v0.2.0 / v0.3.0 / v0.4.0 carries forward unchanged: only `scrybe` (the placeholder) publishes to crates.io. `scrybe-core`, `scrybe-capture-mac`, `scrybe-capture-linux`, `scrybe-capture-win`, `scrybe-android`, and `scrybe-cli` stay workspace-private (`publish = false`).

### What you can actually do at v0.5.0

- Everything you could at v0.4.0 (macOS Core Audio Taps capture, local whisper-rs, local Ollama, OpenAI-compatible cloud STT/LLM, ICS calendar context, signed webhooks, `Hook::Git` auto-commit, channel-split `BinaryChannelDiarizer`, `transcript.partial.jsonl` write-ahead log, Linux + Windows `audio_backend` selectors, ParakeetLocalProvider seam), plus:
- Compose against the `scrybe_android::AudioCapture` impl from any host. The `AndroidCapture` adapter implements the same `start`/`stop`/`frames`/`capabilities` contract as `MacCapture`, `LinuxCapture`, and `WindowsCapture`, with the same `Arc<Mutex<SharedState>>` ownership shape and the same `inject_for_test` / `close_for_test` integration-test surface.
- Choose a backend via `android.audio_backend = "auto" | "media-projection" | "mic-only"` in `config.toml`. Default is `"auto"`, which the runtime resolves against the host's API level and `MediaProjection` consent state. Auto prefers `MediaProjection` over `MicOnly` when both are available; a host on API 28 or with declined consent collapses to `Backend::MicOnly`; a non-Android host collapses to `Backend::Auto → None`, which surfaces as `DeviceUnavailable` with the `NoBackendAvailable` rationale.
- Distinguish a "needs Android 10+" host from a fully unsupported host: an explicit `android.audio_backend = "media-projection"` on API 28 returns `RequestedBackendUnavailable { requested: "media-projection" }` rather than silently falling through to mic-only.
- Reference `PyannoteOnnxDiarizer` and `PyannoteBackend` from any consumer of `scrybe_core::diarize`. The diarizer is generic over the backend so the cluster-to-attendee mapping is exercised by an in-tree stub backend on every PR; the live ONNX runtime binding sits behind the new `diarize-pyannote` cargo feature and lands as a v0.5.x follow-up. Without the feature, `PyannoteOnnxDiarizer::new_live` returns `CoreError::Pipeline(PipelineError::DiarizerUnavailable)` rather than a phantom successful load.
- Configure the diarizer via the new `[diarizer]` block: `kind = "auto" | "binary-channel" | "pyannote-onnx"`. `auto` (the default) routes via `scrybe_core::diarize::select_kind` against the live `Capabilities` and `MeetingContext` per `system-design.md` §4.4 — multi-party (≥3 attendees) or single-channel calls go to the neural fallback; everything else stays on the binary-channel default. Explicit user choice always wins over the auto-rule.

### Added

- `scrybe-android` crate (cdylib + rlib, under the 2 500-LoC ceiling enforced by `scripts/check-loc-budget.py`). New workspace member listed in the root `Cargo.toml`.
- `scrybe_android::AndroidCapture` implementing `scrybe_core::capture::AudioCapture`. Mirrors the `MacCapture`, `LinuxCapture`, and `WindowsCapture` shapes — `Arc<Mutex<SharedState>>` ownership, `tokio::sync::mpsc::unbounded_channel` plumbing, single-consumer `frames()` semantics. `start()` / `stop()` are idempotent; calling `start()` after `stop()` returns `DeviceUnavailable` (the adapter is single-use across a `start → stop → start` cycle, per the existing adapter precedent). `inject_for_test` / `close_for_test` test surfaces match the desktop adapters byte-for-byte.
- `scrybe_android::backend::{Backend, ProbeResult, probe, detect, MEDIA_PROJECTION_MIN_API}`. `Backend` is a three-variant enum (`Auto`/`MediaProjection`/`MicOnly`) with a `from_config_str` parser and an `as_str` round-trip. `ProbeResult::from_api_level(api_level, consent_granted)` derives availability flags from the host API level and the consent-prompt outcome; tests construct it directly to exercise resolution against any host shape without requiring a live Android runner. `detect()` returns `None` on non-Android hosts and on Android hosts when the live JNI probe is not yet wired up; the resolution table collapses to `None` rather than promising an unreachable backend.
- `scrybe_android::error::AndroidCaptureError` — adapter-local error type with `MediaProjectionDisabled`, `MicOnlyDisabled`, `NoBackendAvailable`, `RequestedBackendUnavailable { requested }`, `MediaProjectionRequiresNewerApi { api_level }`, and `UserDeclinedConsent`. `UserDeclinedConsent` promotes to `CaptureError::PermissionDenied` so the consent-decline path surfaces as a permission failure rather than a generic device-unavailable; every other variant promotes to `CaptureError::DeviceUnavailable`, mirroring the adapter pattern in `docs/system-design.md` §4.6.
- `scrybe_core::diarize::pyannote_onnx::{PyannoteOnnxDiarizer, PyannoteOnnxConfig, PyannoteBackend, SpeakerCluster, LivePyannoteBackend}` (Tier-2 stable). `PyannoteOnnxDiarizer<B>` is generic over `PyannoteBackend` so the cluster-to-name mapping (which uses `MeetingContext.attendees` to turn anonymous cluster labels into `SpeakerLabel::Named` when possible, falling back to `Named("Speaker N")` when the attendee list is shorter than the cluster set) is exercised by an in-tree stub backend on every PR. The `*.partial`-rejection at construction time matches the `WhisperLocalProvider` and `ParakeetLocalProvider` shapes.
- `scrybe_core::diarize::kind::{DiarizerKind, select_kind, DIARIZER_KIND_BINARY_CHANNEL, DIARIZER_KIND_PYANNOTE_ONNX}`. `DiarizerKind` is a two-variant enum with a `from_config_str` parser and an `as_str` round-trip. `select_kind(configured, capabilities, ctx)` encodes the auto-routing rule from `system-design.md` §4.4: explicit user choice wins over the auto-rule; the auto-rule sends multi-party (≥3 attendees) or single-channel calls to `PyannoteOnnx` and everything else to `BinaryChannel`.
- `scrybe_core::config::AndroidConfig` (Tier-2 stable, additive). New `[android]` block with `audio_backend: String` defaulting to `"auto"`. `#[serde(deny_unknown_fields)]` matches the rest of the schema. Configs authored before v0.5.0 (no `[android]` block) continue to load unchanged because the field carries `#[serde(default)]`.
- `scrybe_core::config::DiarizerConfig` (Tier-2 stable, additive). New `[diarizer]` block with `kind: String` defaulting to `"auto"`. `is_auto()` and `validated_kind()` helpers on the struct surface both the routing-decision query and the typo-detection path.
- `scrybe_core::config::{ANDROID_AUDIO_BACKEND_AUTO, ANDROID_AUDIO_BACKEND_MEDIA_PROJECTION, ANDROID_AUDIO_BACKEND_MIC_ONLY, DIARIZER_KIND_AUTO, DIARIZER_KIND_BINARY_CHANNEL, DIARIZER_KIND_PYANNOTE_ONNX}` constants. Tested for parity with `scrybe_android::backend::Backend::from_config_str` and `scrybe_core::diarize::kind::DiarizerKind::from_config_str` so the schema and the adapter / module enums stay in lock-step.
- `scrybe_core::error::PipelineError::DiarizerUnavailable { reason: String }` variant. Used by both the feature-disabled branch of `LivePyannoteBackend::new` and by `PyannoteOnnxDiarizer::new_live` to reject `*.partial` model paths. Renders as `pipeline: diarizer unavailable: <reason>` through the `CoreError` `Display` chain.
- `diarize-pyannote` cargo feature on `scrybe-core`. Off by default; enables the live `pyannote-onnx` runtime binding behind `LivePyannoteBackend`. The trait surface and the cluster-to-attendee mapping ship without the feature so they remain testable on every PR.
- New unit tests on `scrybe-android` covering: backend parsing + as-str round-trip, `ProbeResult::from_api_level` across the four representative host shapes (no Android runtime / API 28 / API 29 with consent / API 29 without consent / API 34 with consent), `Backend::Auto` resolution preferring `MediaProjection` over `MicOnly`, explicit-backend resolution returning `None` when the requested backend is unavailable, error-promotion to `CaptureError::DeviceUnavailable` for five error variants and to `CaptureError::PermissionDenied` for the `UserDeclinedConsent` variant, capability advertisement, `frames()` single-consumer semantics, `stop()` idempotence, and start-after-stop returning `DeviceUnavailable`. New unit tests on `scrybe-core::diarize::kind` covering the parser, the round-trip, the auto-rule across all four representative host shapes, and the explicit-choice-wins guarantee. New unit tests on `scrybe-core::diarize::pyannote_onnx` covering the cluster-to-attendee mapping (named attendee, fallback `Speaker N`, no overlap → `Unknown`), the merged-by-`start_ms` ordering, the backend round-trip, the `*.partial` rejection, and the feature-on / feature-off branches of `new_live`. New unit tests on `scrybe-core::config` covering the `[android]` and `[diarizer]` block parsing paths, default values, unknown-field rejection, validated-kind / validated-backend helpers, round-trip preservation, and parity with the adapter / module constants.

### Changed

- Workspace LoC budget gate (`scripts/check-loc-budget.py`) extended with a `scrybe-android: 2500` ceiling and `scrybe-core` raised from `7500` to `8500` to absorb the new `PyannoteOnnxDiarizer`, `DiarizerKind`, `AndroidConfig`, and `DiarizerConfig` modules plus their parity-constant test surface.
- `scrybe-core::diarize` reorganised from a single flat file into a directory module (`diarize/mod.rs` + `diarize/kind.rs` + `diarize/pyannote_onnx.rs`). Every public symbol from the v0.4.0 surface (`Diarizer`, `BinaryChannelDiarizer`, `requires_neural`) is preserved verbatim; the new `DiarizerKind`, `select_kind`, `PyannoteBackend`, `PyannoteOnnxConfig`, `PyannoteOnnxDiarizer`, and `SpeakerCluster` symbols are re-exported through `scrybe_core::diarize::*` and `scrybe_core::*`.
- All workspace crates bumped from `0.4.0` to `0.5.0`. Path-dep version pins follow. `scrybe::tests::test_version_constant_matches_cargo_metadata` updated to lock against the `0.5.x` line.

### Deprecated / Removed

- Nothing.

### Security

- `cargo audit` and `cargo deny` policies are unchanged from v0.4.0 (same advisory ignores, same license clarifies).
- The default-feature graph is unchanged — `scrybe-android` and the `diarize-pyannote` feature add no runtime dependencies. `scrybe-android` imports only crates already in the workspace dependency tree (`async-trait`, `futures`, `scrybe-core`, `thiserror`, `tokio`, `tracing`); no new transitive dependencies enter `Cargo.lock`. The `egress-audit` CI lane verifies on every PR.
- The `AndroidCaptureError` `From` impl is the second adapter (after `scrybe-capture-mac`) to produce a non-`DeviceUnavailable` `CaptureError` variant: the `UserDeclinedConsent` arm promotes to `CaptureError::PermissionDenied`. The path that actually fires this variant requires the live JNI binding (a v0.5.x follow-up), so the `Platform` boxing surface stays unused on Android until then; the lifetime-erased Box surface is exercised only on macOS today.

### Known limitations

- **MediaProjection live binding deferred.** v0.5.0 ships the trait surface, the backend-detection logic, the host-API-level probe, the consent-state gate, and the configuration block. The live JNI binding (and the uniffi-generated Kotlin facade for the Compose UI shell) is tracked as a v0.5.x follow-up; this release surfaces a clear `CaptureError::DeviceUnavailable` rather than attempting capture against an un-validated FFI shape. Validation requires Android hardware and the NDK toolchain; the maintainer's macOS-only development environment does not provide either, and the GitHub-hosted CI matrix does not include an Android runner.
- **Pyannote-ONNX live runtime binding deferred.** v0.5.0 ships the diarizer trait, the cluster-to-attendee mapping, the routing logic, and the `diarize-pyannote` feature gate. The live ONNX runtime wiring lands as a v0.5.x follow-up; with the feature enabled, `PyannoteOnnxDiarizer::new_live` and `LivePyannoteBackend::cluster` return `PipelineError::DiarizerUnavailable` with a reason that names the missing runtime so the typed error surfaces the gap rather than a phantom successful load.
- **uniffi proc-macro generation deferred.** The `scrybe-android` crate ships as a clean Rust API today; the `uniffi` 0.31 proc-macro generation that produces the Kotlin facade for the Compose UI lands when the live JNI binding does. Keeping uniffi gated until there is a concrete Compose UI call site (≥1 today) avoids landing scaffolding that nothing exercises.
- **Self-hosted Android Tier-3 runner not yet registered.** The `nightly-e2e.yml` workflow grew a macOS lane in v0.1.0; the Linux + PipeWire / Pulse, Windows + WASAPI, and Android + MediaProjection equivalents wait on hardware availability per `system-design.md` §11.
- **Diarizer pipeline integration deferred.** The `[diarizer]` config block is wired through the schema and the `select_kind` helper is callable, but the `Session` orchestrator still constructs `BinaryChannelDiarizer` directly. Routing the configured `DiarizerKind` through `Session::run` is the obvious next-step v0.5.x follow-up and lands when the pyannote live runtime arrives so the integration test exercises both branches.

### Workspace

- 7 crates (`scrybe`, `scrybe-core`, `scrybe-capture-mac`, `scrybe-capture-linux`, `scrybe-capture-win`, `scrybe-android`, `scrybe-cli`).
- Publish posture unchanged: only `scrybe` publishes to crates.io.
- 458 unit tests pass workspace-wide (3 + 41 + 39 + 10 + 39 + 76 + 250).

### Contributors

- Maintainer: Mathews Tom.

[0.5.0]: https://github.com/Mathews-Tom/scrybe/releases/tag/v0.5.0

## [0.4.0] — 2026-05-01

Windows capture trait surface, runtime backend detection, and config wiring; ParakeetLocalProvider seam for the English-priority alternate STT path. v0.4.0 delivers the architectural seam for Windows audio capture per `.docs/development-plan.md` §10 — the new `scrybe-capture-win` crate, the `Backend` enum (`auto`/`wasapi-loopback`/`wasapi-process-loopback`), the `RtlGetVersion`-driven host probe that distinguishes Windows 10 build 20348+ (per-process loopback supported) from earlier builds (system-wide loopback only), and the `[windows] audio_backend` configuration block. The live WASAPI binding (system-wide loopback and `AUDIOCLIENT_PROCESS_LOOPBACK_PARAMS` per-process loopback) is tracked as a v0.4.x follow-up: `WindowsCapture::start()` resolves the requested backend against the live host and returns a clear `CaptureError::DeviceUnavailable` when the live binding is not yet wired in. This mirrors the macOS-first / Linux-first pattern from v0.1.0 and v0.3.0.

`ParakeetLocalProvider` ships in the same release as the third local STT path alongside `WhisperLocalProvider` and `OpenAiCompatSttProvider`. The provider type, config struct, and trait wiring are in place; the live `sherpa-rs` binding sits behind the `parakeet-local` cargo feature and lands as a v0.4.x follow-up. Without the feature, `transcribe()` returns `SttError::ModelNotLoaded` with a message naming the missing feature, mirroring the `WhisperLocalProvider` scaffold pattern.

The publish posture from v0.1.0 / v0.2.0 / v0.3.0 carries forward unchanged: only `scrybe` (the placeholder) publishes to crates.io. `scrybe-core`, `scrybe-capture-mac`, `scrybe-capture-linux`, `scrybe-capture-win`, and `scrybe-cli` stay workspace-private (`publish = false`).

### What you can actually do at v0.4.0

- Everything you could at v0.3.0 (macOS Core Audio Taps capture, local whisper-rs, local Ollama, OpenAI-compatible cloud STT/LLM, ICS calendar context, signed webhooks, `Hook::Git` auto-commit, channel-split `BinaryChannelDiarizer`, `transcript.partial.jsonl` write-ahead log, Linux `audio_backend` selector), plus:
- Compose against the `scrybe_capture_win::AudioCapture` impl from a Windows build. The `WindowsCapture` adapter implements the same `start`/`stop`/`frames`/`capabilities` contract as `MacCapture` and `LinuxCapture`, with the same `Arc<Mutex<SharedState>>` ownership shape and the same `inject_for_test` / `close_for_test` integration-test surface.
- Choose a backend via `windows.audio_backend = "auto" | "wasapi-loopback" | "wasapi-process-loopback"` in `config.toml`. Default is `"auto"`, which the runtime resolves against the host build number reported by `RtlGetVersion`. Auto prefers per-process loopback on Windows 10 build 20348+ and falls back to system-wide loopback on earlier builds; a host that cannot expose either backend (or any non-Windows host) collapses to `Backend::Auto → None`, which surfaces as `DeviceUnavailable` with the `NoBackendAvailable` rationale.
- Distinguish a "needs newer Windows" host from a fully unsupported host: an explicit `windows.audio_backend = "wasapi-process-loopback"` on a build older than 20348 returns `RequestedBackendUnavailable { requested: "wasapi-process-loopback" }` rather than silently falling through to system-wide loopback.
- Reference `ParakeetLocalProvider` and `ParakeetLocalConfig` from any consumer of `scrybe_core::providers`. The provider name (`parakeet-local:tdt-v2`) and the `*.partial`-rejection at construction time match the `WhisperLocalProvider` shape so `scrybe-cli` can switch between the two via `[stt] provider = "parakeet-local"` once the live binding lands.

### Added

- `scrybe-capture-win` crate (under the 2 500-LoC ceiling enforced by `scripts/check-loc-budget.py`). New workspace member listed in the root `Cargo.toml`.
- `scrybe_capture_win::WindowsCapture` implementing `scrybe_core::capture::AudioCapture`. Mirrors the `MacCapture` and `LinuxCapture` shapes — `Arc<Mutex<SharedState>>` ownership, `tokio::sync::mpsc::unbounded_channel` plumbing, single-consumer `frames()` semantics. `start()` / `stop()` are idempotent; calling `start()` after `stop()` returns `DeviceUnavailable` (the adapter is single-use across a `start → stop → start` cycle, per the macOS / Linux adapter precedent). `inject_for_test` / `close_for_test` test surfaces match the macOS adapter byte-for-byte.
- `scrybe_capture_win::backend::{Backend, ProbeResult, probe, detect, PROCESS_LOOPBACK_MIN_BUILD, WASAPI_LOOPBACK_MIN_BUILD}`. `Backend` is a three-variant enum (`Auto`/`WasapiLoopback`/`WasapiProcessLoopback`) with a `from_config_str` parser and an `as_str` round-trip. `ProbeResult::from_build(host_build)` derives availability flags from the OS build number; tests construct it directly to exercise resolution against any host shape without requiring a live Windows runner. `detect()` reads the host build via `RtlGetVersion` (the version-lying-suppression API in `ntdll.dll`) on Windows hosts when the `wasapi-loopback` feature is on; on non-Windows hosts and feature-disabled builds it returns `0` so the resolution table collapses to `None` rather than promising an unreachable backend.
- `scrybe_capture_win::error::WindowsCaptureError` — adapter-local error type with `WasapiLoopbackDisabled`, `WasapiProcessLoopbackDisabled`, `NoBackendAvailable`, `RequestedBackendUnavailable { requested }`, and `ProcessLoopbackRequiresNewerBuild { build }`. Promotes uniformly to `CaptureError::DeviceUnavailable` via `From` so the pipeline error-handling path stays identical to the macOS / Linux adapters.
- `scrybe_core::config::WindowsConfig` (Tier-2 stable, additive). New `[windows]` block with `audio_backend: String` defaulting to `"auto"`. `#[serde(deny_unknown_fields)]` matches the rest of the schema, so a typo'd field surfaces with a line number. The `Config::default()` shape includes `windows: WindowsConfig::default()`; configs authored before v0.4.0 (no `[windows]` block) continue to load unchanged because the field carries `#[serde(default)]`.
- `scrybe_core::config::{WINDOWS_AUDIO_BACKEND_AUTO, WINDOWS_AUDIO_BACKEND_WASAPI_LOOPBACK, WINDOWS_AUDIO_BACKEND_WASAPI_PROCESS_LOOPBACK}` constants. Tested for parity with `scrybe_capture_win::backend::Backend::from_config_str` so the `scrybe-core` schema and the adapter enum stay in lock-step.
- `scrybe_core::providers::parakeet_local::{ParakeetLocalProvider, ParakeetLocalConfig}` behind the new `parakeet-local` cargo feature. The provider mirrors the `WhisperLocalProvider` shape: the type exists in every build, `transcribe()` returns `SttError::ModelNotLoaded` when the feature is disabled, and `*.partial` model paths are rejected with `SttError::ModelCorrupt` at construction time. The default model label is `tdt-v2`, naming Parakeet TDT v2; the v3 follow-up will switch the label without touching the trait surface.
- New unit tests on `scrybe-capture-win` covering: backend parsing + as-str round-trip, `ProbeResult::from_build` across the four representative host shapes (zero / Vista / Windows 10 19044 / Windows 11 22621), `Backend::Auto` resolution preferring per-process loopback over system-wide loopback when both are supported, explicit-backend resolution returning `None` when the requested backend is unavailable, error-promotion to `CaptureError::DeviceUnavailable` for all five error variants, capability advertisement, `frames()` single-consumer semantics, `stop()` idempotence, and start-after-stop returning `DeviceUnavailable`. New unit tests on `scrybe-core::config` covering the `[windows]` block parsing path, default value, unknown-field rejection, and round-trip. New unit tests on `scrybe_core::providers::parakeet_local` covering name formatting, `*.partial` rejection, `is_partial` extension matching, and the feature-disabled `transcribe()` surface.

### Changed

- Workspace LoC budget gate (`scripts/check-loc-budget.py`) extended with a `scrybe-capture-win: 2500` ceiling and `scrybe-core` raised from `6500` to `7500` to absorb the new `WindowsConfig` block, the `ParakeetLocalProvider` scaffold, and the parity-constant test surface.
- All workspace crates bumped from `0.3.0` to `0.4.0`. Path-dep version pins follow. `scrybe::tests::test_version_constant_matches_cargo_metadata` updated to lock against the `0.4.x` line.

### Deprecated / Removed

- Nothing.

### Security

- `cargo audit` and `cargo deny` policies are unchanged from v0.3.0 (same advisory ignores, same license clarifies).
- The default-feature graph is unchanged — `scrybe-capture-win` and `parakeet_local` add no runtime dependencies. `egress-audit` CI lane verifies on every PR.
- The `WindowsCaptureError` `From` impl uniformly produces `CaptureError::DeviceUnavailable`, never `Platform`. There is no path by which a v0.4.0 build can emit a `CaptureError::Platform` from the Windows adapter, which keeps the lifetime-erased `Platform` boxing surface unused on Windows until the live binding lands.
- `RtlGetVersion` is invoked through an in-tree `extern "system"` declaration inside an `#[allow(unsafe_code)]` block; the `unsafe_code = "deny"` workspace lint stays untouched on every other crate. The struct shape is fixed by the Win32 ABI and verified against the documented `OSVERSIONINFOW` layout.

### Known limitations

- **WASAPI live binding deferred.** v0.4.0 ships the trait surface, the backend-detection logic, the host-build probe, and the configuration block. The live `windows-sys` WASAPI binding (system-wide loopback via `IAudioClient::Initialize` with `AUDCLNT_STREAMFLAGS_LOOPBACK`, per-process loopback via `ActivateAudioInterfaceAsync` + `AUDIOCLIENT_PROCESS_LOOPBACK_PARAMS`) is tracked as a v0.4.x follow-up; this release surfaces a clear `CaptureError::DeviceUnavailable` rather than attempting capture against an un-validated FFI shape. Validation requires Windows hardware that the maintainer's macOS-only development environment does not provide; the CI matrix's `windows-latest` runner does not host an audio device suitable for end-to-end loopback verification.
- **Parakeet live binding deferred.** v0.4.0 ships the provider scaffold; the `sherpa-rs` integration that links the upstream `sherpa-onnx` C++ runtime lands as a v0.4.x follow-up gated on the `parakeet-local` cargo feature.
- **MSI installer deferred.** `cargo-wix` packaging for Windows release artifacts is documented for the v0.4.x stream but not shipped in v0.4.0. The `INSTALL.md` SmartScreen "More info → Run anyway" walkthrough is the remaining v0.4.x deliverable.
- **Self-hosted Windows Tier-3 runner not yet registered.** The `nightly-e2e.yml` workflow grew a macOS lane in v0.1.0; the Linux + PipeWire / Pulse and Windows + WASAPI equivalents wait on hardware availability.

### Workspace

- 6 crates (`scrybe`, `scrybe-core`, `scrybe-capture-mac`, `scrybe-capture-linux`, `scrybe-capture-win`, `scrybe-cli`).
- Publish posture unchanged: only `scrybe` publishes to crates.io.

### Contributors

- Maintainer: Mathews Tom.

[0.4.0]: https://github.com/Mathews-Tom/scrybe/releases/tag/v0.4.0

## [0.3.0] — 2026-05-01

Linux capture trait surface, runtime backend detection, and config wiring. v0.3.0 delivers the architectural seam for Linux audio capture per `.docs/development-plan.md` §9 — the new `scrybe-capture-linux` crate, the `Backend` enum (`auto`/`pipewire`/`pulse`), the `XDG_RUNTIME_DIR` socket probe that distinguishes a PipeWire host from a Pulse-only host, and the `[linux] audio_backend` configuration block. The live PipeWire and PulseAudio bindings are tracked as a v0.3.x follow-up: `LinuxCapture::start()` resolves the requested backend against the live host and returns a clear `CaptureError::DeviceUnavailable` when the live binding is not yet wired in. This mirrors the macOS-first pattern (PR #8 shipped the `scrybe-capture-mac` scaffold; PR #11 added the live Core Audio Tap binding).

The publish posture from v0.1.0 / v0.2.0 carries forward unchanged: only `scrybe` (the placeholder) publishes to crates.io. `scrybe-core`, `scrybe-capture-mac`, `scrybe-capture-linux`, and `scrybe-cli` stay workspace-private (`publish = false`).

### What you can actually do at v0.3.0

- Everything you could at v0.2.0 (macOS Core Audio Taps capture, local whisper-rs, local Ollama, OpenAI-compatible cloud STT/LLM, ICS calendar context, signed webhooks, `Hook::Git` auto-commit, channel-split `BinaryChannelDiarizer`, `transcript.partial.jsonl` write-ahead log), plus:
- Compose against the `scrybe_capture_linux::AudioCapture` impl from a Linux build. The `LinuxCapture` adapter implements the same `start`/`stop`/`frames`/`capabilities` contract as `MacCapture`, with the same `Arc<Mutex<SharedState>>` ownership shape and the same `inject_for_test` / `close_for_test` integration-test surface.
- Choose a backend via `linux.audio_backend = "auto" | "pipewire" | "pulse"` in `config.toml`. Default is `"auto"`, which the runtime resolves against the user-session sockets under `XDG_RUNTIME_DIR`. Auto prefers PipeWire over Pulse on hosts where both sockets are present (the `pipewire-pulse` shim creates a Pulse socket on PipeWire-default hosts; naive Pulse-first selection would route through the shim instead of the native API).
- Distinguish a Pulse-only host (RHEL 8 / Ubuntu 20.04 LTS) from a fully unsupported host: the runtime detection still recognises the `pulse/native` socket, even though the live Pulse binding is not yet shipped. Configurators see `DeviceUnavailable: PulseAudio backend not yet implemented in this release` rather than a generic "no backend" error.

### Added

- `scrybe-capture-linux` crate (621 LoC under the 2 500-LoC ceiling enforced by `scripts/check-loc-budget.py`). New workspace member listed in the root `Cargo.toml`.
- `scrybe_capture_linux::LinuxCapture` implementing `scrybe_core::capture::AudioCapture`. Mirrors the `MacCapture` shape — `Arc<Mutex<SharedState>>` ownership, `tokio::sync::mpsc::unbounded_channel` plumbing, single-consumer `frames()` semantics. `start()` / `stop()` are idempotent; calling `start()` after `stop()` returns `DeviceUnavailable` (the adapter is single-use across a `start → stop → start` cycle, per the macOS adapter precedent). `inject_for_test` / `close_for_test` test surfaces match the macOS adapter byte-for-byte.
- `scrybe_capture_linux::backend::{Backend, ProbeResult, probe, detect}`. `Backend` is a three-variant enum (`Auto`/`PipeWire`/`Pulse`) with a `from_config_str` parser and an `as_str` round-trip. `probe(xdg_runtime_dir)` returns a pure `ProbeResult` testable against a tempdir tree. `detect(requested)` reads `XDG_RUNTIME_DIR` (or falls back to `/run/user/<uid>` parsed from `/proc/self/status`) and returns the resolved backend, or `None` if no supported socket is present.
- `scrybe_capture_linux::error::LinuxCaptureError` — adapter-local error type with `PipeWireDisabled`, `PulseDisabled`, `NoBackendAvailable`, `RequestedBackendUnavailable { requested }`. Promotes uniformly to `CaptureError::DeviceUnavailable` via `From` so the pipeline error-handling path stays identical to the macOS / future Windows adapters.
- `scrybe_core::config::LinuxConfig` (Tier-2 stable, additive). New `[linux]` block with `audio_backend: String` defaulting to `"auto"`. `#[serde(deny_unknown_fields)]` matches the rest of the schema, so a typo'd field surfaces with a line number. The `Config::default()` shape includes `linux: LinuxConfig::default()`; configs authored before v0.3.0 (no `[linux]` block) continue to load unchanged because the field carries `#[serde(default)]`.
- 38 unit tests on `scrybe-capture-linux` covering: backend parsing + as-str round-trip, socket probing across all four (pipewire-only, pulse-only, both, neither) tempdir layouts, `Backend::Auto` resolution preferring PipeWire over Pulse on dual-socket hosts, explicit-backend resolution returning `None` when the requested socket is missing, `/proc/self/status` UID parsing (real layout, missing field, non-numeric), error-promotion to `CaptureError::DeviceUnavailable` for all four error variants, capability advertisement, `frames()` single-consumer semantics, `stop()` idempotence, and start-after-stop returning `DeviceUnavailable`. 6 new unit tests on `scrybe-core::config` covering the `[linux]` block parsing path, default value, unknown-field rejection, and round-trip.

### Changed

- Workspace LoC budget gate (`scripts/check-loc-budget.py`) extended with a `scrybe-capture-linux: 2500` ceiling. Current LoC: 621.

### Deprecated / Removed

- Nothing.

### Security

- `cargo audit` and `cargo deny` policies are unchanged from v0.2.0 (same advisory ignores, same license clarifies).
- The default-feature graph is unchanged — `scrybe-capture-linux` adds no runtime dependencies. `egress-audit` CI lane verifies on every PR.
- The `LinuxCaptureError` `From` impl uniformly produces `CaptureError::DeviceUnavailable`, never `Platform`. There is no path by which a v0.3.0 build can emit a `CaptureError::Platform` from the Linux adapter, which keeps the lifetime-erased `Platform` boxing surface unused on Linux until the live binding lands.

### Known limitations

- **PipeWire and PulseAudio live bindings deferred.** v0.3.0 ships the trait surface, the backend-detection logic, and the configuration block. The live `pipewire 0.9` and `libpulse-binding 2.28` bindings are tracked as v0.3.x follow-ups; this release surfaces a clear `CaptureError::DeviceUnavailable` rather than attempting capture against an un-validated FFI shape. Validation requires Linux hardware that the maintainer's macOS-only development environment does not provide; the CI matrix's `ubuntu-latest` runner does not host a PipeWire daemon either.
- **Distro packaging deferred.** `cargo deb` for Ubuntu 22+ / Debian 12+, AUR `scrybe-bin`, and Flatpak manifest are documented for the v0.3.x stream but not shipped in v0.3.0.
- **Self-hosted Linux Tier-3 runner not yet registered.** The `nightly-e2e.yml` workflow grew a macOS lane in v0.1.0; the Linux + PipeWire / Pulse equivalent waits on hardware availability.

### Workspace

- 5 crates (`scrybe`, `scrybe-core`, `scrybe-capture-mac`, `scrybe-capture-linux`, `scrybe-cli`).
- Publish posture unchanged: only `scrybe` publishes to crates.io.

### Contributors

- Maintainer: Mathews Tom.

[0.3.0]: https://github.com/Mathews-Tom/scrybe/releases/tag/v0.3.0

## [0.2.0] — 2026-05-01

Cloud providers, calendar context, signed webhooks, and channel-split diarization land on top of the macOS-alpha foundation. v0.1.0 was a vertical slice of the local-only path — record on macOS, transcribe with whisper-rs, summarize with Ollama, write markdown to disk. v0.2.0 is the horizontal expansion across the four extension seams: a second `SttProvider` and `LlmProvider` (OpenAI-compatible HTTP for Groq / OpenAI / Together / vLLM / self-hosted), a second `ContextProvider` (`IcsFileProvider`), a second `Hook` (HMAC-SHA256-signed webhook), and the v0.1-default `BinaryChannelDiarizer` impl materialized in code rather than a trait shape. Storage gains the crash-recovery write-ahead log documented in `docs/system-design.md` §8.3. Configuration gains four additive blocks (`[stt.retry]`, `[llm.retry]`, `[context]`, `[hooks.webhook]`, `[consent.default_mode]`); every default preserves v0.1 behavior and `schema_version` stays at 1.

The publish posture from v0.1.0 carries forward unchanged: only `scrybe` (the placeholder) publishes to crates.io. `scrybe-core`, `scrybe-capture-mac`, and `scrybe-cli` stay workspace-private (`publish = false`) because the Tier-2 trait surface (`ContextProvider`, `SttProvider`, `LlmProvider`, `Diarizer`, `Hook`) is still alpha-quality and explicitly evolving per `system-design.md` §12. Binary distribution remains via cargo-dist-built GitHub Release tarballs.

### What you can actually do at v0.2.0

- Everything you could at v0.1.0 (macOS Core Audio Taps capture, local whisper-rs, local Ollama, markdown on disk, `Hook::Git` auto-commit), plus:
- Point STT at any OpenAI-compatible endpoint via `provider = "openai-compat"` in `[stt]` — Groq, Together, OpenAI itself, or a self-hosted vLLM in OpenAI-compat mode all work without a fake-key shim. Retry policy with exponential backoff is configured under `[stt.retry]`; defaults match `system-design.md` §8.2 (3 attempts, 500 ms initial, 8 s ceiling).
- Same selection mechanism for the LLM in `[llm]` with retry under `[llm.retry]`.
- Match upcoming meetings against a local `.ics` file via `sources = ["ics"]` in `[context]`. Google Calendar, Outlook, and Apple Calendar exports are all parsed within a 15-minute start-time window; attendee `CN`-with-and-without-quotes, `mailto:` local-part fallback, and the three datetime shapes that show up in real exports are all handled.
- POST a small JSON payload (with optional HMAC-SHA256 signing under the `X-Scrybe-Signature: sha256=…` header) to a configured webhook URL on `SessionEnd`, `NotesGenerated`, and `SessionFailed`. Per-chunk events are skipped on purpose — webhook receivers care about completion, not progress.
- Get a `Me:` / `Them:` channel-split transcript on a 1:1 remote call via the now-real `BinaryChannelDiarizer`. Multi-party calls (≥3 attendees) and single-channel inputs continue to route to the v0.5 neural fallback as documented in §4.4.
- Recover a crashed session deterministically: `transcript.partial.jsonl` (a write-ahead log of in-flight chunks) lets `scrybe doctor` distinguish chunks rendered to `transcript.md` from chunks that crashed mid-render. The append-then-mark protocol is idempotent under re-marks, malformed lines are counted but not fatal, and reopening after a crash continues without manual intervention.

### Added

- `scrybe-core::providers::openai_compat_stt::OpenAiCompatSttProvider` and `openai_compat_llm::OpenAiCompatLlmProvider` behind the new `openai-compat` feature. The HTTP-status-to-retry-outcome mapping (200-299 ok, 408/425/429/500-599 transient, 401/4xx-other permanent, network-timeout transient, malformed-JSON permanent) reuses the existing `retry_with_policy` / `RetryOutcome` infrastructure rather than reinventing exponential backoff per provider. `Authorization` is omitted when `api_key` is empty so self-hosted Ollama / vLLM in OpenAI-compat mode work without a fake-key shim. STT input is encoded as a single-channel 16 kHz 16-bit-PCM WAV in memory and POSTed as `multipart/form-data` (commit `4a0f52e`).
- `scrybe-core::context::ics::IcsFileProvider` behind the new `context-ics` feature. Parses local `.ics` files via the `ical` crate; matches by start-time window (15 minutes default) against the session's `started_at`. Recurring events rely on the per-occurrence emission every major calendar exporter does inside short windows. Attendee extraction handles `ATTENDEE;CN=Tom:mailto:…`, `ATTENDEE;CN="Tom, Mathews":mailto:…`, and bare `mailto:` local-part fallbacks (commit `5c82146`).
- `scrybe-core::hooks::webhook::WebhookHook` behind the new `hook-webhook` feature. Async POST + HMAC-SHA256 body signing in the `X-Scrybe-Signature: sha256=<lowercase-hex>` GitHub-style header. The `webhook_sign_body` helper is exposed so server-side tests can assert the same algorithm without duplicating the math. Hook fires on `SessionEnd`, `NotesGenerated`, and `SessionFailed`; `HookFailed` is intentionally skipped to avoid reentry loops (commits `8f5bed2`, `9326c71`).
- `scrybe-core::diarize::BinaryChannelDiarizer` — the v0.1-documented default, now materialized. Mic-channel transcripts attribute to `SpeakerLabel::Me`, system-channel transcripts to `SpeakerLabel::Them`, merged stream sorted by `start_ms` with mic-first on ties. The `meta.toml` `[providers].diarizer = "binary-channel"` string matches the `BinaryChannelDiarizer::NAME` constant, keeping the on-disk schema stable across the v0.1 → v0.2 transition (commit `b24e87a`).
- `scrybe-core::storage::transcript_log::TranscriptPartialLog` — the `transcript.partial.jsonl` write-ahead log from `system-design.md` §8.3. `open()` recovers the next monotonic `seq` by scanning the existing file, `append_pending(chunk)` writes a `flushed_to_transcript = false` line, `mark_flushed(seq, chunk)` writes the matching `flushed_to_transcript = true` line, and `scan_recovery(folder)` collapses the WAL by taking the highest-flushed record per seq. Pending records that never received a flushed follow-up surface as orphans; flushed records become a `flushed_seqs` cursor. Malformed lines are counted in `malformed_line_count` but do not abort recovery (commits `99b3748`, `e323f12`).
- Configuration schema additions (commit `7a0f663`): `[stt.retry]`, `[llm.retry]`, `[context.sources]` + `[context.ics_path]`, `[hooks.webhook]` (`url`, `secret_env`, `timeout_ms`), and `[consent.default_mode]`. HMAC secrets are read via the `secret_env` env-var name, never stored in the on-disk file — matches `~/.claude/rules/security-standards.md`. `#[serde(deny_unknown_fields)]` on every nested block stays in force, so a typo'd `[hooks.webhok]` still surfaces with a line number.
- Workspace dependency additions (commit `901b985`): `reqwest 0.12` (rustls-only TLS, no openssl), `ical 0.10`, `hmac 0.12`, `sha2 0.10`, `hex 0.4`. `wiremock 0.6.0` (precise pin, dev-only) for HTTP mock tests — `0.6.5+` requires rustc 1.88. All v0.2 runtime deps are optional on `scrybe-core` and gated behind their feature flags so the default-feature graph remains free of network and TLS crates; the `egress-audit` CI lane verifies this.

### Changed

- `ConsentMode` derives `Default` with `Quick` as the `#[default]` variant (commit `70eab69`). Lets `ConsentConfig` derive `Default` without hand-rolling the variant pick at every call site. Quick is the floor enforced by the consent step per `system-design.md` §5; on-disk shape is unchanged (lowercase serde tags via `#[serde(rename_all = "lowercase")]`).
- AUDIT-LOG.md (commits `14663f7`, `720b64b`): two appended entries record the v0.1.0 release-pipeline triage trail (PRs #24, #25, #26 — three distinct latent defects between the first `git push origin v0.1.0` and the final green release.yml run, with diagnosis, fix, and why PR-time CI didn't catch it for each round) and a self-review observation about an AI-attribution-adjacent substring appearing in narrative use inside a prior commit body (letter-of-the-rule clean per the global `forbidden-strings` list, spirit-of-the-rule borderline). Records two complementary future-hardening lanes (`dist-build-host` for Rounds 1 and 2; `dist-stage-asset-shape` for Round 3) rather than the original single-lane claim, which Round-3 analysis showed wouldn't catch the `gh release create` directory-rejection failure.

### Fixed

- `WebhookHook` now fires on `LifecycleEvent::SessionFailed` (commit `9326c71`). Receivers configured for completion alerts were silently never firing on a crashed session; the new payload field is `error: Option<String>` (rendered `Display` chain). `LifecycleEvent::HookFailed` is still skipped on purpose — a webhook returning an error reentering `dispatch_hooks` would loop.
- `OpenAiCompatLlmProvider` returns `LlmError::Transport` (not `LlmError::PromptRendering`) when the upstream answers 200 with an empty `choices` array (commit `9326c71`). `PromptRendering` is documented at `scrybe-core/src/error.rs:362` as "implementation rejects the prompt shape" — the rendering of *our* prompt failed; the empty-choices case is upstream-decoding, not prompt-rendering. Until `LlmError::Decoding` lands as a Tier-2 variant, the empty-choices case is wrapped in `std::io::Error::other` so `meta.toml` does not record a misleading category.
- Clippy under `--all-features` (commit `08f0989`). `providers/whisper_local.rs` test imports (`Arc`, `Duration`, `FrameSource`) are now gated under the same `#[cfg(not(feature = "whisper-local"))]` as the test that uses them so they don't become unused under `--all-features`. `hooks/git.rs` two pre-existing patterns rewritten without behavior change: `[notes].iter()` on a one-element array → `std::iter::once(&notes)`; `match repo.head() { Ok(_) => …, Err(_) => Vec::new() }` → `Result::map_or_else`.

### Deprecated / Removed

- Nothing.

### Security

- `cargo audit` remains clean against the known advisories; ignores in `audit.toml` and `deny.toml` are unchanged from v0.1.0.
- `cargo deny check advisories licenses bans sources` clean across the workspace. Two clarifications added (commits `77a5648`, `c653c5b`): `ical 0.10` ships an unmodified Apache-2.0 LICENSE file but its published manifest omits the SPDX field — `[[licenses.clarify]]` declares `Apache-2.0` and pins to the LICENSE CRC32 so a future tampering or upstream license change forces manual review. `webpki-roots 1.0.7` (transitive via `reqwest` + `rustls`) ships under CDLA-Permissive-2.0; added to the allowlist alongside Apache/BSD/MIT/ISC. Drop the `ical` clarify when upstream adds the manifest field.
- Egress audit: `openai-compat`, `context-ics`, and `hook-webhook` are off by default. `cargo build --no-default-features` remains free of `reqwest`, `hyper`, `rustls`, and `ical` in the dependency graph — the `egress-audit` CI lane verifies this on every PR.
- HMAC secrets for `WebhookHook` are read from an environment variable named via `[hooks.webhook].secret_env`; the on-disk config file stays free of credentials.

### Known limitations

- **macOS-only, still.** Linux PipeWire (§9) ships next; Windows WASAPI loopback (§10) and Android `MediaProjection` (§11) follow.
- **Cloud STT not yet hardware-validated against a real provider.** Tests cover the §8.3.1 unit matrix (200, 401, 429-then-200, 503 retry-exhaust, malformed JSON, missing-Authorization path) via wiremock. Real-provider validation against Groq + OpenAI is the I-10 integration test deferred to a future v0.2.x patch release.
- **`.ics` recurring events: per-occurrence-only.** The provider does not expand `RRULE` itself; it relies on the calendar exporter emitting one `VEVENT` per occurrence inside the match window. Google + Outlook always do; Apple Calendar emits an `RRULE`-bearing master with override children, which only matches if the override falls inside the window. Workaround: re-export from a calendar tool that expands recurrences, or open an issue if this matters.
- **Channel-split diarization needs the live Core Audio Tap binding.** `BinaryChannelDiarizer` is correct; the upstream system-audio channel only carries data when `scrybe-capture-mac` runs with the `core-audio-tap` feature on macOS 14.4+. Without it (e.g. on the synthetic in-process audio source), the diarizer attributes everything to `Me:` because there is no system-channel signal.

### Workspace

- Same 4 crates (`scrybe`, `scrybe-core`, `scrybe-capture-mac`, `scrybe-cli`).
- Publish posture unchanged: only `scrybe` publishes to crates.io. The other three remain `publish = false` per the v0.1.0 Option B rationale.

### Contributors

- Maintainer: Mathews Tom.

[0.2.0]: https://github.com/Mathews-Tom/scrybe/releases/tag/v0.2.0

## [0.1.0] — 2026-05-01

The first non-placeholder release of scrybe. Closes the macOS-alpha contract from `.docs/development-plan.md` §7. The `scrybe` crate on crates.io advances from `0.1.0-alpha.1` (placeholder reservation) to `0.1.0` (still a placeholder; the functional surface lives in workspace-private crates per the trait-stability tradeoff in §12). Binary distribution moves to GitHub Releases via `cargo-dist`-built tarballs; `cargo install scrybe` continues to install only the placeholder.

### What you can actually do at v0.1.0

- Capture audio on macOS 14.4+ via Core Audio Taps (`scrybe-capture-mac`, feature `core-audio-tap`). Mic-only attribution; channel-split diarization is a v0.2 deliverable.
- Run `scrybe init`, `scrybe record`, `scrybe list`, `scrybe show <id>`, `scrybe doctor` end-to-end with a synthetic in-process audio source (sine sweep) for environments where the Core Audio Tap binding isn't built in.
- Auto-commit the resulting transcript and notes to git via `Hook::Git` (feature `hook-git`).
- Transcribe locally via `whisper-rs` (feature `whisper-local`) or run the headless stub-provider path for smoke-testing the pipeline without a model.
- Distribute the macOS binary as an unsigned arm64 / x86_64 tarball with a documented `xattr -dr com.apple.quarantine` removal step (`INSTALL.md`). One-liner shell installer published alongside.

### Added

- `scrybe-core`: ports-and-adapters trait seams (`AudioCapture`, `ContextProvider`, `SttProvider`, `LlmProvider`, `Hook`, `Diarizer`, `ConsentPrompter`), structured error hierarchy (`CoreError` and 8 sibling variants), atomic-write storage primitives with platform-correct durability (`F_FULLFSYNC` on macOS, `MoveFileExW` on Windows, `fsync(dir)` elsewhere), pipeline stages (chunker, resampler, encoder, energy-VAD), session orchestrator, and TOML configuration loader (PR #7).
- `scrybe-capture-mac`: Core Audio Taps binding (`CATapDescription`, `AudioHardwareCreateProcessTap`, `AudioDeviceStart/Stop`) via `objc2` 0.6, gated behind the `core-audio-tap` feature so non-macOS hosts can still type-check the workspace (PR #11).
- `scrybe-cli`: `init`, `record`, `list`, `show`, `doctor` commands; tray icon (`NSStatusItem`) and global hotkey backed by a main-thread shell driver under the `cli-shell` feature; `Hook::Git` reference implementation under the `hook-git` feature; `WhisperLocalProvider` under `whisper-local` (PRs #8, #11, #13).
- Release pipeline: `cargo-dist` 0.25 workflow producing reproducible `aarch64-apple-darwin` and `x86_64-apple-darwin` tarballs plus a `scrybe-cli-installer.sh` one-liner. Tarballs are unsigned with documented quarantine removal; SHA256SUMS.txt covers every artifact (PRs #14, #15, #16).
- CI hardening: `cargo audit` and `cargo deny` (advisories, licenses, bans, sources), workspace and `scrybe-core` line-coverage gates (≥ 80% / ≥ 90% respectively), per-crate LoC budget enforcement via tokei, default-feature-graph egress audit ensuring no network crates leak in, and a `cargo bench --no-run` compile gate so the criterion harness can't bitrot (PRs #9, #10, #12, #17).
- Tier-3 self-hosted nightly E2E lane (`.github/workflows/nightly-e2e.yml`) targeting any registered `[self-hosted, macos, arm64]` runner. Setup procedure documented in `docs/ci-self-hosted.md`. Lane gated by `vars.NIGHTLY_E2E_ENABLED` and `github.repository == 'Mathews-Tom/scrybe'`. The first registered runner (`DRUK-scrybe`, MacBook Pro M-series) executes two ignored Tier-3 tests on every nightly tick (PR #18).
- E-1 hardware-validation test (`test_tap_captures_nonzero_frames_during_known_audio_playback`) that spawns `afplay` against a system fixture, captures from the live tap for 1.5s, and asserts both that frames arrived and that peak amplitude exceeds a 0.01 noise floor. Combined assertion uniquely identifies "TCC for Audio Capture is granted AND the tap is routed to the default output device". Empirically validated on the self-hosted runner (PR #20).

### Changed

- License: settled on Apache-2.0 (single, not dual). Patent grant + attribution + trademark + enforceable disclaimers — strictly dominates MIT for this project's risk profile (`docs/LEGAL.md`, PR #1).
- macOS distribution: dropped the Apple Developer ID requirement. Binaries ship unsigned via Homebrew taps (the convenience path) and direct GitHub Release tarballs (the audit-friendly path). Notarization is explicitly out of scope through v1.0 (PR #14).
- Hardware-neutral self-hosted runner setup: `docs/system-design.md` §11 Tier 3 originally cited a Mac mini M2 with a "signed test-helper". Empirically falsified on macOS 26.4.1 — an unsigned `Runner.Listener` inherits Audio Capture TCC without any GUI grant. Doc rewritten to call out any Apple Silicon Mac (M1+, macOS 14.4+) (PRs #18, #20).
- Prompter testability: `TtyPrompter::prompt` now delegates to `render_prompt_body`, `parse_consent_response`, and `read_consent_blocking<W: Write, R: BufRead>`. Production behavior is byte-for-byte identical (writer is dropped before reader blocks); test injection is via `Cursor` + `Vec<u8>` (PR #19).

### Fixed

- `scrybe-cli` per-command coverage. `commands/show.rs` was 95% line / 78% region; now 97% / 87%. `commands/record.rs` 96% / 81% → 96% / 85%. `prompter.rs` 75% / 58% → 92% / 82% (PRs #12, #19).
- `read_consent_blocking` lock-holding semantics. The pre-refactor `TtyPrompter::prompt` explicitly dropped the stdout handle before reading stdin; the helper-extracted form now does the same via `drop(writer)` after flush. Currently invisible (no concurrent stdout writers exist on the consent-prompt path), but matches original intent byte-for-byte (PR #19).

### Deprecated / Removed

- Nothing. v0.1.0 is the first non-placeholder release; the v0.1.0-alpha.1 placeholder remains discoverable on crates.io as the reservation entry.

### Security

- `cargo audit` clean (10 allowed warnings, none blocking).
- `cargo deny check advisories licenses bans sources` clean across the workspace.
- Egress audit: the default-feature build of `scrybe-cli` contains no network crates in its dependency graph. Network-using providers (cloud STT, cloud LLM) only appear when the consumer opts in via feature flags. Verified on every PR by `scripts/check-egress-baseline.py`.
- `RUSTSEC-2026-0008` (in `git2 0.19`) is the only known advisory affecting a workspace dep; tracked and ignored per `deny.toml` because the affected code path is not reachable from `Hook::Git`.

### Known limitations

- **macOS-only.** Linux PipeWire (§9) and Windows WASAPI loopback (§10) are deferred to v0.3 and v0.4 respectively.
- **Mic-only attribution.** `BinaryChannelDiarizer` ships as a trait shape but the channel-split implementation is a v0.2 deliverable (`.docs/development-plan.md` §8.2).
- **Local providers only.** `OpenAiCompatSttProvider` and `OpenAiCompatLlmProvider` for BYO cloud endpoints are v0.2.
- **`scrybe-cli` LoC ceiling.** §7.4 specifies ≤ 800 LoC; current size is ~1.5K. Either the ceiling or the architecture has to give; flagged in §7.6.3 for the next plan revision.
- **No self-update.** v1.1 ships `scrybe self-update`; v0.1 users update via Homebrew/Scoop/AUR/Flatpak/F-Droid or by re-running the shell installer.

### Workspace

- 4 crates (`scrybe`, `scrybe-core`, `scrybe-capture-mac`, `scrybe-cli`).
- Only `scrybe` is published to crates.io. `scrybe-core`, `scrybe-capture-mac`, and `scrybe-cli` are workspace-private (`publish = false`) — the Tier-2 trait surface (`ContextProvider`, `SttProvider`, `LlmProvider`, `Diarizer`, `Hook`) is alpha-quality and explicitly evolving per `system-design.md` §12; locking it behind a SemVer commitment to external consumers is premature. The dev-plan §7.4 wording calls for publishing all four; the next plan revision should reconcile.

### Contributors

- Maintainer: Mathews Tom.

[0.1.0]: https://github.com/Mathews-Tom/scrybe/releases/tag/v0.1.0

## [0.1.0-alpha.1] — 2026-04-29

Crate-name reservation on crates.io. No functional content. See PR #3.

[0.1.0-alpha.1]: https://github.com/Mathews-Tom/scrybe/releases/tag/v0.1.0-alpha.1
