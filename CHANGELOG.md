# Changelog

All notable changes to scrybe are documented here. The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html) within the stability tiers documented in `docs/system-design.md` §12.

## [0.1.0] — pending

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
