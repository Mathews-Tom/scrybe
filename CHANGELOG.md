# Changelog

All notable changes to scrybe are documented here. The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html) within the stability tiers documented in `docs/system-design.md` §12.

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
- `scrybe-core::storage::transcript_log::TranscriptPartialLog` — the `transcript.partial.jsonl` write-ahead log from `system-design.md` §8.3. Open() recovers the next monotonic `seq` by scanning the existing file, `append_pending(chunk)` writes a `flushed_to_transcript = false` line, `mark_flushed(seq, chunk)` writes the matching `flushed_to_transcript = true` line, and `scan_recovery(folder)` collapses the WAL by taking the highest-flushed record per seq. Pending records that never received a flushed follow-up surface as orphans; flushed records become a `flushed_seqs` cursor. Malformed lines are counted in `malformed_line_count` but do not abort recovery (commits `99b3748`, `e323f12`).
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
- **Cloud STT not yet hardware-validated against a real provider.** Tests cover the §8.3.1 unit matrix (200, 401, 429-then-200, 503 retry-exhaust, malformed JSON, missing-Authorization path) via wiremock. Real-provider validation against Groq + OpenAI is the I-10 integration test deferred from v0.2 to v0.2.x.
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
