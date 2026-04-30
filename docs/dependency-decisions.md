# scrybe — Dependency Decisions

> Frozen dependency choices for v1.0. Every entry pins a version, names the maintainer/host, identifies the license, and documents the supply-chain posture. Changes to this file go through the same review gate as architecture changes — adding or removing a direct dependency is a decision, not a tweak.

Companion docs:

- `system-design.md` §4 — the trait shapes these crates implement
- `system-design.md` §10 — security model, including supply-chain expectations
- `.docs/development-plan.md` §2.6 — the substitution table this document expands

---

## 1. Selection rules

A direct dependency lands in `Cargo.toml` only if it satisfies all four:

1. **Maintenance signal in the last 6 months.** No commits, no closed issues, no released versions ⇒ disqualified.
2. **License compatible with Apache-2.0 publication.** MIT, BSD-2/3, Apache-2.0, MPL-2.0, ISC, Zlib accepted. GPL/LGPL/AGPL rejected at the workspace boundary. CC0 / Unlicense rejected on `cargo-deny` policy.
3. **Auditable surface.** A reviewer can read the crate's source and form an opinion in an evening. Crates above ~30 K LoC need a documented rationale and a `cargo-vet` audit.
4. **Replaceable behind a trait.** If the dependency disappears, we can swap it for another implementation without rewriting the core. Anything that violates this is treated as a Tier-1 architectural decision.

Indirect (transitive) dependencies inherit the same rules but are vetted lazily: `cargo-vet` records audits, `cargo-deny` catches license drift.

## 2. Version pinning

| Field | Convention |
|---|---|
| Direct deps | Major-minor pinned (`= "1.42"` style). Patches float. Major bumps are a decision. |
| Build/dev deps | Same as direct. |
| `Cargo.lock` | Committed. Renovate keeps it current; review every patch update. |
| MSRV | Rust 1.85 (stable AFIT and edition 2024 — both required by the modern transitive dependency tree, including `serde_spanned` 1.x, `toml_parser` 1.x, `getrandom` 0.4). Bumped only with a release. |

## 3. The dependency table

### 3.1 Audio capture and platform shims

| Crate | Version | License | Host | Role | Notes |
|---|---|---|---|---|---|
| `objc2` | `0.5` | MIT | github.com/madsmtm/objc2 | macOS Objective-C bindings; replaces `cocoa`/`objc` (both unmaintained) | Active 2026; minimal unsafe surface; well-typed |
| `objc2-foundation` | `0.2` | MIT | madsmtm/objc2 | Foundation framework bindings | Same upstream as `objc2` |
| `coreaudio-tap-rs` | TBD | Apache-2.0 | **to-be-written** in scrybe org | macOS Core Audio Taps binding (`CATapDescription`, `AudioHardwareCreateProcessTap`) | No published crate exists. v0.1 deliverable; vendor in-tree first, publish standalone after v0.1 stabilization. Tracking issue to be opened. |
| `screencapturekit` | `0.3` | MIT OR Apache-2.0 | github.com/svtlabs/screencapturekit-rs | macOS 13.0–14.3 fallback for system audio | Less preferred than Core Audio Taps; only used when `cfg!(target_os="macos")` and OS version below 14.4 |
| `wasapi` | `0.16` | MIT OR Apache-2.0 | github.com/HEnquist/wasapi-rs | Windows WASAPI bindings, including per-process loopback | `cpal` does not expose loopback; we use `wasapi` directly for system audio and `cpal` only for the mic device |
| `cpal` | `0.16` | Apache-2.0 | github.com/RustAudio/cpal | Cross-platform mic input | System audio is platform-specific; `cpal` covers the mic-only path on every desktop OS |
| `pipewire` | `0.9` | MIT | gitlab.freedesktop.org/pipewire/pipewire-rs | Linux PipeWire client | Primary Linux backend on modern distros |
| `libpulse-binding` | `2.28` | LGPL-2.1 | github.com/jnqnfe/pulse-binding-rust | Linux PulseAudio fallback | LGPL is acceptable as a *dynamic* link via the system `libpulse` shared library — the binding crate itself is dual-licensed, the system library is dynamically loaded. `cargo-deny` config has an explicit allow note. |
| `windows-sys` | `0.59` | MIT OR Apache-2.0 | github.com/microsoft/windows-rs | Windows Win32 API bindings | Used for `MoveFileExW`, `WTSRegisterSessionNotification`, `MMNotificationClient` |
| `uniffi` | `0.31` | MPL-2.0 | github.com/mozilla/uniffi-rs | Rust ↔ Kotlin bindings for Android shell | Mozilla-tested for Firefox-on-Android; proc-macro mode (no UDL files) |
| `jni` | `0.21` | MIT OR Apache-2.0 | github.com/jni-rs/jni-rs | Optional fallback for hand-written JNI bridges | Used only where `uniffi` cannot express the contract (e.g. `MediaProjection` permission flow needs raw JNI) |

