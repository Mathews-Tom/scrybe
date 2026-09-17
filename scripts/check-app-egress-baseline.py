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
   same reason: the shipped application offers an in-app model
   download, so "no HTTP client anywhere in the graph" is not a
   property it can have, and asserting it over a configuration nobody
   ships would read as a guarantee while covering nothing.

2. The host built with `--no-default-features` carries none of the
   denylisted crates at all. Nobody ships that configuration and this
   gate does not pretend otherwise. It is checked because it is what
   proves the gating is real: `model-download` genuinely controls
   whether a transport is linked, rather than naming a dependency that
   arrives through some other edge regardless. If that stops being
   true, the feature is a label and the first check above is no longer
   measuring what it claims.

3. The checked-in model catalog names exactly one destination, over
   HTTPS, at the approved host, pinned to a revision its own URL
   carries. The transport buys the capability to reach one place; this
   is the check that the catalog only ever points there.

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

# The feature that buys the host an HTTP client. On by default, because
# the shipped application performs an in-app model download; still a
# named feature so that turning it off resolves a graph with no
# transport, which is what check 2 reads.
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


def ships_the_transport_behind_a_named_feature() -> bool:
    """Whether the host ships the transport, and still names it.

    Two properties in one answer, because neither is sufficient alone.
    The transport must be in the default feature set, or the shipped
    application cannot download a model and the documentation that says
    it can is wrong. And it must still be a named feature, or the two
    graphs below are the same graph and the second check stops asking
    anything.
    """
    manifest = tomllib.loads(HOST_MANIFEST.read_text())
    features = manifest.get("features", {})
    return DOWNLOAD_FEATURE in features and DOWNLOAD_FEATURE in features.get("default", [])


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
        shipped_graph = host_graph([])
        transportless_graph = host_graph(no_default_features=True)
    except RuntimeError as error:
        print(f"app egress audit: {error}", file=sys.stderr)
        return 2

    held = [
        report(
            f"the host ships the transport and still names it `{DOWNLOAD_FEATURE}`",
            True,
            ships_the_transport_behind_a_named_feature(),
        ),
        report(
            "the shipped host graph carries exactly the approved transport and nothing more",
            sorted(APPROVED_WITH_DOWNLOAD),
            sorted(NETWORK_DENYLIST & shipped_graph),
        ),
        report(
            f"the host built without `{DOWNLOAD_FEATURE}` carries no transport at all",
            [],
            sorted(NETWORK_DENYLIST & transportless_graph),
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
            f"{len(transportless_graph)} without `{DOWNLOAD_FEATURE}`, "
            f"{len(catalog_destinations())} approved model destination(s)"
        )
        return 0
    print(f"app egress audit FAILED — {held.count(False)} of {len(held)} checks")
    return 1


if __name__ == "__main__":
    sys.exit(main())
