# Changelog

All notable changes to scrybe are documented here. The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html) within the stability tiers documented in `docs/system-design.md` §12.

## [1.0.2] — 2026-05-02

Two v1.0.1 bugs fixed; both surfaced from the manual mic-capture smoke test in PR #36. No new functional surface; this is a bug-fix patch within the v1.0.x stream per `MAINTENANCE.md` §1.

The publish posture from v1.0 carries forward unchanged: only `scrybe` (the placeholder) publishes to crates.io. No Tier-1 surface changes — the `Encoder` trait shape is unchanged; the only public addition is the `default_session_encoder` factory + the feature-gated `OggOpusEncoder` type, both Tier-2 (per `system-design.md` §12.3, `scrybe-core::pipeline::*` internals are Tier 3, and the trait + the new factory satisfy Tier-2 best-effort signatures).

### Added

- `scrybe-core::pipeline::encoder::OggOpusEncoder` behind the new `encoder-opus` feature on `scrybe-core`. Real Ogg-Opus encoder closing the v0.1 carryover documented in `.docs/development-plan.md` §7.6.4 E-6 where `audio.opus` was raw f32 PCM bytes under an `.opus` filename, rejected by ffmpeg / vlc / browser audio tags. Validates `EncoderConfig.sample_rate` against Opus's natively supported rates (8/12/16/24/48 kHz) at construction; rejects others with `PipelineError::OpusEncode`. Buffers f32 PCM into 20-ms frames (RFC 7845 §1 modal VoIP frame size), encodes via libopus through the `opus 0.3` Rust binding (`Application::Voip`, configurable bitrate), wraps Opus packets in an Ogg container per RFC 7845 via the `ogg 0.9` pure-Rust crate. Writes the OpusHead + OpusTags header pages on the first `push_pcm` call; subsequent packets ride `EndPage` boundaries every `packets_per_page` for storage-layer `append_durable` semantics. Owns the OGG output buffer in `Arc<Mutex<Vec<u8>>>` so the `Encoder: Send` trait bound holds across the cpal-callback / pipeline-task boundary.
- `scrybe-core::pipeline::encoder::default_session_encoder()` — public factory that picks `OggOpusEncoder` when the `encoder-opus` feature is built and falls back to `NullEncoder` otherwise. The session orchestrator (`scrybe-core::session::drive_session`) calls this instead of constructing `NullEncoder` directly, so a release build with the feature emits a real audio file while the deterministic test path stays intact.
- `scrybe-cli` `encoder-opus` cargo feature forwarding to `scrybe-core/encoder-opus`. Local builds with `--features cli-shell,hook-git,mic-capture,whisper-local,encoder-opus` emit a real `audio.opus` decodable by every standard tool. The cargo-dist release tarball stays on `NullEncoder` until the release runner has libopus headers verified — same rationale as the `whisper-local` deferral.
- Six new unit tests on `scrybe-core::pipeline::encoder` (gated to `encoder-opus`): rejects-unsupported-sample-rate, rejects-six-channels, emits-ogg-magic-on-first-push, buffers-below-one-opus-frame, finish-emits-end-stream-marker, default-session-encoder-returns-ogg-opus-with-feature. One new test gated to no-feature: default-session-encoder-returns-null-encoder-without-feature.
- Two new unit tests on `scrybe-core::providers::whisper_local` for the model-name regression: test-whisper-local-provider-name-reflects-actual-model-file (the manual-test smoke from PR #36 encoded as a regression guard) and test-derive-model-label-handles-pathological-inputs (covers multi-extension stems, absolute paths, no-extension paths, empty-path fallback).
- `INSTALL.md` "Record from a real microphone with local Whisper transcription" section updated: build invocation now includes `encoder-opus`; an `ffprobe audio.opus` line shows the user how to verify the file is real Opus; a paragraph documents that `meta.toml` now records the actual loaded model name.

### Changed

- All workspace crates bump from `1.0.1` to `1.0.2`. Path-dep version pins follow.
- `scrybe-core::providers::whisper_local::WhisperLocalConfig::new()` derives `model_label` from the model file's stem (e.g. `ggml-base.en.bin` → `ggml-base.en`) instead of returning the hardcoded `"large-v3-turbo"` string. Closes the v1.0.1 reporting bug where `meta.toml [providers].stt` recorded `whisper-local:large-v3-turbo` regardless of which model file was actually loaded — the manual-test smoke in PR #36 surfaced this discrepancy. Callers that want a different reporting label can still override the `model_label` field directly after `new()`.
- `scrybe-core::session::drive_session` constructs the audio encoder via `default_session_encoder` instead of `NullEncoder::new` directly, so the encoder choice now flows through a single public entry point.
- `scrybe-core` LoC ceiling raised from 8500 to 9000 in `scripts/check-loc-budget.py` to absorb `OggOpusEncoder` + tests + the new `derive_model_label` helper (~340 LoC).
- `MAINTENANCE.md` §1 canonical feature list updated to include `encoder-opus`. Documented as anticipated since v0.5 per the `encoder.rs` module docstring; landing it at v1.0.2 is a v1.0.x bug fix closing the v0.1 carryover, not scope expansion.

### Deprecated / Removed

- Nothing.

### Security

- `cargo audit` and `cargo deny` policies are unchanged from v1.0.1. The two new transitive dependencies — `opus 0.3` and `ogg 0.9` — are local-only (no network surface) and not on the `egress-audit` denylist; `scripts/check-egress-baseline.py` reports clean.
- `opus 0.3` links libopus via `audiopus_sys 0.2` (vendored on macOS, system package elsewhere). License is BSD-3-Clause (libopus) + MIT (`opus` Rust crate); both are on the existing `cargo deny` allowlist.
- `ogg 0.9` is pure Rust, BSD-3-Clause licensed.

### Known limitations

- **`encoder-opus` requires 48 kHz mono input by default.** `EncoderConfig::default()` uses 48 kHz which matches cpal's default input rate on macOS. Hosts where the default input device runs at 44.1 kHz (some Linux setups, some Windows audio devices) will need either a per-device cpal config or a runtime resample to 48 kHz before the encoder; both are v1.0.x → v1.1 follow-ups.
- **Real LLM in `scrybe record`.** Notes step still uses the stub. Wiring an Ollama / openai-compat LLM into `run_with_stop` is a separate v1.x deliverable (unchanged from v1.0.1).
- **System audio capture in `scrybe record`.** `scrybe-capture-mac::MacCapture` is hardware-validated but still not consumed by the CLI (unchanged from v1.0.1).
- **Tray icon and global hotkey.** `--shell` still prints an advisory and runs the headless path (unchanged from v1.0.1).
- **`PermissionModel::Microphone` not added.** Tier-1 enum frozen at v1.0; v2.0 candidate (unchanged from v1.0.1).
- **Reproducibility and `cargo-vet` lanes unchanged from v1.0** — both remain advisory; promotion to blocking remains a v1.0.x → v1.1 deliverable.

### Workspace

- 8 crates (unchanged from v1.0.1).
- Publish posture unchanged.
- Test counts: 502 default / 507 with `encoder-opus` enabled (was 499 at v1.0.1; +3 whisper-name tests, +5 encoder-opus tests, +0/–0 default tests).

### Contributors

- Maintainer: Mathews Tom.

[1.0.2]: https://github.com/Mathews-Tom/scrybe/releases/tag/v1.0.2

## [1.0.1] — 2026-05-02

Closes the v0.1 mic-only path documented in `.docs/development-plan.md` §7.2 ("scrybe record — start session; press hotkey or --title flag; mic-only capture; live append to transcript.md; run Whisper after each chunk") that shipped under the synthetic 440 Hz sine generator and stub providers through v1.0. Two opt-in flags surface real audio capture and real Whisper transcription on `scrybe record`.

The publish posture from v1.0 carries forward unchanged: only `scrybe` (the placeholder) publishes to crates.io. No Tier-1 surface changes — the orchestrator's `S: SttProvider` generic stays sized; CLI-side enum dispatch (`CliStt`) bridges the stub and live providers without touching `scrybe-core`'s public surface.

### Added

- `scrybe-capture-mic` workspace crate — cross-platform microphone adapter implementing `scrybe_core::capture::AudioCapture` via cpal 0.15. Mirrors the `MacCapture` / `LinuxCapture` / `WindowsCapture` shape: `Arc<Mutex<SharedState>>` ownership, `tokio::sync::mpsc::unbounded_channel` plumbing, single-consumer `frames()` semantics, in-tree `UnboundedReceiverStream` shim, `inject_for_test` / `close_for_test` integration-test surface. The cpal `Stream` is `!Send`; the live binding owns it on a dedicated `scrybe-mic-capture` OS thread with shutdown via `std::sync::mpsc::channel::<()>`. F32 / I16 / U16 sample formats convert to `f32` per `AudioFrame` at callback time using `cpal::FromSample`. Behind the `live-mic` feature; without it, `MicCapture::start()` returns `CaptureError::PermissionDenied` so cross-platform CI hosts that cannot link cpal still compile the crate. New crate listed in the root `Cargo.toml` with a 1500 LoC ceiling in `scripts/check-loc-budget.py`.
- `scrybe record --source {synthetic,mic}` flag. `synthetic` (default) keeps the deterministic 440 Hz sine-source path so CI smoke-tests stay hermetic. `mic` opens the host's default input device through `scrybe-capture-mic::MicCapture`. Behind the new `mic-capture` feature on `scrybe-cli` which forwards to `scrybe-capture-mic/live-mic`. Without the feature, `--source mic` returns `CaptureError::PermissionDenied` per the adapter contract. The `mic_keepalive: Option<MicCapture>` binding in `run_with_stop` keeps the dedicated cpal capture thread alive for the lifetime of `run_session`.
- `scrybe record --whisper-model <PATH>` flag. When set AND the binary is built with `--features whisper-local`, the STT provider becomes `WhisperLocalProvider` against the supplied `.bin` / `.gguf` weights. Without the feature, supplying the flag errors at start time (`--whisper-model … provided but binary built without --features whisper-local`) rather than silently falling back to the stub. `*.partial` paths are rejected per the existing `WhisperLocalProvider::new` contract; the CLI surfaces the rejection as a context-chained `loading whisper.cpp model at <path>` error.
- `mic-capture` cargo feature on `scrybe-cli`, forwarding to `scrybe-capture-mic/live-mic`. Off by default so the cpal transitive dependency tree (CoreAudio / ALSA / WASAPI bindings) stays out of default-feature builds; the `egress-audit` CI lane stays green.
- `INSTALL.md` "Record from a real microphone with local Whisper transcription" section. Documents the two-feature build (`cargo install --features cli-shell,hook-git,mic-capture,whisper-local`), the model-download recipe, the Microphone-permission grant on first run, the four common whisper.cpp model sizes with RAM use and realtime factor on M1 Pro, and the `*.partial`-rejection guard.
- Three new unit tests on `scrybe-cli`: `test_capture_source_arg_default_is_synthetic`, `test_build_stt_provider_returns_stub_when_no_model_path_supplied`, plus two mutually-exclusive feature-gated tests covering the no-feature error path and the live-feature `*.partial` rejection. Six new unit tests on `scrybe-capture-mic` covering capabilities, default constructor, single-consumer `frames()`, inject round-trip, and feature-gated start/stop semantics.

### Changed

- All workspace crates bump from `1.0.0` to `1.0.1`. Path-dep version pins follow. The SemVer guard test in `scrybe::tests::test_version_constant_matches_cargo_metadata` continues to assert `starts_with("1.0.")` — 1.0.1 satisfies the existing lock, so no test change.
- `scrybe-cli` LoC ceiling raised from 2300 to 2500 in `scripts/check-loc-budget.py` to absorb the new flag wiring + three new tests; the new code is ~140 LoC including tests. The bump rationale is recorded inline at the ceiling.
- `scrybe-cli/src/commands/record.rs` module docstring rewritten to describe the v1.0.1 mic + Whisper flags instead of the v0.1 deferral.

### Deprecated / Removed

- Nothing.

### Security

- `cargo audit` and `cargo deny` policies are unchanged from v1.0.0 (same advisory ignores, same license clarifies). cpal pulls in `coreaudio-sys` / `alsa-sys` / `windows-sys` per platform — none on the `egress-audit` denylist; the lane reports clean.
- `scrybe record --source mic` triggers the OS Microphone-permission prompt on macOS the first time it runs. The permission grant lives in System Settings → Privacy & Security → Microphone and is per-binary; rebuilding the binary requires re-granting.
- `scrybe record --whisper-model <PATH>` rejects `*.partial` paths up-front. An interrupted whisper.cpp model download (which writes `<name>.bin.partial` then renames to `<name>.bin`) cannot silently produce a corrupt transcript.

### Known limitations

- **System audio capture is not yet wired into `scrybe record`.** The `scrybe-capture-mac::MacCapture` adapter (Core Audio Taps, hardware-validated at v0.1 per PR #20) exists but is not consumed by `scrybe record`. v1.0.1 ships mic-only; system+mic stereo capture (the v0.2-scoped channel-split path documented in `.docs/development-plan.md` §8.2 deliverable #1) remains a follow-up.
- **Tray icon and global hotkey not implemented.** `scrybe record --shell` still prints an advisory and runs the headless path (SIGINT to stop). Wiring `scrybe-cli::tray` and `scrybe-cli::hotkey` into `run_with_stop` is a separate v1.x deliverable.
- **Notes step still uses the stub LLM.** `WhisperLocalProvider` produces real transcripts; the LLM summary at session-end remains the canned stub. Wiring an Ollama / openai-compat LLM into `scrybe record` is a separate v1.x deliverable.
- **`PermissionModel::Microphone` not added.** The Tier-1 enum is frozen at v1.0; `MicCapture` reuses the closest existing variant per platform (`CoreAudioTap` on macOS, `PipeWirePortal` on Linux, `WasapiLoopback` on Windows). The field is informational and does not gate behavior. A `Microphone` variant is a v2.0 breaking-change candidate.
- **Reproducibility and `cargo-vet` lanes unchanged from v1.0.0** — both remain advisory; promotion to blocking remains a v1.0.x → v1.1 deliverable.

### Workspace

- 8 crates (was 7 at v1.0.0): `scrybe`, `scrybe-core`, `scrybe-capture-mac`, `scrybe-capture-linux`, `scrybe-capture-win`, `scrybe-capture-mic` (new), `scrybe-android`, `scrybe-cli`.
- Publish posture unchanged: only `scrybe` publishes to crates.io. `scrybe-capture-mic` is `publish = false` like the other adapter crates.

### Contributors

- Maintainer: Mathews Tom.

[1.0.1]: https://github.com/Mathews-Tom/scrybe/releases/tag/v1.0.1

## [1.0.0] — 2026-05-02

First stable release. v1.0 freezes the Tier-1 surface documented in `docs/system-design.md` §12.1 and commits to a six-month no-scope-expansion window per `MAINTENANCE.md` §1. The functional surface is unchanged from v0.9.0-rc1; this is the stability cut, not a feature release. Workspace crates bump from `0.9.0-rc1` to `1.0.0` and the SemVer guard test in `scrybe::tests::test_version_constant_matches_cargo_metadata` re-locks at the `1.0.x` line.

The publish posture from v0.1.0 / v0.2.0 / v0.3.0 / v0.4.0 / v0.5.0 / v0.9.0-rc1 carries forward unchanged: only `scrybe` (the placeholder) publishes to crates.io. The Tier-1 freeze on `AudioCapture`, `MeetingContext`, `LifecycleEvent`, `ConsentAttestation`, the `meta.toml` schema, and the storage-layout invariants is now active — breaking these requires a v2.0 major bump and a six-month deprecation window with a `LifecycleEvent::SchemaDeprecated` warning emitted on every load through the deprecation cycle.

### Added

- `MAINTENANCE.md` — public commitments for the v1.0 series. Documents the six-month scope freeze and what it does and does not cover, the three-tier stability matrix in operational language, the issue-triage SLA (7 days for bugs, 72 hours for security), the 6-week minor-release cadence per `docs/system-design.md` §12.5, the unchanged Option-B publish posture and the unchanged unsigned-binary distribution-trust posture, the contributor expectations (conventional commits, 90% / 80% / 95% coverage thresholds, no CLA), and the bus-factor mitigation (Apache-2.0 forkability + self-contained architecture).

### Changed

- All workspace crates bump from `0.9.0-rc1` to `1.0.0`. Path-dep version pins follow. `scrybe::tests::test_version_constant_matches_cargo_metadata` updated to lock against the `1.0.x` line; the comment block on the assertion now warns "loosen when bumping to the next minor".
- `INSTALL.md` verification recipes (cosign verify-blob, local reproducibility recipe, Linux + Windows audit-friendly `cargo install --git ... --tag`) retarget at `v1.0.0`. The reproducibility-advisory narrative shifts from "v0.9.x → v1.0 follow-up" to "v1.0.x → v1.1 follow-up".
- `.github/workflows/reproducibility.yml` lane-status comment, advisory `::warning::` annotation, and "promotion to a blocking gate" deliverable retarget at v1.0.0 / v1.0.x → v1.1.
- `.github/workflows/ci.yml` `vet` job comment notes the `cargo-vet` lane stays advisory through v1.0; the audit-completion deliverable retargets at v1.0.x → v1.1 with a cross-reference to `MAINTENANCE.md` §5.
- `supply-chain/{config.toml,audits.toml}` comments retarget the `cargo-vet` lifecycle at v1.0; the historical "shipped at v0.9.0-rc1" anchors remain because they are factually correct breadcrumbs.

### Deprecated / Removed

- Nothing.

### Security

- `cargo audit` and `cargo deny` policies are unchanged from v0.9.0-rc1 (same advisory ignores, same license clarifies).
- `cosign verify-blob` over `SHA256SUMS.txt` remains the cryptographic anchor for distribution trust through v1.0. Native Apple Developer ID notarization and Windows code-signing certificates remain explicitly out of scope per `MAINTENANCE.md` §5; the cost-benefit reads against shipping a vendor-tied trust dependency this early in the project's life.
- Security disclosures: 72-hour first-response SLA, 7-day fix-or-mitigate target for High/Critical, 30-day for Medium. Use private GitHub security advisories (`Security` tab → `Report a vulnerability`); do not post in public issues. Documented in `MAINTENANCE.md` §3.

### Known limitations

- **Reproducibility lane is advisory at v1.0.0.** The `reproducibility.yml` lane reports tarball-level divergences on `macos-14` despite `SOURCE_DATE_EPOCH`, `--remap-path-prefix`, `-Wl,-no_uuid`, and a pinned `1.95.0` toolchain. The remaining non-determinism sources need diffoscope-driven investigation. Promotion to a blocking gate is a v1.0.x → v1.1 deliverable per `MAINTENANCE.md` §5; the lane uploads both legs' artifacts on every run so the comparison work is unblocked.
- **Reproducibility lane is macOS-only.** `[workspace.metadata.dist] targets` continues to enumerate only Apple Silicon and Intel macOS through v1.0. Linux + Windows reproducibility lanes land alongside the `cargo deb` / `cargo wix` packaging work in the v1.0.x stream.
- **Direct-dep cargo-vet audits not yet committed.** The wiring is in place; the audit work is a v1.0.x → v1.1 follow-up. The expected first batch covers crates the imported feeds do not vouch for — `objc2-core-audio*` (Apple bindings) and `whisper-rs` (vendored Codeberg primary) are the leading candidates per `docs/dependency-decisions.md`.
- **Package-manager templates not yet submitted.** Submission to Homebrew tap / Scoop bucket / AUR / Flathub / F-Droid is a maintainer action per the stop-condition policy. Templates land in-tree at `packaging/`; the first submission round is a v1.0.x stream deliverable per `MAINTENANCE.md` §1.
- **MSI (`cargo-wix`) and `.deb` (`cargo-deb`) artifacts not yet emitted.** The Scoop and AUR templates point at tarball paths that the cargo-dist matrix produces today on macOS only; Linux / Windows native artifacts land alongside the cargo-dist target expansion in the v1.0.x stream.
- **Unsigned macOS / Windows binaries.** Apple Developer ID notarization and Windows Authenticode certificates remain out of scope through v1.x per `MAINTENANCE.md` §5. Users handle Gatekeeper's "Apple cannot verify" prompt and Windows SmartScreen's "Run anyway" path manually per `INSTALL.md`.

### Workspace

- 7 crates (unchanged from v0.9.0-rc1): `scrybe`, `scrybe-core`, `scrybe-capture-mac`, `scrybe-capture-linux`, `scrybe-capture-win`, `scrybe-android`, `scrybe-cli`.
- Publish posture unchanged: only `scrybe` publishes to crates.io. `scrybe-core`, `scrybe-cli`, and the four capture adapters keep `publish = false`. Downstream users install via the cargo-dist tarballs, the `curl | sh` installer one-liner, or `cargo install --git https://github.com/Mathews-Tom/scrybe scrybe-cli --tag v1.0.0 --features cli-shell,hook-git`. Promoting `scrybe-core` to a published crate is a v1.0.x → v1.1 consideration deferred under the §1 no-scope-expansion commitment in `MAINTENANCE.md`.

### Contributors

- Maintainer: Mathews Tom.

[1.0.0]: https://github.com/Mathews-Tom/scrybe/releases/tag/v1.0.0

## [0.9.0-rc1] — 2026-05-02

Reproducible builds, supply-chain attestation, and cross-platform packaging templates land in the v0.9.0 release-candidate stream per `.docs/development-plan.md` §13. The functional surface is unchanged from v0.6.0; this is the distribution-readiness release. Three new capabilities ship together: bit-equality verification of the cargo-dist tarballs across runner instances, cosign keyless OIDC signing of the release SHA256 manifest and CycloneDX SBOM, and in-tree templates for the five downstream package managers (Homebrew, Scoop, AUR, Flatpak, F-Droid) named in §13.1. The cargo-vet wiring lands as an advisory CI lane; promotion to a blocking gate is a v1.0 deliverable.

The publish posture from v0.5.0 carries forward unchanged: only `scrybe` (the placeholder) publishes to crates.io. Workspace crates bump from `0.5.0` to `0.9.0-rc1`, skipping the unrealised `0.6.0`/`0.7.0`/`0.8.0` Cargo versions; the v0.6.0 git tag was a docs+infrastructure tag with no Cargo manifest bump. The `0.9.x-rc` series concludes when §13 exit criteria are uniformly green; the `0.9.0` final cut and `1.0.0` immediately follow.

### Added

- `.github/workflows/reproducibility.yml` — runs each release tarball target through two independent `dist build` invocations from divergent workspace paths (`scrybe/` and `scrybe-rebuild/`) on the same `macos-14` runner and compares SHA256 across legs. Triggers on `workflow_dispatch`, on PRs touching `release.yml` / `Cargo.toml` / `Cargo.lock` / `rust-toolchain.toml`, and on a weekly Sunday 06:00 UTC schedule. Path divergence catches `--remap-path-prefix` regressions; the `SOURCE_DATE_EPOCH=1714464000`, `-Wl,-no_uuid` linker flag, and pinned-toolchain inputs match `release.yml`. The lane runs in **advisory mode** at v0.9.0-rc1 — the four reproducibility inputs are not yet sufficient to make cargo-dist tarballs bit-identical on `macos-14`, and mismatches emit `::warning::` annotations rather than failing the job. Both legs' artifacts upload on every run so a maintainer can run `diffoscope` between them to localise the residual non-determinism. Promotion to a blocking gate is a v0.9.x → v1.0 deliverable.
- CycloneDX SBOM generation in `release.yml` via `cargo-cyclonedx 0.5.7`. Emitted at release time as `scrybe-cli-sbom.cdx.json`, included in `SHA256SUMS.txt`, and signed alongside the manifest.
- Cosign keyless OIDC signing in `release.yml` via `cosign 2.4.1`. The `id-token: write` permission lets the release job mint a short-lived OIDC token bound to the workflow run; cosign exchanges it with Fulcio for a signing certificate whose SAN records `https://github.com/Mathews-Tom/scrybe/.github/workflows/release.yml@<ref>`. `SHA256SUMS.txt` and the SBOM are signed with `cosign sign-blob`, emitting `<asset>.sig` (signature) and `<asset>.pem` (certificate) alongside each. Verifying the manifest's signature transitively covers every asset whose hash appears in the file. Recipe in `INSTALL.md` "Verify a release with cosign".
- `supply-chain/{config.toml, audits.toml}` — initial cargo-vet wiring per `.docs/development-plan.md` §13.1. `config.toml` declares the policy and imports six upstream audit feeds (Bytecode Alliance, Embark, Google, ISRG, Mozilla, Zcash); `audits.toml` is empty pending the maintainer's direct-dep audit pass. The `vet` job in `ci.yml` runs `cargo vet check --locked` in advisory mode (`continue-on-error: true`) so a missing audit does not block PRs while the audit work is incremental. Promotion to a blocking gate is a v1.0 deliverable.
- `packaging/` — in-tree templates for the five downstream package managers per `.docs/development-plan.md` §13.1: `homebrew/scrybe.rb`, `scoop/scrybe.json`, `aur/PKGBUILD`, `flatpak/dev.scrybe.scrybe.yaml`, `fdroid/dev.scrybe.scrybe.yml`. Each template uses `{{ ... }}` placeholders rendered against a published release tag's SHA256 manifest. Submission to each downstream registry is a maintainer-bound action (Homebrew tap, Scoop bucket, AUR account, Flathub PR, F-Droid PR) per the stop-condition policy; templates land in-tree so the rendering work is reproducible without ad-hoc copy-pasting at release time. `packaging/README.md` documents the render workflow.
- `INSTALL.md` "Verify a release with cosign" section — full `cosign verify-blob` recipe pinned to the release workflow's identity (`--certificate-identity-regexp` against the workflow path at the release tag, `--certificate-oidc-issuer` against `token.actions.githubusercontent.com`).
- `INSTALL.md` "Verify reproducibility" section — local-reproduction recipe matching the CI job's three inputs (`SOURCE_DATE_EPOCH=1714464000`, `RUSTFLAGS=--remap-path-prefix`, `rust-toolchain.toml` 1.95.0).
- `INSTALL.md` "Linux" and "Windows" sections — `cargo install --git ... --tag` audit-friendly install paths for hosts where the cargo-dist matrix does not yet emit native artifacts. Both sections name the v0.9.x packaging-template stream as the upcoming distribution surface.

### Changed

- All workspace crates bump from `0.5.0` to `0.9.0-rc1`. Path-dep version pins follow. `scrybe::tests::test_version_constant_matches_cargo_metadata` updated to lock against the `0.9.x` line.
- `release.yml` `build` matrix env block: comment refreshed to reference the new `reproducibility.yml` lane as the verification companion to the in-place `SOURCE_DATE_EPOCH` + `RUSTFLAGS` settings.
- `release.yml` cargo-dist install step: now installs `cargo-cyclonedx@0.5.7` and `cosign@2.4.1` alongside `cargo-dist@0.25.1` so the release job has SBOM and signing tooling available without separate `apt`/`brew install` dance.
- `INSTALL.md` "manual install (audit-friendly)" intro: replaced the "cosign-based provenance verification will land in a future release" deferral with a forward-pointer to the new "Verify a release with cosign" section.

### Deprecated / Removed

- Nothing.

### Security

- `cargo audit` and `cargo deny` policies are unchanged from v0.5.0 (same advisory ignores, same license clarifies).
- `cosign verify-blob` over `SHA256SUMS.txt` is the cryptographic anchor for distribution trust through v1.0. Native Apple Developer ID notarization and Windows code-signing certificates remain explicitly out of scope per `.docs/development-plan.md` §13.1; cosign keyless signing provides artifact-level CI provenance (proves "this asset came from the GHA workflow on this commit") without paying the $99/yr Apple tax or coupling the project to a vendor-signed certificate authority.
- The `vet` job is advisory at v0.9.0-rc1. v1.0 promotes it to blocking after the maintainer commits direct-dep audit entries to `supply-chain/audits.toml`.

### Known limitations

- **Reproducibility lane is advisory at v0.9.0-rc1.** Empirically the verification reports tarball-level divergences on `macos-14` even with `SOURCE_DATE_EPOCH`, `--remap-path-prefix`, `-Wl,-no_uuid`, and a pinned 1.95.0 toolchain in place. The remaining non-determinism sources (candidate suspects per the cargo-dist + Rust reproducibility surveys: rustc embedded build-host triple, cargo-dist metadata embedding, xz compressor non-determinism on macOS) need diffoscope-driven investigation. The lane uploads both legs' artifacts on every run so the comparison work is unblocked. Promotion to a blocking gate is a v0.9.x → v1.0 deliverable.
- **Reproducibility lane is macOS-only.** The cargo-dist matrix only targets Apple Silicon and Intel macOS through this release per `Cargo.toml` `[workspace.metadata.dist] targets`. Linux + Windows reproducibility lanes land alongside the `cargo deb` / `cargo wix` packaging work in the v0.9.x stream.
- **Direct-dep cargo-vet audits not yet committed.** The wiring is in place; the audit work is a v0.9.x follow-up. The expected first batch covers crates that the imported feeds do not vouch for — `objc2-core-audio*` (Apple bindings, no upstream feed reviews them) and `whisper-rs` (vendored Codeberg primary) are the leading candidates per `docs/dependency-decisions.md`.
- **Package-manager templates not yet submitted.** Submission to Homebrew tap / Scoop bucket / AUR / Flathub / F-Droid is a maintainer action per the stop-condition policy. Templates land in-tree at v0.9.0-rc1; the first round of submissions is a v0.9.x follow-up.
- **MSI (`cargo-wix`) and `.deb` (`cargo-deb`) artifacts not yet emitted.** The Scoop and AUR templates point at tarball paths that the cargo-dist matrix produces today on macOS only; Linux / Windows native artifacts land alongside the cargo-dist target expansion in the v0.9.x stream.

### Workspace

- 7 crates (unchanged from v0.5.0): `scrybe`, `scrybe-core`, `scrybe-capture-mac`, `scrybe-capture-linux`, `scrybe-capture-win`, `scrybe-android`, `scrybe-cli`.
- Publish posture unchanged: only `scrybe` publishes to crates.io.

### Contributors

- Maintainer: Mathews Tom.

[0.9.0-rc1]: https://github.com/Mathews-Tom/scrybe/releases/tag/v0.9.0-rc1

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