### 3.2 Speech-to-text and audio processing

| Crate | Version | License | Host | Role | Notes |
|---|---|---|---|---|---|
| `whisper-rs` | `0.16` | MIT OR Apache-2.0 | **codeberg.org/tazz4843/whisper-rs** | Whisper.cpp Rust bindings | Primary host moved off GitHub; the GitHub mirror is archived. We pin a known-good commit and **vendor the source** under `vendor/whisper-rs/` so a Codeberg outage does not block release. Track Codeberg release feed manually. |
| `voice_activity_detector` | `0.2` | MIT | github.com/nkeenan38/voice_activity_detector | Silero v5 VAD via ONNX runtime | Replaces `webrtc-vad-rs` (last release 2019, dead). Active 2025-08. ~1 ms / 30 ms chunk on a single CPU thread. |
| `sherpa-rs` | `0.6` | Apache-2.0 | github.com/thewh1teagle/sherpa-rs | Optional Parakeet TDT v2/v3 STT | Behind `--features parakeet-local`. v0.4 deliverable. English-priority; Whisper remains default for multilingual. |
| `ort` | `2.0` | MIT OR Apache-2.0 | github.com/pykeio/ort | ONNX runtime bindings (used transitively by `voice_activity_detector` and `pyannote-onnx`) | Direct dep declared at workspace level so we can pin the runtime version once. |
| `pyannote-onnx` | `3.1` | MIT | github.com/pengzhendong/pyannote-onnx | Neural speaker diarization fallback | Behind `--features diarize-pyannote`. v0.5 deliverable. |
| `opus` | `0.3` | BSD-3 | github.com/RustAudio/opus | Opus encoder for `audio.opus` | 32 kbps default per `system-design.md` §6. |
| `ogg` | `0.9` | BSD-3 | github.com/RustAudio/ogg | Ogg container for the Opus stream | 1-second page flush per `system-design.md` §8.3. |
| `rubato` | `0.16` | MIT | github.com/HEnquist/rubato | Resampling to 16 kHz | Active, well-benchmarked, supports SIMD. |

### 3.3 Async runtime, HTTP, serialization

| Crate | Version | License | Host | Role | Notes |
|---|---|---|---|---|---|
| `tokio` | `1.42` | MIT | github.com/tokio-rs/tokio | Async runtime | Feature subset: `["rt-multi-thread", "macros", "fs", "io-util", "sync", "signal", "time"]`. We deliberately exclude `["full"]` to save ~200 KB and to keep the surface auditable. `"time"` is required for `tokio::time::sleep` in the retry/backoff helper (`scrybe-core::providers::retry`) and for `tokio::time::timeout` in the hook dispatcher; adding any further feature requires updating this doc. |
| `async-trait` | `0.1` | MIT OR Apache-2.0 | github.com/dtolnay/async-trait | Async functions in traits | Will be removed when stable AFIT covers all our trait shapes; tracked. |
| `futures` | `0.3` | MIT OR Apache-2.0 | rust-lang/futures-rs | `join_all`, stream combinators | Used in the hook dispatcher and pipeline. |
| `reqwest` | `0.12` | MIT OR Apache-2.0 | github.com/seanmonstar/reqwest | HTTP client for OpenAI-compat providers and webhook hook | Default features disabled; we enable `["rustls-tls", "json", "stream"]`. Excludes native-tls to avoid OS-trust-store divergence. |
| `rustls` | `0.23` | MIT OR Apache-2.0 OR ISC | github.com/rustls/rustls | TLS via `reqwest` | No certificate pinning per §10 threat model. |
| `serde` | `1` | MIT OR Apache-2.0 | github.com/serde-rs/serde | Serialization | With `derive`. |
| `serde_json` | `1` | MIT OR Apache-2.0 | serde-rs/json | JSON for HTTP and `transcript.partial.jsonl` | |
| `toml` | `0.8` | MIT OR Apache-2.0 | github.com/toml-rs/toml | `meta.toml` and `config.toml` | Strict mode (`#[serde(deny_unknown_fields)]`); unknown keys rejected. Pinned at `0.8` because `0.9` pulls `serde_spanned 1.x` / `toml_parser 1.x`, which require Rust 1.85's edition-2024 transitive features in a cargo-feature-gate configuration that the workspace is not yet ready to consume cleanly. Reassess at the next minor bump. |

