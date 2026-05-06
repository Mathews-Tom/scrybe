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
    # Raised to 9800 for v1.1.x to absorb the timestamp-aware
    # interleaver rewrite. The new code adds head-cursor tracking,
    # exact-fractional sample/ns conversion helpers, gap-bridge and
    # overlap-skip logic on push, and per-side head alignment in
    # drain — together with seven new regression tests that exercise
    # initial-offset alignment in both directions, intra-source gap
    # zero-fill, overlapping-frame prefix discard, steady-state head
    # advancement, and zero-fill anchoring. These resolve the
    # `(fast forwarding)` STT corruption surfaced in PR #47 testing.
    "scrybe-core": 9800,
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
    "scrybe-cli": 2800,
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
