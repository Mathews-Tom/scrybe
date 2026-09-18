#!/usr/bin/env python3
# Copyright 2026 Mathews Tom
# Licensed under the Apache License, Version 2.0 (the "License");
# you may not use this file except in compliance with the License.
# You may obtain a copy of the License at
#     https://www.apache.org/licenses/LICENSE-2.0
"""Per-crate Rust LoC budget gate using tokei.

Counts code-only lines (excludes blank lines and comments) under each
member crate's `src/` tree and asserts the total stays under the
ceiling specified in `LOC_CEILINGS` below. Tests inline in `#[cfg(test)]`
modules are counted because tokei parses files as a whole; the ceilings
are sized accordingly.

Run locally:

    python3 scripts/check-loc-budget.py

Run in CI: see `.github/workflows/ci.yml` job `loc-budget`.

Ceilings track `.docs/DEVELOPMENT_PLAN.md` §7 and are updated when
the plan revises them. Increasing a ceiling is a deliberate decision:
state the rationale in the commit message and surface it for review.
"""

from __future__ import annotations

import json
import subprocess
import sys
from pathlib import Path

# Code-only LoC ceilings per member crate. Counts include `#[cfg(test)]`
# modules that live inline in the same file as production code; if the
# test footprint dominates, prefer extracting tests to `tests/` (which
# tokei excludes here because we point it at `src/` only).
LOC_CEILINGS: dict[str, int] = {
    # 8500 was the v1.0.0 ceiling (after pyannote-onnx). Raised to
    # 9000 at v1.0.2 to absorb the OggOpusEncoder (~340 LoC including
    # tests + the new derive_model_label helper for the whisper-name
    # fix). The encoder closes the v0.1 carryover where audio.opus
    # was raw PCM under an `.opus` filename; further growth in
    # scrybe-core should still trip this gate.
    # Raised to 9050 for v1.0.5 to absorb generated-title orchestration
    # and the `[record]` config block that lets bare `scrybe record`
    # resolve capture/STT/LLM defaults from config.
    # Raised to 9550 for v1.1.0 to absorb the `StereoInterleaver` module
    # (`pipeline/interleave.rs` ~300 LoC including tests), the `[audio]`
    # meta block, and the channel-split regression tests in
    # `session.rs`. The interleaver closes the v1.0.x duration-drift
    # bug where `--source mic+system` produced an `audio.opus` whose
    # length was the sum of mic and system sample counts rather than
    # the wall-clock session duration; further growth in scrybe-core
    # should still trip this gate.
    # Raised to 9750 for v1.0.5 to absorb the `record_defaults` module
    # (~161 LoC including tests). The module provides ergonomic-default
    # resolvers consumed by the `scrybe record <title>` subcommand:
    # platform-aware source defaults (mic+system on macOS, mic
    # elsewhere), Whisper model path resolution against the platform
    # project-data dir, and a 100 ms TCP probe of `127.0.0.1:11434` to
    # auto-select `openai-compat` when a local Ollama is reachable. The
    # schema defaults in `RecordConfig` remain `synthetic` / `stub` so
    # existing `scrybe rec` invocations and bare config behavior are
    # unchanged; the resolvers only run when the new ergonomic command
    # is invoked.
    # Raised to 10100 for v1.1.0 to absorb the `pipeline::journal`
    # module (`journal.rs` ~360 LoC including tests) and its wiring
    # into `session.rs`'s `SessionJournals`. Every session now writes
    # per-source raw f32 PCM to rotating `journal/<source>-<seq>.f32`
    # segments on a dedicated OS thread, independent of the live
    # encode path, so a crash mid-session loses at most the current
    # segment rather than the whole recording (closes
    # `.docs/development-plan.md` §19.2 defect D1's durability half;
    # the anchor manifest and offline merge that replace the live
    # encode path land in later stacks of the same release).
    # Raised to 10350 for v1.1.0 to absorb the journal anchor contract:
    # `JournalAnchor`/`JournalManifest` plus `write_manifest`/
    # `read_manifest` in `pipeline::journal` (with round-trip tests),
    # the `AudioFrame::timestamp_ns` doc-comment correction (source-
    # relative, undefined origin across sources — defect D2), and
    # `AudioMeta`'s new `mic_epoch_ms`/`system_epoch_ms` fields plus
    # the session-level tests proving they and `journal/manifest.toml`
    # land correctly end-to-end.
    # Raised to 10850 for v1.1.0 to absorb `pipeline::merge`
    # (`merge.rs` ~450 LoC including tests): the offline merge that
    # turns per-source journal segments into `audio.opus` after
    # capture ends -- downmix, resample to the encoder rate,
    # epoch-delta silence-prefix the later-starting source,
    # interleave, encode once, and assert the result is within 1% of
    # wall clock (`PipelineError::DurationMismatch`) before deleting
    # the journal. Not yet wired into `session::drive_session`'s live
    # path -- that cutover, plus `StereoInterleaver` removal and
    # `scrybe repair`, lands in the next stack of this release.
    # Raised to 10900 for M2/M3's macOS system-audio selection: the
    # configuration contract names the chosen backend explicitly so absent
    # settings can resolve to ScreenCaptureKit without changing legacy Tap
    # invocations. The core ceiling keeps a 37-line margin after that contract;
    # capture implementation remains isolated in its adapter crate.
    # Raised to 11000 for M4's terminal capture-error finalization: sessions
    # now flush the journal, merge audio, and write artifacts before exposing
    # the original capture failure to callers. Raised to 11050 for the M5 STT
    # boundary validation and its consumer-observable regression tests.
    # Raised to 11200 for M5's opt-in `SherpaStreamingProvider`: the pinned
    # artifact validator, OnlineRecognizer construction, and isolated
    # blocking decode retain Whisper selection while keeping the native runtime
    # out of the default build. Raised to 12500 for M5's partial
    # transcripts/token timings: persistent per-source streaming state,
    # stateful normalization, token conversion, and recovery-safe WAL records.
    # Raised to 13000 for M5's paired STT benchmark: strict checked corpus
    # parsing, PCM16 WAV validation, complete result-matrix aggregation, and
    # versioned WER/provider-lifecycle reporting. Raised to 14250 for M9's
    # capability-limited session reader, protocol, and behavioral proof that
    # every served operation leaves the fixture tree unchanged. Raised to
    # 14750 for M6 PR-1's accepted-final boundary: anchor-relative source
    # progress, bounded final-chunk ordering, deferred WAL flushes, and tests
    # that prove durable transcript and hook payloads remain identical. Raised
    # to 15450 for M6 PR-2's exact token dedup and crash-safe suppressed WAL
    # disposition, including boundary, recovery, and dual-source stream tests.
    # Raised to 15800 for M7's canonical transcript parser, explicit `[notes]`
    # configuration, and whole-segment packing with bounded overlap. Raised to
    # 16300 for M7's preflighted local tokenizer, capped map dispatch,
    # deterministic processing gaps, reduction orchestration, and title source
    # cutover. Reduction/title cap compaction and templates remain separate.
    # Raised to 16400 for the M10 source-clock duration guard: journal manifests
    # now retain per-source capture timestamps so model startup and shutdown do
    # not invalidate a real capture, while timestamp gaps still fail loudly.
    # Raised to 16850 after the first real meeting exposed finalization and
    # playback defects. The increase covers one persistent Whisper context,
    # centered playback encoding, synchronous progress events, provisional and
    # reconstructed metadata, bounded notes output, and recovery regressions.
    "scrybe-core": 16850,
    # 2000 was the v0.5 ceiling. Raised to 2300 at v0.6 to absorb the
    # `scrybe bench` subcommand. Raised to 2500 at v1.0.1 to absorb
    # the `--source mic` and `--whisper-model` wiring on `scrybe record`
    # (the v0.1 mic-only path that shipped under stub providers
    # through v1.0; see CHANGELOG `[1.0.1]`). New code lands as
    # ~140 LoC including the three new tests; further growth in
    # scrybe-cli should still trip this gate.
    # Raised to 2650 for v1.0.5 to absorb `scrybe init --profile
    # mac-local` and config-backed `scrybe record` defaults.
    # Raised to 2800 for v1.1.0 to absorb `scrybe doctor --check-tap`,
    # the end-to-end Core Audio Tap diagnostic that distinguishes the
    # three failure shapes for the system-tap-silent-frames bug. The
    # probe is gated behind `system-capture-mac` so non-feature builds
    # surface a "skipped" message rather than carrying the dead path.
    # Raised to 3300 for v1.0.5 to absorb the new `record` ergonomic
    # subcommand (~280 LoC including 8 unit tests at
    # `commands/record.rs`) and the `bundle_launcher` module (~210 LoC
    # including 3 unit tests). The ergonomic command resolves capture
    # source, Whisper model, and LLM kind from config plus platform
    # probes, then on macOS auto-launches via the .app bundle so
    # TCC's AudioCapture grant binds to the bundle's responsible
    # process — direct invocation of the inner binary silently
    # zero-fills the system tap (see `.docs/handoff.md` §1, §7).
    # Existing `scrybe rec` semantics are unchanged; the new command
    # is additive.
    # Raised to 3400 for v1.0.5 to absorb the bundle_launcher polish
    # found during hardware testing: stderr suppression on the
    # post-shutdown `kill -0` poll (was leaking "kill: PID: No such
    # process" to the user's terminal twice per session), graceful
    # already-exited handling on `send_sigint`, and a final session-
    # summary block (`scrybe record: session ULID written to ...`
    # with transcript/notes/meta/audio paths) reconstructed from the
    # session folder since the bundle's own stdout summary is
    # detached by Launch Services. Adds ~50 LoC including 2 unit
    # tests for the meta.toml session_id parser.
    # Raised to 3650 for v1.1.0 to absorb the `scrybe repair
    # <id-or-folder>` subcommand (`commands/repair.rs` ~150 LoC
    # including 3 tests): recovers `audio.opus` from a session's
    # `journal/` after a crash or `SIGKILL` left it without a
    # completed offline merge, using the same `pipeline::merge_journal`
    # a live session runs, then reconstructs `meta.toml` when one
    # was never durably written. Also absorbs `commands::list`'s
    # `UNFINISHED` row detection (journal present, no `audio.opus`)
    # that points users at the new subcommand, and a net LoC increase
    # from extracting the `resolve_folder` duplicated between
    # `commands::show` and the new `commands::repair` into
    # `runtime::resolve_session_folder` (fewer total lines than two
    # private copies, but the shared helper's doc comment and tests
    # now live in `runtime.rs` instead of a `show.rs`-local fn).
    # Raised to 4000 for the M2/M3 macOS system-audio control plane: backend
    # selection, independent diagnostics, and bounded Tap-to-ScreenCaptureKit
    # recovery live in CLI command handlers. This covers the committed M3 stack
    # without weakening the adapter-specific ceilings above.
    # Raised to 4100 for M4's user-facing macOS input-device catalog and its
    # deterministic, tab-separated rendering contract. Device enumeration
    # remains in `scrybe-capture-mac`; the CLI owns the command and presentation.
    # Raised to 4200 for M4's UID-pinned microphone path, including the
    # feature-gated fallback and regression coverage for explicit selectors.
    # Raised to 4300 for M4's capture registry, signal teardown, and bounded
    # liveness watchdog. The CLI owns orchestration; adapters remain separate.
    # Raised to 4450 for M5's explicit `--sherpa-model` selector and matching
    # ergonomic forwarding. Both paths reject a missing feature instead of
    # silently substituting the stub provider. Raised to 4750 for M5's explicit
    # `bench stt` mode, its fail-loud provision checks, and the unchanged
    # Criterion-harvest dispatch. Raised to 4850 for M9's default-off stdio
    # command, config refusal, and recorded five-tool integration transcript.
    # Raised to 5200 for M10's redacted qualification verifier: receipt schema,
    # stage dependency, session-artifact evidence, named failures, CLI wiring,
    # and adversarial no-content serialization tests. Raised to 5350 for
    # M10's installed CLI-owned self-signed bundle creator, Keychain identity
    # diagnostic, and deterministic signature verification.
    # Raised to 5550 after the first real meeting exposed the need for exact
    # input-device pinning, live transcript/finalization output, resumable
    # notes generation, stale-lock cleanup, and bounded bundle shutdown.
    # Raised to 5750 for the shared macOS bundle lifecycle: single-source
    # discovery and identity policy, four-state validation, temporary sibling
    # construction, signature verification, and rollback-safe replacement.
    # Raised to 6200 for backend-aware doctor onboarding: TTY-gated SCK and
    # Tap flows, explicit repair precedence, bounded Launch Services handoff,
    # private diagnostic output capture, and corrected Tap-only CLI guidance.
    # Raised to 7300 for the native recording shell: strict indicator config,
    # one stop/finalization coordinator, deterministic status-bar rendering,
    # a non-activating AppKit panel, ergonomic dispatch, and focused regressions.
    "scrybe-cli": 7300,
    # Raised to 2700 for M6's opt-in transactional VoiceProcessingIO path:
    # explicit device/format/processing configuration, ducking suppression,
    # RAII cleanup, and a plain-input fallback proved against forced failures.
    # Shared application-service layer consumed by the CLI and the
    # read-only agent surface. Sized for typed serializable contracts,
    # opaque confined identity, and errors; the paged cancellable session
    # repository with classification, root confinement, and coalesced
    # invalidation; the structure-preserving atomic configuration editor
    # with rollback; structured diagnostic findings with a separate repair
    # boundary; the versioned recording state model with idempotent stop;
    # and the inline `#[cfg(test)]` coverage this script counts. Grounded
    # in the 1,171 measured lines of `scrybe-core/src/agent_access` and the
    # 1,758 measured lines of the migrating `scrybe-cli` list/show/runtime/
    # doctor/shell surfaces, which this layer generalizes rather than
    # copies. The `scrybe-core` and `scrybe-cli` ceilings do not move: the
    # migration deletes the duplicated scans it replaces.
    #
    # Raised to 7000 for the managed model catalog and the guided-setup
    # contracts the same stack builds on it. The catalog and manager
    # measure 830 lines — the checked-in manifest's parser and its two
    # validators, the serializable acquisition contracts, the
    # transport-independent state machine with its free-space
    # preflight, cancellation, exact size and digest verification and
    # atomic promotion, the HTTP transport behind `model-download`, and
    # the per-platform free-space probe — taking the measured figure
    # from 5,189 to 6,097. The remaining 903 lines of headroom are
    # sized for the rest of the stack in this layer rather than chosen
    # round: a list-typed configuration value and the three new
    # GUI-owned fields, the typed readiness snapshot, the model and
    # local-notes-provider findings under the already-reserved
    # `Providers` component, and the loopback reachability probe that
    # does not exist today. Every behavioural test for all of it lives
    # in `tests/`, which this script excludes, as the note above
    # recommends; only the catalog's own whole-file duplicate rule and
    # the free-space probe's two cases stayed inline, because neither
    # is reachable through the public surface.
    #
    # Raised to 7700 for the recording orchestration every surface now
    # resolves and checks through. It is a build rather than an
    # adaptation: before it, the invocation policy lived binary-private
    # in `scrybe-cli`'s `commands/rec.rs`, six of the seven preflight
    # checks were written inline in its `run_with_stop`, and
    # `ErrorCode::PreflightFailed` was a variant nothing produced.
    # Itemized from what the measured figure did, not rounded:
    #
    #   6967  before any of it
    #   +212  `recording/plan.rs`: the four kinds a recording is
    #         described by, the override record a surface fills in, and
    #         the resolution itself — capture source, system-audio
    #         adapter, input device, transcription model, notes
    #         backend, consent mode, and the storage root with its `~`
    #         expansion, which the command-line tool and the desktop
    #         host had each been doing for themselves
    #   +357  `recording/preflight.rs`: seven checks, each with its own
    #         outcome, the report and the refusal it converts to, and
    #         the build-capability record a frontend passes in —
    #         passed in rather than read from a `cfg!` here, because
    #         the capture adapters live in crates this one does not
    #         depend on
    #   + 84  `recording/orchestration.rs`: the one path from "a
    #         surface asked to record" to "capture may begin" — the
    #         already-running refusal that runs no preflight, the
    #         check, the visible settle through `Preparing` and back to
    #         idle, and the boxed refusal carrying the report and the
    #         plan a surface renders them against
    #   + 13  the module's re-exports
    #   =7633 measured
    #
    # The ceiling is 7700. The last 67 lines are margin for maintenance
    # inside this layer, not a reservation for anything named. The
    # capture construction and the `scrybe_core::session` call are
    # deliberately not in that sum and are not reserved for here: they
    # name per-platform capture adapters this crate does not depend on,
    # and the work that moves them should argue its own ceiling.
    #
    # Every behavioural test for all of it is in
    # `tests/recording_preflight.rs`, which this script excludes, as
    # the note at the top recommends.
    #
    # Raised again, to 8350, for the rest of one recording: the
    # providers a plan names, the pipeline call, the accepted-stop
    # signal, and the adapter registry. All four were binary-private in
    # `scrybe-cli`, so a desktop host could resolve and check a
    # recording and then had nothing to run one with. Continuing the
    # sum:
    #
    #   7633  measured, after resolution and preflight
    #   +388  `recording/run.rs`: the two dispatch enums over the
    #         providers a plan can name and their construction, the
    #         stub transcription and stub notes that used to live in
    #         `commands/rec.rs`, the settled-consent prompter a window
    #         answers through, the deterministic synthetic source, and
    #         the `SessionInputs` assembly and `run_with_notes` call
    #         every frontend now shares
    #   +100  `recording/progress.rs`: the four ordered saving steps, a
    #         surface renders from, and the one observer that drives the
    #         controller from the pipeline's own boundaries. Its whole
    #         reason for existing is the projection that refuses to
    #         derive a step from the pipeline event carrying transcript
    #         text, so no indicator, event, or log can come to hold what
    #         a meeting said
    #   + 81  `recording/capture.rs`: the adapter registry, moved
    #         verbatim from `scrybe-cli`, and the microphone the
    #         `mic-capture` feature buys
    #   + 44  `recording/stop.rs`: the one route from an accepted stop
    #         to the capture holding a recording open. It adds no
    #         coordinator — the controller's mutex is still what decides
    #         — and exists so six surfaces do not each write the
    #         check-then-signal ordering themselves
    #   + 25  the new error codes, and the module's re-exports
    #   =8271 measured
    #
    # The ceiling is 8350, leaving 79 lines of margin. `scrybe-cli` went
    # the other way over the same change, from 7074 to 6738 against an
    # unchanged 7300, because what moved here was deleted there.
    "scrybe-application": 8350,
    "scrybe-capture-mac": 2700,
    "scrybe-capture-linux": 2500,
    "scrybe-capture-win": 2500,
    # Cross-platform mic adapter via cpal. Closes the v0.1 mic-only path
    # documented in `.docs/development-plan.md` §7.2 that shipped under
    # synthetic providers through v1.0. Ceiling sized for the cpal stream
    # owner + per-format conversion + tests; raise if a per-platform
    # device-listing surface is added.
    "scrybe-capture-mic": 1500,
    "scrybe-android": 2500,
    # The native macOS indicators a running recording is shown on. A
    # crate of its own for two reasons that are not organisational: the
    # modules were private to a `[[bin]]` that declares no `[lib]`, so
    # nothing could reach them; and `scrybe-desktop/src-tauri` sets
    # `unsafe_code = "forbid"`, which a local `#![allow]` cannot
    # override, so the floating panel's `msg_send!` blocks can never be
    # inlined into the desktop host. Unpublished, like the parked
    # platform crates.
    #
    #   218  `floating_panel.rs`, moved verbatim from `scrybe-cli`
    #   123  `hotkey.rs`, moved verbatim
    #   133  `status.rs`: the waveform geometry, the eight-frame
    #        sequence, and the colour blend, taken out of `tray.rs` —
    #        which keeps its `tray-icon` object, because the desktop
    #        host has a Tauri-native one of its own
    #    26  `view.rs`: the rendering-independent snapshot every
    #        indicator takes, which had to leave `shell.rs` for the
    #        panel to compile anywhere else
    #     6  the module declarations
    #   =506 measured
    #
    # The ceiling is 600. The 94 lines of headroom are for the Tauri
    # side of the same widgets, not a reservation for a second surface:
    # `scrybe-cli` fell 6685 -> 6235 over the same change.
    "scrybe-widgets": 600,
    # The desktop host: the Tauri process that owns windowing, the tray,
    # single-instance activation, and the translation between the
    # WebView's IPC and the shared application services. It lives in its
    # own Cargo workspace, so no root-workspace job can see it; this
    # entry is the only thing that counts it, and a member absent from
    # this table is silently uncounted rather than failing closed.
    # Sized for the transport contracts and their generated-binding
    # export, the narrow per-service commands, the process-lifetime
    # application state, the window/tray/menu/single-instance/idle
    # lifecycle, the navigation guard, the debug-only lifecycle channel
    # and control socket, and the inline `#[cfg(test)]` coverage this
    # script counts. Grounded in the 2,292 measured lines of
    # `scrybe-application/src/sessions` and `src/config`, which the host
    # mirrors as data-transfer types and forwarding commands rather than
    # reimplements; it carries no domain policy of its own, so it is
    # budgeted well under the service layer it fronts.
    #
    # The measured implementation is 1,062, leaving 138 lines of
    # headroom. That is sized against the next thing to land here rather
    # than chosen round: recording control adds two commands to
    # `commands.rs` and their contract types, and turns the tray's
    # `Record now` from a disabled item into a live one with a
    # confirmation on the quit path — the same shape as the four
    # commands and one tray item already measured here, which together
    # account for roughly that many lines.
    #
    # Raised to 2,300 for guided setup, settings, and the qualification
    # way-in. The measured figure is 2,115.
    #
    # An earlier raise to 1,700 priced seven commands, from a 1,111-line
    # measurement, at roughly ninety lines per command-and-projection
    # pair. Both inputs were wrong, and restating them is the point of
    # this paragraph: the next person proposing a raise reads the
    # comment, not the commit log.
    #
    # Ten commands landed, not seven — the settings form and its update,
    # the diagnostic report and its explicit repair, the readiness
    # snapshot, the model offer, the confirmed install and its
    # cancellation, and the two platform-surface opens — over the four
    # already here, each forwarding to a service method and each with a
    # capability entry. And the per-pair figure was low because
    # `contract/setup.rs` is not a projection of the shape the earlier
    # ones were: it carries a two-way exhaustive field mirror between
    # the service layer's settings enumeration and the wire form, so
    # that a field added to one and not the other fails to compile
    # rather than silently disappearing. That module alone measures 478
    # of the 906 lines under `contract/`.
    #
    # Raised from 2300 to 3000 for the session library. Derived, not
    # rounded, from what the measured figure did and what the rest of
    # this layer still owes:
    #
    #   2119  before any of it
    #   + 91  the request-keyed cancellation registry and the cancel
    #         command a search is abandoned through
    #   +401  the detail surface: six session commands, the reveal and
    #         the two clipboard paths, and `contract/detail.rs`, which
    #         projects eleven types rather than the three `session.rs`
    #         carried
    #   =2611 measured
    #   +370  the audio protocol: the scheme handler, the byte-range
    #         parsing written by hand because Tauri supplies none, the
    #         typed refusals for a missing or unfinished artifact, and
    #         the debug-only probe that drives the real webview at a
    #         real URL on the scheme — the only thing that can tell a
    #         correctly-admitted scheme from one admitted under the
    #         wrong policy directive. The confinement and range tests
    #         live in `tests/`, which this gate excludes.
    #   =2981 measured
    #
    # The ceiling is 3000. The last 19 lines are margin for maintenance
    # inside this layer, not a reservation for anything named: no
    # further work in this stack adds Rust here.
    #
    # Recording control, which this table used to reserve 140 lines
    # for, is not in that sum: it has not landed and its ceiling should
    # be argued by the work that lands it, not carried here.
    #
    # `lifecycle/model_probe.rs` is 140 of the measured figure and is
    # `#[cfg(debug_assertions)]` in full. It is counted anyway: this
    # gate measures source a maintainer reads, not bytes a user ships.
    #
    # The frontend is deliberately out of scope: this script measures
    # Rust only, and the TypeScript surface is governed by review rather
    # than by this gate.
    #
    # Raised to 3100 for the recording preflight this host now reports.
    # Continuing the sum above:
    #
    #   2981  measured, before any of it
    #   + 96  the preflight surface: `contract/recording.rs`'s four new
    #         projected types and their two exhaustive mirrors of the
    #         service enums, the `recording_preflight` command, and the
    #         record of what this binary actually linked — which is
    #         synthetic capture and no transcription runtime, because
    #         this crate depends on no capture adapter, and saying
    #         otherwise would let a preflight clear a recording this
    #         host cannot perform. Net of the four lines `expand_home`
    #         lost when it became a call into the shared expansion the
    #         orchestration owns.
    #   =3051 measured
    #
    # The ceiling is 3100. The last 49 lines are margin for maintenance
    # inside this layer.
    #
    # Raised to 3350 for the main window's recording control, which is
    # what the note above meant by work that should argue its own
    # ceiling. Continuing the sum:
    #
    #   3051  measured, after the preflight surface
    #   +181  `recording.rs`: the start and stop commands, the handle
    #         that binds a window's control to the recording in flight,
    #         the capture this binary can open, and the settling of a
    #         completed or failed session back to idle. It owns no
    #         policy — resolution, the checks, the providers, the
    #         pipeline, and the state model are all in
    #         `scrybe-application` — and what is here is the
    #         translation between the WebView's IPC and them
    #   + 50  the progress transport: the four ordered steps projected
    #         through an exhaustive mirror of the service enum, the
    #         payload a window renders them from, and the event name
    #         they are published on
    #   =3282 measured
    #
    # The ceiling is 3350, leaving 68 lines of margin.
    #
    # Raised to 3500 for four of the six surfaces a recording is stopped
    # from, and the quit path they made possible. Continuing the sum:
    #
    #   3282  measured, after the window's recording control
    #   + 31  `lifecycle/hotkey.rs`: registering the accelerator on the
    #         thread the platform pins its handler to, and draining its
    #         presses from a thread that is not that one
    #   + 32  `lifecycle/signals.rs`: the first terminating signal read
    #         as a quit request, and the second deliberately left to the
    #         platform
    #   + 37  the tray's two recording items, the state they are enabled
    #         and disabled from, and the dispatch behind each verb
    #   + 34  the deferred quit: the decision, the flag one is pending,
    #         and the exit taken when the recording becomes durable —
    #         replacing the refusal that used to strand a reader with a
    #         window they could not close
    #   =3416 measured
    #
    # The ceiling is 3500, leaving 84 lines of margin. The floating pill
    # is not in that sum and is not reserved here: it is the one native
    # surface this work does not wire, and the work that wires it should
    # argue for it.
    "scrybe-desktop/src-tauri": 3500,
}


