#!/usr/bin/env python3
# Copyright 2026 Mathews Tom
# Licensed under the Apache License, Version 2.0 (the "License");
# you may not use this file except in compliance with the License.
# You may obtain a copy of the License at
#     https://www.apache.org/licenses/LICENSE-2.0
"""Egress-baseline audit for the desktop application.

`scripts/check-egress-baseline.py` audits the library and command-line
graph and says so in its own header: the desktop host declares its own
workspace and resolves its own lockfile, and that gate never reaches it.
This one does, and it asks a different question, because the desktop
host is the only build in the tree that may legitimately carry an HTTP
client at all.

Four things are asserted.

1. The **shipped** graph — the host built with default features, which
   is what `tauri build` produces and what a user installs — carries
   **exactly** the approved set below. Equal to it, not merely
   contained in it. Equality is the point, and it is the primary check
   here. A subset check passes a build that has quietly lost its TLS
   implementation; a non-empty check passes a build that has quietly
   gained a WebSocket transport. The approved set is what one HTTPS
   client costs, and a change to it is a change a reviewer has to agree
   to here.

   This is also the assertion `scripts/qualify-desktop-app.py
   --scenario lifecycle` makes about the running application, for the
   same reason: the shipped application offers authenticated updates,
   an in-app model download, and notes regeneration through a configured
   provider. "No HTTP client anywhere in the graph" is not a property it
   can have.

2. The host built with `--no-default-features` still carries exactly that
   approved HTTPS stack. The updater is an unconditional product capability:
   Tauri validates its permission manifest while building the host, and every
   shipped application must be able to authenticate and retrieve updates.
   This minimum-graph check prevents another client, TLS implementation,
   WebSocket, QUIC, or DNS resolver from arriving through that baseline.

3. The updater configuration names exactly the project's HTTPS release
   manifest on GitHub. The updater transport is permanent, so its destination
   is a reviewed supply-chain boundary rather than an arbitrary URL.

4. The checked-in model catalog names exactly one destination, over HTTPS, at
   the approved host, pinned to a revision its own URL carries. The transport
   buys the capability to reach one place; this is the check that the catalog
   only ever points there.

Run locally:

    python3 scripts/check-app-egress-baseline.py

Run in CI: see `.github/workflows/ci.yml` job `desktop-host`.

Exit status 0 means all four held. Exit status 1 means at least one
did not, and the difference is printed. Exit status 2 means the audit
could not run, which is a failure too — a gate that cannot resolve a
graph must not report a clean one.
"""

from __future__ import annotations

import json
import re
import subprocess
import sys
import tomllib
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent
HOST_MANIFEST = REPO_ROOT / "scrybe-desktop" / "src-tauri" / "Cargo.toml"
CATALOG = REPO_ROOT / "scrybe-application" / "models.toml"
TAURI_CONFIG = REPO_ROOT / "scrybe-desktop" / "src-tauri" / "tauri.conf.json"

# Every optional application feature that uses the approved HTTPS stack. Both
# are on by default because the shipped application downloads its model and
# regenerates notes through a configured provider. The updater plugin is the
# unconditional transport baseline and is therefore not listed as a feature.
#
# Listed rather than singular because another optional feature arriving here
# and not in this tuple is the failure mode that would matter: the manifest
# check would keep passing while the feature audit quietly stopped describing
# every product capability using the client.
OPTIONAL_TRANSPORT_FEATURES = ("model-download", "notes-generation")

# Mirrors the denylist the library gate and the lifecycle qualification
# apply. Kept as a literal here rather than imported, because a gate
# that read its own expectations from the thing it audits would pass
# whatever that thing was changed to.
NETWORK_DENYLIST: frozenset[str] = frozenset(
    {
        "reqwest", "hyper", "hyper-util", "h2", "h3", "ureq", "attohttpc", "isahc",
        "surf", "tonic", "tokio-tungstenite", "tungstenite", "rustls", "rustls-pemfile",
        "rustls-webpki", "tokio-rustls", "tokio-native-tls", "native-tls", "openssl",
        "openssl-sys", "boring", "boring-sys", "trust-dns-resolver", "trust-dns-proto",
        "hickory-resolver", "hickory-proto", "quinn", "quinn-proto", "quinn-udp",
    }
)

# Exactly what one rustls-backed HTTPS client costs, and nothing else.
# `reqwest` is the client; `hyper` and `hyper-util` are the protocol
# implementation beneath it; `rustls`, `rustls-webpki`, and
# `tokio-rustls` are the TLS stack. No OpenSSL, no QUIC, no WebSocket
# transport, no async DNS resolver.
APPROVED_NETWORK_STACK: frozenset[str] = frozenset(
    {
        "reqwest",
        "hyper",
        "hyper-util",
        "rustls",
        "rustls-webpki",
        "tokio-rustls",
    }
)

# The one host the catalog may name. A model destination is a supply
# chain, and widening it is a decision, not a configuration change.
APPROVED_MODEL_HOST = "huggingface.co"
APPROVED_UPDATER_ENDPOINT = (
    "https://github.com/Mathews-Tom/scrybe/releases/latest/download/latest.json"
)

