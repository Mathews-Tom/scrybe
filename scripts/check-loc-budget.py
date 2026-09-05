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

Ceilings track `.docs/development-plan.md` §7.4 and are updated when
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
    "scrybe": 100,
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
    # the original capture failure to callers.
    "scrybe-core": 11000,
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
    "scrybe-cli": 4300,
    "scrybe-capture-mac": 2500,
    "scrybe-capture-linux": 2500,
    "scrybe-capture-win": 2500,
    # Cross-platform mic adapter via cpal. Closes the v0.1 mic-only path
    # documented in `.docs/development-plan.md` §7.2 that shipped under
    # synthetic providers through v1.0. Ceiling sized for the cpal stream
    # owner + per-format conversion + tests; raise if a per-platform
    # device-listing surface is added.
    "scrybe-capture-mic": 1500,
    "scrybe-android": 2500,
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
    print(f"{'crate':<22} {'code LoC':>9}  {'ceiling':>8}  status")
    print(f"{'-' * 22} {'-' * 9}  {'-' * 8}  {'-' * 6}")
    for crate, ceiling in sorted(LOC_CEILINGS.items()):
        loc = measure(repo_root / crate / "src")
        status = "ok" if loc <= ceiling else "OVER"
        print(f"{crate:<22} {loc:>9}  {ceiling:>8}  {status}")
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