def measure(crate_src: Path) -> int:
    """Return code-only Rust LoC under `crate_src`.

    Raises `RuntimeError` if tokei is missing or returns malformed JSON,
    rather than masking the failure as zero LoC. The CI gate must fail
    loudly when the measurement tool is broken.
    """
    if not crate_src.is_dir():
        raise RuntimeError(f"crate src path does not exist: {crate_src}")
    result = subprocess.run(
        ["tokei", str(crate_src), "--types", "Rust", "--output", "json"],
        capture_output=True,
        text=True,
        check=True,
    )
    payload = json.loads(result.stdout)
    rust = payload.get("Rust")
    if rust is None:
        raise RuntimeError(f"tokei returned no Rust entry for {crate_src}")
    code = rust.get("code")
    if not isinstance(code, int):
        raise RuntimeError(f"tokei returned non-integer code count for {crate_src}: {code!r}")
    return code


def main() -> int:
    repo_root = Path(__file__).resolve().parent.parent
    overshoots: list[tuple[str, int, int]] = []
    print(f"{'crate':<25} {'code LoC':>9}  {'ceiling':>8}  status")
    print(f"{'-' * 25} {'-' * 9}  {'-' * 8}  {'-' * 6}")
    for crate, ceiling in sorted(LOC_CEILINGS.items()):
        loc = measure(repo_root / crate / "src")
        status = "ok" if loc <= ceiling else "OVER"
        print(f"{crate:<25} {loc:>9}  {ceiling:>8}  {status}")
        if loc > ceiling:
            overshoots.append((crate, loc, ceiling))
    if overshoots:
        print()
        print("LoC budget exceeded:")
        for crate, loc, ceiling in overshoots:
            print(f"  {crate}: {loc} > {ceiling} ({loc - ceiling} over)")
        print()
        print("Either reduce LoC, split modules, or revise the ceiling in")
        print("`scripts/check-loc-budget.py` AND `.docs/development-plan.md` §7.4.")
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
