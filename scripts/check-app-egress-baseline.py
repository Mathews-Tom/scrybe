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

Three things are asserted.

1. The host's **default** graph carries none of the denylisted HTTP,
   TLS, DNS, QUIC, or WebSocket crates. That is the graph the shipped
   application and `scripts/qualify-desktop-app.py --scenario lifecycle`
   both build, and it is what makes that scenario's own no-network
   assertion mean something.

2. The host's graph with `model-download` enabled carries **exactly**
   the approved set below — equal to it, not merely contained in it.
   Equality is the point. A subset check passes a build that has
   quietly lost its TLS implementation; a non-empty check passes a
   build that has quietly gained a WebSocket transport. The approved
   set is what one HTTPS client costs, and a change to it is a change
   a reviewer has to agree to here.

3. The checked-in model catalog names exactly one destination, over
   HTTPS, at the approved host, pinned to a revision its own URL
   carries. The feature buys the capability to reach one place; this is
   the check that the catalog only ever points there.

Run locally:

    python3 scripts/check-app-egress-baseline.py

Run in CI: see `.github/workflows/ci.yml` job `desktop-host`.

Exit status 0 means all three held. Exit status 1 means at least one
did not, and the difference is printed. Exit status 2 means the audit
could not run, which is a failure too — a gate that cannot resolve a
graph must not report a clean one.
"""

from __future__ import annotations

import re
import subprocess
import sys
import tomllib
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent
HOST_MANIFEST = REPO_ROOT / "scrybe-desktop" / "src-tauri" / "Cargo.toml"
CATALOG = REPO_ROOT / "scrybe-application" / "models.toml"

# The feature that buys the host an HTTP client. Off by default; this
# is the only build in the tree that turns it on.
DOWNLOAD_FEATURE = "model-download"

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
APPROVED_WITH_DOWNLOAD: frozenset[str] = frozenset(
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

PACKAGE_LINE = re.compile(r"^([A-Za-z0-9_.-]+) v[0-9]")


def host_graph(features: list[str]) -> set[str]:
    """Every crate the desktop host links, with `features` enabled.

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
    for feature in features:
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


def declares_download_feature() -> bool:
    """Whether the host still gates its transport behind a feature.

    A host that made the transport unconditional would resolve one
    graph, and both checks below would be asking the same question of
    it. The gating is the property; this is what notices its removal.
    """
    manifest = tomllib.loads(HOST_MANIFEST.read_text())
    features = manifest.get("features", {})
    return DOWNLOAD_FEATURE in features and features.get("default") == []


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

    try:
        default_graph = host_graph([])
        download_graph = host_graph([DOWNLOAD_FEATURE])
    except RuntimeError as error:
        print(f"app egress audit: {error}", file=sys.stderr)
        return 2

    held = [
        report(
            f"the transport is still gated behind a default-off `{DOWNLOAD_FEATURE}`",
            True,
            declares_download_feature(),
        ),
        report(
            "the default host graph carries no HTTP, TLS, DNS, QUIC, or WebSocket crate",
            [],
            sorted(NETWORK_DENYLIST & default_graph),
        ),
        report(
            f"the `{DOWNLOAD_FEATURE}` host graph carries exactly the approved transport",
            sorted(APPROVED_WITH_DOWNLOAD),
            sorted(NETWORK_DENYLIST & download_graph),
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
            f"app egress audit: ok — {len(default_graph)} crates by default, "
            f"{len(download_graph)} with `{DOWNLOAD_FEATURE}`, "
            f"{len(catalog_destinations())} approved model destination(s)"
        )
        return 0
    print(f"app egress audit FAILED — {held.count(False)} of {len(held)} checks")
    return 1


if __name__ == "__main__":
    sys.exit(main())
