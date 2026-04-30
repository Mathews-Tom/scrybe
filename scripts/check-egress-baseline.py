#!/usr/bin/env python3
# Copyright 2026 Mathews Tom
# Licensed under the Apache License, Version 2.0 (the "License");
# you may not use this file except in compliance with the License.
# You may obtain a copy of the License at
#     https://www.apache.org/licenses/LICENSE-2.0
"""Egress-baseline audit for `scrybe-cli` default-feature builds.

Walks `scrybe-cli`'s transitive dependency graph with default features
only — the air-gappable shape advertised in `docs/system-overview.md`
and `.docs/development-plan.md` §7.4 — and asserts that no resolved
package matches a denylist of HTTP, TLS, DNS, or transport crates.

Why a static dependency-graph check rather than running `lsof` against
a built binary:

- `lsof` has no portable equivalent on Windows; `netstat -ano` and
  `Get-NetTCPConnection` answer different questions.
- A binary that *can* open sockets does not necessarily *will* open
  sockets in any single test run; absence of egress at runtime does
  not prove absence of capability. The dependency graph proves the
  capability is not even compiled in.
- Adding a network crate to the default-feature build is the exact
  regression class this gate is meant to catch (e.g. someone moves a
  cloud-STT provider from a feature-gated module into the default
  build by accident).

Tokio's `net`, `process`, and `signal-unix` features are similarly
inspected via `cargo tree --format` so accidental enabling of
`tokio/net` is caught even though tokio itself is allowlisted.

Run locally:

    python3 scripts/check-egress-baseline.py

Run in CI: see `.github/workflows/ci.yml` job `egress-audit`.

Exit status 0 means the default-feature graph is clean. Exit status 1
means at least one denylisted crate appeared; the offending crates and
the path that pulled each one in are printed.
"""

from __future__ import annotations

import re
import subprocess
import sys
from pathlib import Path

# Crates whose presence in the `scrybe-cli` default-feature graph would
# imply the binary can speak HTTP, TLS, or DNS to a remote host.
# Cloud STT/LLM providers are gated behind `--features openai-compat`
# and `--features whisper-local` respectively (the latter is local
# inference and does not pull network deps); both must remain absent
# from the default-feature build.
NETWORK_DENYLIST: frozenset[str] = frozenset(
    {
        # HTTP clients
        "reqwest",
        "hyper",
        "hyper-util",
        "h2",
        "h3",
        "ureq",
        "attohttpc",
        "isahc",
        "surf",
        # gRPC and websocket transports
        "tonic",
        "tonic-build",
        "tokio-tungstenite",
        "tungstenite",
        # TLS and crypto
        "rustls",
        "rustls-pemfile",
        "rustls-webpki",
        "tokio-rustls",
        "tokio-native-tls",
        "native-tls",
        "openssl",
        "openssl-sys",
        "boring",
        "boring-sys",
        # Async DNS
        "trust-dns-resolver",
        "trust-dns-proto",
        "hickory-resolver",
        "hickory-proto",
        # QUIC
        "quinn",
        "quinn-proto",
        "quinn-udp",
    }
)

# Tokio is allowlisted, but the `net` and `process` features pull in
# socket and child-process syscalls that are out of scope for v0.1
# default builds. `cargo tree --format "{p} {f}"` reports active
# features per package.
TOKIO_FORBIDDEN_FEATURES: frozenset[str] = frozenset({"net", "process", "tokio-net"})


PACKAGE_LINE = re.compile(r"^([A-Za-z0-9_\-]+)\s+v([0-9][^\s]*)")


def run_cargo_tree(manifest: Path) -> str:
    """Run `cargo tree` with default features only and return stdout.

    Raises `RuntimeError` rather than masking any cargo failure as an
    empty graph — silent failure of the audit is exactly the regression
    class we are trying to prevent.
    """
    result = subprocess.run(
        [
            "cargo",
            "tree",
            "--manifest-path",
            str(manifest),
            "--no-default-features",
            "--edges",
            "normal",
            "--prefix",
            "none",
        ],
        capture_output=True,
        text=True,
        check=False,
    )
    if result.returncode != 0:
        raise RuntimeError(
            f"cargo tree failed (rc={result.returncode}):\n{result.stderr}"
        )
    return result.stdout