### 3.4 Storage, config, time, IDs

| Crate | Version | License | Host | Role | Notes |
|---|---|---|---|---|---|
| `tempfile` | `3` | MIT OR Apache-2.0 | github.com/Stebalien/tempfile | `NamedTempFile::new_in` for atomic writes | See `system-design.md` §8.3. |
| `libc` | `0.2` | MIT OR Apache-2.0 | github.com/rust-lang/libc | `fcntl(F_FULLFSYNC)` on macOS for true platter durability; raw POSIX syscalls in storage and capture adapters | Unix-only direct dep (`#[cfg(unix)]`). `tokio::fs::File::sync_all` and `std::fs::File::sync_all` call `fsync(2)` on macOS, which only commits to drive cache; `libc::fcntl` with `F_FULLFSYNC` is the only correct primitive. See `system-design.md` §8.3. |
| `fs2` | `0.4` | MIT OR Apache-2.0 | github.com/danburkert/fs2-rs | Cross-platform `flock`/`LockFileEx` for `pid.lock` | Maintained but slow-moving; vetted in `cargo-vet`. |
| `directories` | `5` | MIT OR Apache-2.0 | github.com/dirs-dev/directories-rs | Platform-conventional config and data paths | macOS → `~/Library/Application Support/dev.scrybe.scrybe/`; Windows → `%APPDATA%\scrybe\scrybe\`; Linux → `$XDG_CONFIG_HOME/scrybe/`. |
| `chrono` | `0.4` | MIT OR Apache-2.0 | github.com/chronotope/chrono | Timestamps in `meta.toml` and folder names | `serde` feature for TOML round-trip. |
| `ulid` | `1` | MIT | github.com/dylanhart/ulid-rs | Folder-suffix uniqueness within a minute | Replaces UUID v4 here because it sorts lexicographically and stays grep-friendly. |
| `git2` | `0.20` | binding crate: MIT OR Apache-2.0; statically links libgit2 (GPL-2.0-with-linking-exception) | github.com/rust-lang/git2-rs (binding) and github.com/libgit2/libgit2 (C library) | `Hook::Git` reference impl | The Rust binding crate is dual-licensed MIT/Apache-2.0; the underlying libgit2 C library is GPL-2.0 with a linking exception that explicitly permits static linking into proprietary or differently-licensed binaries. Both layers are documented in `cargo-deny.toml`. |

### 3.5 CLI, errors, observability

| Crate | Version | License | Host | Role | Notes |
|---|---|---|---|---|---|
| `clap` | `4.5` | MIT OR Apache-2.0 | github.com/clap-rs/clap | CLI parsing | Derive macro mode. |
| `clap_complete` | `4.5` | MIT OR Apache-2.0 | clap-rs/clap | Shell completion generation for bash/zsh/fish/PowerShell | `scrybe completions <shell>` subcommand. |
| `thiserror` | `1` | MIT OR Apache-2.0 | github.com/dtolnay/thiserror | Error type derive in libs | See `system-design.md` §4.6. |
| `anyhow` | `1` | MIT OR Apache-2.0 | github.com/dtolnay/anyhow | CLI error context | CLI binary only; never in libs. |
| `tracing` | `0.1` | MIT | github.com/tokio-rs/tracing | Structured logging | Subscribe at CLI; library code only emits, never configures. |
| `tracing-subscriber` | `0.3` | MIT | tokio-rs/tracing | Subscriber config in CLI | env-filter feature; default off in tests. |

### 3.6 Calendar, hooks, indexing

| Crate | Version | License | Host | Role | Notes |
|---|---|---|---|---|---|
| `ical` | `0.11` | MIT OR Apache-2.0 | github.com/Peltoche/ical-rs | `.ics` calendar parsing | v0.2 deliverable; behind `--features context-ics`. |
| `tantivy` | `0.24` | MIT | github.com/quickwit-oss/tantivy | Optional search index | v0.6 deliverable; behind `--features hook-tantivy`. Index lives at `~/scrybe/.index/`, regenerable, never source-of-truth. |
| `hmac` | `0.12` | MIT OR Apache-2.0 | RustCrypto/MACs | Webhook HMAC-SHA256 signing | RustCrypto family; `cargo-vet` audited. |
| `sha2` | `0.10` | MIT OR Apache-2.0 | RustCrypto/hashes | Model-checksum verification + HMAC | Same family. |
| `global-hotkey` | `0.7` | Apache-2.0 OR MIT | github.com/tauri-apps/global-hotkey | Cross-platform global hotkey | Used by `scrybe-cli` desktop binaries; has its own permission model on macOS but does not require Screen Recording. |
| `tray-icon` | `0.20` | Apache-2.0 OR MIT | github.com/tauri-apps/tray-icon | Cross-platform system-tray icon | macOS uses `objc2` under the hood; Windows uses NotifyIcon; Linux uses libappindicator. |

### 3.7 Test and dev dependencies

| Crate | Version | License | Role |
|---|---|---|---|
| `proptest` | `1.5` | MIT OR Apache-2.0 | Property tests for chunker, channel split, transcript merge |
| `wiremock` | `0.6` | Apache-2.0 | HTTP mocks for `OpenAiCompatSttProvider` and webhook hook |
| `criterion` | `0.7` | Apache-2.0 OR MIT | Benchmarks; CI fails on >10% regression vs prior tag |
| `mock_instant` | `0.5` | MIT | Deterministic `Instant` for retry-policy tests |
| `assert_fs` | `1` | MIT OR Apache-2.0 | Filesystem assertions in storage tests |
| `assert_cmd` | `2` | MIT OR Apache-2.0 | CLI integration tests |
| `pretty_assertions` | `1` | MIT OR Apache-2.0 | Diff-based assertion failure output |

### 3.8 Build and packaging tooling

These are not Rust dependencies; they run at release time. Versions pinned in `rust-toolchain.toml` and the `cargo-dist` workflow.

| Tool | Version | Role |
|---|---|---|
| `cargo-dist` | `0.25` | GitHub Release artifact pipeline; cross-platform unsigned-binary builds |
| `cargo-deb` | `2.6` | `.deb` package generation for Linux distros |
| `cargo-wix` | `0.3` | Windows MSI generation (unsigned through v1.0) |
| `cargo-ndk` | `3.5` | Android NDK toolchain wrapper |
| `cargo-vet` | `0.10` | Per-dep audit ledger; required for every direct dep |
| `cargo-audit` | `0.21` | RustSec advisory scanning; CI gate |
| `cargo-deny` | `0.18` | License + advisory + bans + sources policy |
| `cargo-sbom` | `0.9` | CycloneDX SBOM attached to each GitHub Release |
| `cosign` | `2.4` | Keyless signing of release tarballs via GitHub Actions OIDC (artifact-level CI provenance only; not OS-level code signing) |

OS-level code-signing toolchains (Apple `codesign` / `notarytool`, Microsoft `signtool`) are intentionally absent. macOS and Windows artifacts ship unsigned through v1.0, with `INSTALL.md` documenting `xattr -dr com.apple.quarantine` (macOS) and SmartScreen "More info → Run anyway" (Windows). Rationale: vendor-tied trust dependencies are deferred until after the project has demonstrated longevity. See `.docs/development-plan.md` §13.1.

## 4. Vendored sources

The following are pulled into `vendor/` rather than fetched from a registry:

| Source | Why | Update cadence |
|---|---|---|
| `whisper-rs` (Codeberg) | Primary host is Codeberg; vendoring isolates us from Codeberg outages and rate-limits | Manual: review Codeberg releases monthly |
| `coreaudio-tap-rs` | Crate does not exist on crates.io; we are writing it | Continuous through v0.1 stabilization |

Vendored sources are subject to the same `cargo-vet` audits as direct deps. Updating a vendored source is a documented commit with the upstream commit-SHA in the message.

## 5. Excluded dependencies (and why)

| Crate | Excluded because |
|---|---|
| `webrtc-vad-rs` | Last release 2019; effectively unmaintained. Replaced by `voice_activity_detector` (Silero v5) |
| `cocoa` / `objc` (madsmtm/cocoa-rs) | Superseded by `objc2`; no new releases since 2023 |
| Old `whisper-rs` GitHub mirror | Mirror is archived; primary host moved to Codeberg |
| `cpal` (for system audio) | Does not expose Windows loopback or macOS Core Audio Taps; only used for mic input |
| `tokio` `["full"]` feature | ~200 KB binary cost; we don't need the full feature set, and listing the subset documents what we *do* need |
| `native-tls` (via reqwest) | OS trust store divergence between platforms creates non-reproducible TLS behavior; `rustls` is preferred |
| GUI toolkits (`egui`, `iced`, `tauri`, etc.) | Not building a GUI in v1.0; tray icon and CLI are the entire desktop UX |
| Embedded databases (`sqlite`, `sled`, `redb`, `libsql`) | Filesystem-as-database is the architectural commitment; introducing an embedded DB is a Tier-1 architecture change, not a dependency tweak |
| `chrono-tz` / `time-tz` | Timestamps stored in UTC; timezone display is the renderer's concern |
| OpenSSL bindings | Conflicts with `rustls`; would force a second TLS stack |

## 6. License posture

The workspace is published under Apache-2.0. `cargo-deny` blocks:

- AGPL, GPL-3.0 (without linking exception), SSPL, BUSL, Commons Clause, ELv2, Confluent Community License.
- CC0-1.0 (defective in some jurisdictions; we require an explicit licensor).
- "MIT-0" / "Unlicense" (rejected for liability reasons; we want a recognized warranty disclaimer).

Allowed via explicit allow-list:

- MIT, BSD-2-Clause, BSD-3-Clause, ISC, Zlib, MPL-2.0.
- Apache-2.0, Apache-2.0 WITH LLVM-exception.
- LGPL-2.1 / LGPL-3.0 *only* via dynamic linking to a system library (currently only `libpulse-binding`).
- GPL-2.0 WITH linking exception (`git2-rs` / `libgit2`).

Each exception is documented in `cargo-deny.toml` with a justification.

## 7. Supply-chain hardening

| Control | Implementation |
|---|---|
| Reproducible builds | `SOURCE_DATE_EPOCH`, `--remap-path-prefix`, pinned `rust-toolchain.toml`. Verified by CI rerun on identical commit producing identical artifact SHA256. |
| `cargo-vet` audits | Required for every direct dep before it lands; transitive deps audited on a budget per release |
| `cargo-audit` | RustSec advisory scan on every PR; CI fails on any HIGH or CRITICAL |
| `cargo-deny` | License + advisory + bans + sources on every PR |
| `cargo-sbom` | CycloneDX SBOM attached to every GitHub Release |
| `cosign` keyless | Signs every artifact via GitHub Actions OIDC; verification instructions in `INSTALL.md` |
| Vendored hash verification | `cargo --frozen` + `Cargo.lock` + vendored-source commit-SHA recorded in commit messages |

A direct dep with a HIGH-severity advisory and no patched version is a stop condition; the project halts and a maintainer decides between vendor-and-patch, swap, or pause the release.

## 8. Update policy

| Class | Cadence | Gate |
|---|---|---|
| Security patches (RustSec) | Within 7 days of advisory | CI green; release branch |
| Patch updates (no API change) | Weekly via Renovate | CI green; merged automatically if green |
| Minor updates (additive) | Reviewed at each minor release | Manual review of CHANGELOG |
| Major updates | At a minor release boundary | Update this doc; `cargo-vet` re-audit |
| New direct dep | Update this doc, open a PR | Maintainer review + `cargo-vet` audit |

This is a living document; every change to the dependency surface updates this file in the same PR.

---

*Pinned for v1.0 entry. Future revisions: at every minor release, plus on any direct-dep addition or removal.*