PACKAGE_LINE = re.compile(r"^([A-Za-z0-9_.-]+) v[0-9]")


def host_graph(features: list[str] | None = None, *, no_default_features: bool = False) -> set[str]:
    """Every crate the desktop host links, under one feature selection.

    Called with neither argument this resolves the shipped graph, which
    is the default feature set and therefore includes the transport.

    Raises `RuntimeError` rather than returning an empty set on
    failure: an audit that silently saw nothing is worse than no audit.
    """
    command = [
        "cargo", "tree",
        "--manifest-path", str(HOST_MANIFEST),
        "--edges", "normal",
        "--prefix", "none",
        "--format", "{p}",
    ]
    if no_default_features:
        command.append("--no-default-features")
    for feature in features or []:
        command += ["--features", feature]
    result = subprocess.run(command, capture_output=True, text=True, check=False)
    if result.returncode != 0:
        raise RuntimeError(
            f"cargo tree failed (rc={result.returncode}):\n{result.stderr}"
        )
    packages = {
        match.group(1)
        for match in (PACKAGE_LINE.match(line.strip()) for line in result.stdout.splitlines())
        if match is not None
    }
    if not packages:
        raise RuntimeError("cargo tree resolved no packages at all for the desktop host")
    return packages


def optional_transport_features_shipped_and_still_named() -> list[str]:
    """Which optional transport-using features are both declared and default.

    Two properties per feature, because neither is sufficient alone. It must
    be in the default feature set, or the shipped application cannot do the
    thing the feature exists for and the documentation saying it can is wrong.
    It must also remain named so the minimum graph continues to isolate the
    unconditional updater baseline.
    """
    manifest = tomllib.loads(HOST_MANIFEST.read_text())
    features = manifest.get("features", {})
    default = features.get("default", [])
    return [
        name
        for name in OPTIONAL_TRANSPORT_FEATURES
        if name in features and name in default
    ]


def updater_endpoints() -> list[str]:
    """Every updater endpoint in the Tauri configuration."""
    config = json.loads(TAURI_CONFIG.read_text())
    return config["plugins"]["updater"]["endpoints"]


def catalog_destinations() -> list[tuple[str, str, str]]:
    """Every `(id, url, revision)` the checked-in catalog names."""
    catalog = tomllib.loads(CATALOG.read_text())
    return [
        (entry["id"], entry["source_url"], entry["source_revision"])
        for entry in catalog["model"]
    ]


def report(label: str, expected: object, observed: object) -> bool:
    ok = expected == observed
    print(f"{'ok  ' if ok else 'FAIL'}  {label}")
    if not ok:
        print(f"        expected: {expected!r}")
        print(f"        observed: {observed!r}")
    return ok


def main() -> int:
    if not HOST_MANIFEST.is_file():
        print(f"app egress audit: no desktop host manifest at {HOST_MANIFEST}", file=sys.stderr)
        return 2
    if not CATALOG.is_file():
        print(f"app egress audit: no model catalog at {CATALOG}", file=sys.stderr)
        return 2
    if not TAURI_CONFIG.is_file():
        print(f"app egress audit: no Tauri config at {TAURI_CONFIG}", file=sys.stderr)
        return 2

    try:
        shipped_graph = host_graph([])
        minimum_graph = host_graph(no_default_features=True)
    except RuntimeError as error:
        print(f"app egress audit: {error}", file=sys.stderr)
        return 2

    held = [
        report(
            "the host ships every optional transport-using feature",
            list(OPTIONAL_TRANSPORT_FEATURES),
            optional_transport_features_shipped_and_still_named(),
        ),
        report(
            "the shipped host graph carries exactly the approved network stack",
            sorted(APPROVED_NETWORK_STACK),
            sorted(NETWORK_DENYLIST & shipped_graph),
        ),
        report(
            "the minimum host graph carries only the updater network stack",
            sorted(APPROVED_NETWORK_STACK),
            sorted(NETWORK_DENYLIST & minimum_graph),
        ),
        report(
            "the updater names exactly the approved release manifest",
            [APPROVED_UPDATER_ENDPOINT],
            updater_endpoints(),
        ),
    ]

    for model_id, url, revision in catalog_destinations():
        held.append(
            report(
                f"catalog entry {model_id!r} is served over HTTPS from the approved host",
                f"https://{APPROVED_MODEL_HOST}",
                f"{url.split('://')[0]}://{url.split('/')[2] if '//' in url else '?'}",
            )
        )
        held.append(
            report(
                f"catalog entry {model_id!r} names a URL pinned to its own revision",
                True,
                revision in url,
            )
        )

    print()
    if all(held):
        print(
            f"app egress audit: ok — {len(shipped_graph)} crates in the shipped graph, "
            f"{len(minimum_graph)} in the updater-only minimum graph, "
            f"{len(catalog_destinations())} approved model destination(s)"
        )
        return 0
    print(f"app egress audit FAILED — {held.count(False)} of {len(held)} checks")
    return 1


if __name__ == "__main__":
    sys.exit(main())