def collect_packages(tree_output: str) -> set[str]:
    """Extract the set of unique package names from a `cargo tree` dump."""
    packages: set[str] = set()
    for line in tree_output.splitlines():
        stripped = line.strip()
        if not stripped:
            continue
        match = PACKAGE_LINE.match(stripped)
        if match is not None:
            packages.add(match.group(1))
    return packages


def run_cargo_tree_with_features(manifest: Path) -> str:
    """Variant of `run_cargo_tree` that emits per-package active features.

    `--format "{p} {f}"` prints "<name> v<ver> <feature1>,<feature2>..."
    so we can audit feature flags on allowlisted crates like tokio.
    """
    result = subprocess.run(
        [
            "cargo",
            "tree",
            "--manifest-path",
            str(manifest),
            "--no-default-features",
            "--edges",
            "normal",
            "--prefix",
            "none",
            "--format",
            "{p} {f}",
        ],
        capture_output=True,
        text=True,
        check=False,
    )
    if result.returncode != 0:
        raise RuntimeError(
            f"cargo tree --format failed (rc={result.returncode}):\n{result.stderr}"
        )
    return result.stdout


def tokio_active_features(feature_output: str) -> set[str]:
    """Return the union of features active on every `tokio` instance.

    Raises `RuntimeError` if no `tokio` line was parsed at all. The
    workspace pins tokio in `Cargo.toml`, so its absence from the dep
    graph means either the manifest changed (deliberate, audit out of
    date) or `cargo tree --format` shifted its output schema (the
    silent-failure case this guard prevents). Either way, fail loud
    rather than vacuously pass the forbidden-feature check against an
    empty set.
    """
    saw_tokio_line = False
    active: set[str] = set()
    for line in feature_output.splitlines():
        stripped = line.strip()
        if not stripped.startswith("tokio "):
            continue
        saw_tokio_line = True
        # Format: "tokio v1.42.0 feature1,feature2,..."
        parts = stripped.split(maxsplit=2)
        if len(parts) < 3:
            continue
        for raw_feature in parts[2].split(","):
            feature = raw_feature.strip()
            if feature:
                active.add(feature)
    if not saw_tokio_line:
        raise RuntimeError(
            "egress audit: no tokio entry found in `cargo tree --format` output; "
            "either the workspace dropped tokio (update this audit) or cargo's "
            "tree format changed (update the parser)"
        )
    return active


def main() -> int:
    repo_root = Path(__file__).resolve().parent.parent
    manifest = repo_root / "scrybe-cli" / "Cargo.toml"
    if not manifest.is_file():
        print(f"egress audit: manifest not found at {manifest}", file=sys.stderr)
        return 2

    tree = run_cargo_tree(manifest)
    packages = collect_packages(tree)
    hits = sorted(NETWORK_DENYLIST & packages)

    feature_tree = run_cargo_tree_with_features(manifest)
    tokio_features = tokio_active_features(feature_tree)
    forbidden_tokio = sorted(TOKIO_FORBIDDEN_FEATURES & tokio_features)

    print(f"egress audit: {len(packages)} packages in default-feature graph")
    print(f"egress audit: tokio features active = {sorted(tokio_features)}")

    if not hits and not forbidden_tokio:
        print("egress audit: ok — no networking crates, no forbidden tokio features")
        return 0

    if hits:
        print()
        print("egress audit FAILED — networking crates present in default build:")
        for crate in hits:
            print(f"  - {crate}")
        print()
        print("If this is intentional, gate the dep behind a non-default")
        print("feature (e.g. `openai-compat`) and update this denylist.")
    if forbidden_tokio:
        print()
        print("egress audit FAILED — forbidden tokio features active:")
        for feature in forbidden_tokio:
            print(f"  - tokio/{feature}")
        print()
        print("Default-feature builds must not enable tokio's net/process layer.")
    return 1


if __name__ == "__main__":
    sys.exit(main())
