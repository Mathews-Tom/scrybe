#!/usr/bin/env python3
# Copyright 2026 Mathews Tom
# Licensed under the Apache License, Version 2.0 (the "License");
# you may not use this file except in compliance with the License.
# You may obtain a copy of the License at
#     https://www.apache.org/licenses/LICENSE-2.0
"""Lifecycle qualification for the desktop application.

Drives the actual development candidate — the bundle a double-click
opens — through launch, close, restore, recreation, second launch, and
quit, with no human input and no manual checkpoint.

Why not WebDriver: Tauri's `tauri-driver` supports Windows and Linux
only, because Apple ships no standalone WKWebView driver. The
WebdriverIO `embedded` provider would require shipping an in-app debug
WebDriver plugin, and the cross-platform fork requires a paid macOS key.
None of those is available here.

What replaces it:

- a debug-build-only record the application appends to inside the
  disposable storage root, one JSON object per observation;
- a debug-build-only control socket in the same place, which accepts the
  tray's own menu-item identities and dispatches them through exactly
  the functions the platform dispatches through;
- operating-system facts that need no Accessibility grant: process
  counts for ownership, and `NSRunningApplication` at bundle-identifier
  level for activation and exit.

Both debug-only surfaces are `#[cfg(debug_assertions)]` and the
recording macro expands to nothing in release. This harness proves that
rather than asserting it: it builds the release binary and checks that
the event names present in the candidate are absent from it.

What this cannot establish, stated plainly:

- it dispatches tray actions by menu-item identity rather than by
  synthesizing a click on the native menu, because no supported macOS
  interface does the latter without an Accessibility grant. The tray
  item's existence and its items' enabled state are read from the
  record the application writes while building it;
- the egress assertion has three parts, and each covers something the
  others do not. The crate-graph check covers the Rust host only, and
  it is an equality rather than an absence: the shipped application
  offers an in-app model download, so it carries one rustls-backed
  HTTPS client by design, and what is asserted is that the denylisted
  crates in its graph are exactly that client — a gained crate and a
  lost one both fail. A graph with no client at all is not a shape the
  application can have, and asserting one over a configuration nobody
  ships would read as a guarantee while covering nothing. The socket
  leg below is what says the compiled-in client stays unused unless
  somebody asks it to do something.

  It says nothing about the webview, which carries the platform's own
  networking stack, so a single remote image, font, stylesheet, or
  `fetch()` added to a future view would egress without changing the
  graph at all. What governs that is the content security policy, which
  is read out of the built bundle and compared against the expected
  value, and the navigation guard, which is driven from the frontend
  and observed rather than assumed. The
  socket sampler runs on a cadence from launch until exit and records
  the union of everything it saw, so a connection opened and closed
  between two samples is still likely to be caught — but sampling is
  sampling, and a connection that opened and closed entirely inside one
  poll interval would be missed;
- the WebView-state assertion covers the two directories macOS resolves
  from the user database for any WKWebView host. It is checked against
  a positive control, so it is known to be able to fail, but a session
  artifact written somewhere else under `~/Library` would not be seen
  by it.

Two scenarios, one build. Both compile the shape the application
ships — default features, which include the model transport — and
there is no per-scenario feature selection at all. There used to be:
`setup` named `model-download` explicitly while the host's default set
did not carry it, and the effect was that the only artifact ever built
with a transport was a qualification candidate. A scenario that
qualifies a binary nobody installs establishes nothing about the one
they do, so the selection is gone and the candidate differs from the
shipped bundle only in the `--debug` profile that makes the control
channel reachable.

What differs is what each run drives, and therefore what its socket
observations mean.

`lifecycle` never asks for a model. Its claim rests on three legs — the
host's crate graph carries exactly the approved transport and nothing
more; the content security policy read back out of the built artifact
is the expected one directive for directive; and the socket sampler
observed no internet socket at all. That last leg is what says the
compiled-in client stays unused unless somebody asks it to act.

`setup` does ask, against a fixture on loopback, so an empty set of
destinations would mean the run had not exercised what it exists to
exercise. Its socket leg is the stronger statement rather than the
weaker one: every destination observed must be the local fixture, on
loopback, on the fixture's own port, with a separate assertion that
some socket was seen at all so the check cannot pass by vacuity.

Run locally:

    python3 scripts/qualify-desktop-app.py --hermetic --scenario lifecycle
    python3 scripts/qualify-desktop-app.py --hermetic --scenario setup

Exit status 0 means every check held. Exit status 1 means at least one
did not; each failing check prints what was expected and what was
observed.
"""

from __future__ import annotations

import argparse
import hashlib
import http.server
import ipaddress
import json
import os
import re
import shutil
import socket
import subprocess
import sys
import tempfile
import threading
import time
from collections.abc import Callable
from dataclasses import dataclass, field
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent
DESKTOP = REPO_ROOT / "scrybe-desktop"
HOST = DESKTOP / "src-tauri"

BUNDLE_IDENTIFIER = "dev.scrybe.desktop"
BUNDLE_NAME = "Scrybe.app"
EXECUTABLE = "scrybe-desktop"

# What `pgrep -f` is given, rather than the path itself.
#
# `pgrep -f` matches against whole command lines, and a `pgrep`
# invocation's own command line contains the pattern it was given. It
# excludes itself, but not another `pgrep` running the same query
# concurrently — which the socket sampler does, from its own thread, for
# the whole run. Bracketing the first character makes the pattern a
# regular expression that matches the application's command line and not
# the command line of any process carrying the pattern literally.
PROCESS_PATTERN = f"[{BUNDLE_NAME[0]}]{BUNDLE_NAME[1:]}/Contents/MacOS/{EXECUTABLE}"

LIFECYCLE_RECORD = ".desktop-lifecycle.jsonl"
CONTROL_SOCKET = ".desktop-control.sock"

# `tauri-plugin-single-instance` coordinates through this path. It is
# not application data, it is named here rather than hidden, and the
# harness asserts it is gone once the process exits.
SINGLE_INSTANCE_SOCKET = Path("/tmp") / f"{BUNDLE_IDENTIFIER.replace('.', '_')}_si.sock"

# The paths a misconfigured candidate would fall back to. A hermetic run
# must leave both exactly as it found them.
REAL_CONFIG = Path.home() / "Library/Application Support/dev.scrybe.scrybe/config.toml"
REAL_STORAGE_ROOT = Path.home() / "scrybe"

# Where the application resolves managed models when the configuration
# does not redirect it. A hermetic run must leave it alone: it holds
# whatever this developer has installed, and half a gigabyte of it.
PLATFORM_MODELS = Path.home() / "Library/Application Support/dev.scrybe.scrybe/models"

# macOS creates these for any WKWebView host, resolved from the user
# database rather than from `$HOME`, so no launch-time environment can
# redirect them. They are WebView state, not application data; the
# harness asserts no session artifact reaches them.
WEBVIEW_STATE = [
    Path.home() / "Library/WebKit" / BUNDLE_IDENTIFIER,
    Path.home() / "Library/Caches" / BUNDLE_IDENTIFIER,
]
SESSION_ARTIFACTS = ("meta.toml", "transcript.md", "notes.md", "audio.opus", "journal")

# A bundled runtime would contradict the whole reason for using the
# platform's own WebView. Matched case-insensitively against every file
# name in the bundle.
FOREIGN_RUNTIME_MARKERS = ("node", "chrom", "electron", "ffmpeg", ".asar", "v8_context")

# Event names the candidate records. Their absence from a release binary
# is what "compiled out, not merely inert" means here.
DEBUG_ONLY_STRINGS = [
    LIFECYCLE_RECORD,
    CONTROL_SOCKET,
    "window-hidden",
    "window-recreated",
    "second-launch-activated",
    "quit-accepted",
    "implicit-exit-prevented",
]

# The model-acquisition probe the `setup` scenario drives. Its absence
# from a release binary is what makes the probe a debug affordance
# rather than a shipped one; the `lifecycle` scenario does not drive it,
# so it checks the list above and this one is checked where it is used.
MODEL_PROBE_STRINGS = ["probe-model-install", "probe-model-cancel", "probe-model-offer"]

# The artifact the setup scenario's own server hands the application.
#
# Large enough that the download is not instantaneous, so a sampler can
# look at the models directory while it is in flight and see what is
# there mid-way; small enough that a run does not spend real time on it.
FIXTURE_ARTIFACT_BYTES = 6 * 1024 * 1024

# How long the fixture server takes to serve the whole artifact, spread
# evenly across its chunks. Long enough for the mid-flight samples and
# the cancellation below to land inside it.
FIXTURE_SERVE_SECONDS = 3.0
FIXTURE_CHUNK_BYTES = 64 * 1024

# The filename the fixture artifact is promoted to. Deliberately not the
# catalog's own, so a run cannot be confused with one that fetched the
# real model.
FIXTURE_DESTINATION = "ggml-harness-fixture.bin"

# How often the models directory is sampled while a download is running.
MODELS_SAMPLE_SECONDS = 0.05

# Crates whose presence would mean the host can speak HTTP, TLS, DNS, or
# a streaming transport to a remote peer. Mirrors the denylist
# `scripts/check-egress-baseline.py` applies to the library and CLI
# graph; the desktop host is a separate workspace that gate cannot see.
NETWORK_DENYLIST = frozenset(
    {
        "reqwest", "hyper", "hyper-util", "h2", "h3", "ureq", "attohttpc", "isahc",
        "surf", "tonic", "tokio-tungstenite", "tungstenite", "rustls", "rustls-pemfile",
        "rustls-webpki", "tokio-rustls", "tokio-native-tls", "native-tls", "openssl",
        "openssl-sys", "boring", "boring-sys", "trust-dns-resolver", "trust-dns-proto",
        "hickory-resolver", "hickory-proto", "quinn", "quinn-proto", "quinn-udp",
    }
)

# Exactly which of those the shipped host is allowed to carry: one
# rustls-backed HTTPS client, and nothing else. Mirrors the set
# `scripts/check-app-egress-baseline.py` approves, and is compared for
# equality rather than containment — a subset passes a build that lost
# its TLS implementation, a non-empty check passes a build that gained
# a WebSocket transport, and both are changes a reviewer must agree to
# rather than notice later.
APPROVED_TRANSPORT = frozenset(
    {"reqwest", "hyper", "hyper-util", "rustls", "rustls-webpki", "tokio-rustls"}
)

SETTLE_SECONDS = 2.0
POLL_SECONDS = 0.1

# How often the socket sampler looks, once it is running. Short enough
# that a request which opens and closes across a settle is seen, long
# enough that `lsof` is not the thing being measured.
SAMPLE_SECONDS = 0.25

# How far past the start of a policy to read before giving up on
# finding its end. Generous: the shipped policy is under 300 bytes, and
# over-reading costs nothing because the grammar stops the match.
POLICY_WINDOW = 4096

# The content security policy the bundle must ship, read back out of the
# built `index.html`. Declared here rather than read from
# `tauri.conf.json`, because a check that reads the policy from the same
# file the policy is written in would pass whatever it was changed to.
#
# `connect-src` closes fetch, XHR, WebSocket, and beacon; `frame-src`
# closes iframes; `form-action` closes form submission, which does not
# inherit from `default-src` and so has to be named. Top-level
# navigation is governed by none of them — that is the navigation guard,
# checked separately below.
EXPECTED_CSP = (
    "default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; "
    "img-src 'self' data:; connect-src 'self' ipc: http://ipc.localhost; "
    "object-src 'none'; base-uri 'none'; frame-src 'none'; form-action 'none'"
)

# The origin the bundled application is served from. A window URL with
# any other origin after the navigation probe means the guard let it go.
APPLICATION_ORIGIN = "tauri://localhost"

# The tray's own menu-item identities, which the control channel accepts
# so a run drives the same dispatch the platform drives.
TRAY_OPEN = "open"
TRAY_QUIT = "quit"

# Control verbs that are not tray items: the two window verbs, and the
# navigation probe that drives the exfiltration path the policy cannot
# govern.
NAVIGATE_OFFSITE = "navigate-offsite"
OPEN_OFFSITE = "open-offsite"
RECORD_WINDOW_URL = "record-window-url"
RECORD_WEBVIEWS = "record-webviews"

# The one webview label this application ever creates. A second one
# means a new-window request was answered rather than dropped.
MAIN_WEBVIEW = "main"


@dataclass
class Evidence:
    """One recorded observation, and whether it held."""

    check: str
    expected: str
    observed: str
    ok: bool


@dataclass
class Run:
    """Accumulated evidence for one scenario."""

    entries: list[Evidence] = field(default_factory=list)

    def record(self, check: str, expected: object, observed: object) -> None:
        self.entries.append(
            Evidence(check, repr(expected), repr(observed), expected == observed)
        )

    def assert_that(self, check: str, condition: bool, detail: str) -> None:
        self.entries.append(
            Evidence(check, "holds", detail if condition else f"FAILED: {detail}", condition)
        )

    @property
    def failures(self) -> list[Evidence]:
        return [entry for entry in self.entries if not entry.ok]


def build_candidate() -> Path:
    """Builds the bundle a double-click opens, and returns its path.

    Default features, and no way to ask for anything else. That is the
    point: the candidate every scenario drives differs from the bundle
    a user installs only in the `--debug` profile, which is what makes
    the debug-only control channel reachable. No scenario can qualify a
    feature selection nobody ships.
    """
    subprocess.run(
        ["pnpm", "--dir", str(DESKTOP), "install", "--frozen-lockfile"],
        cwd=REPO_ROOT,
        check=True,
    )
    subprocess.run(
        [
            "pnpm", "--dir", str(DESKTOP), "exec",
            "tauri", "build", "--debug", "--bundles", "app",
        ],
        cwd=REPO_ROOT,
        check=True,
    )
    bundle = HOST / "target" / "debug" / "bundle" / "macos" / BUNDLE_NAME
    if not bundle.is_dir():
        raise RuntimeError(f"the candidate bundle was not produced at {bundle}")
    return bundle


def build_release_binary() -> Path:
    """Builds the release host, for the compiled-out assertion.

    Default features, like the candidate: the string pools being
    compared have to come from the same feature selection, or the
    absence of a debug-only symbol from the release binary would say
    as much about a feature as about the profile.
    """
    subprocess.run(
        [
            "cargo", "build", "--manifest-path", str(HOST / "Cargo.toml"),
            "--release", "--bin", EXECUTABLE, "--locked",
        ],
        cwd=REPO_ROOT,
        check=True,
    )
    return HOST / "target" / "release" / EXECUTABLE


class Candidate:
    """The running application, and the facts observable about it."""

    def __init__(self, bundle: Path, workspace: Path) -> None:
        self.bundle = bundle
        self.workspace = workspace
        self.root = workspace / "sessions"
        self.models = workspace / "models"
        self.config = workspace / "config.toml"
        self.root.mkdir(parents=True)
        # Three things this configuration has to do, none of them
        # obvious from the storage root alone.
        #
        # An absolute `[stt].model` is what redirects managed model
        # storage: the application resolves the models directory from
        # it through the same resolver the recorder loads a model
        # through. Without it a run reads the platform directory
        # holding this developer's own install — and deciding whether
        # what is there is the catalog's artifact means hashing half a
        # gigabyte of it, on the thread a window is waiting on.
        #
        # Both provider endpoints are remote, and deliberately at
        # `.invalid`, which is reserved and resolves nowhere. The
        # application dials a *loopback* notes endpoint to report
        # whether local notes are available, and the built-in default
        # is one; under the defaults a run's socket observations would
        # therefore depend on whether this machine happens to be
        # running a local provider. A remote endpoint is reported from
        # its URL and never dialled, so what the sockets show is what
        # the run did rather than what the machine was doing.
        self.config.write_text(
            f'[storage]\nroot = "{self.root}"\n\n'
            f'[stt]\nprovider = "openai-compat"\n'
            f'base_url = "https://stt.invalid/v1"\n'
            f'model = "{self.models / "ggml-small.en.bin"}"\n\n'
            f'[llm]\nbase_url = "https://notes.invalid/v1"\n'
        )

    # -- operating-system facts -------------------------------------

    def process_ids(self) -> list[int]:
        """Every process running this bundle's executable."""
        result = subprocess.run(
            ["pgrep", "-f", PROCESS_PATTERN],
            capture_output=True,
            text=True,
            check=False,
        )
        return [int(line) for line in result.stdout.split()]

    def registered_process_ids(self) -> list[int]:
        """Every process the window server knows by bundle identifier.

        `NSRunningApplication` answers this without an Accessibility
        grant, and it is the level at which "the application is running"
        and "the application was activated" are actually defined.
        """
        script = (
            'ObjC.import("AppKit");'
            f'const a = $.NSRunningApplication.runningApplicationsWithBundleIdentifier("{BUNDLE_IDENTIFIER}");'
            "JSON.stringify(Array.from({length: a.count}, (_, i) => a.objectAtIndex(i).processIdentifier))"
        )
        result = subprocess.run(
            ["osascript", "-l", "JavaScript", "-e", script],
            capture_output=True,
            text=True,
            check=True,
        )
        return json.loads(result.stdout.strip() or "[]")

    def internet_sockets(self) -> list[str]:
        """Internet sockets held by any process of this application."""
        found: list[str] = []
        for pid in self.process_ids():
            result = subprocess.run(
                ["lsof", "-a", "-p", str(pid), "-i", "-nP"],
                capture_output=True,
                text=True,
                check=False,
            )
            found.extend(line for line in result.stdout.splitlines()[1:] if line.strip())
        return found

    # -- the application's own record --------------------------------

    def records(self) -> list[dict]:
        path = self.root / LIFECYCLE_RECORD
        if not path.exists():
            return []
        return [json.loads(line) for line in path.read_text().splitlines() if line.strip()]

    def events(self) -> list[str]:
        return [record["event"] for record in self.records()]

    def detail_of(self, event: str) -> str | None:
        for record in self.records():
            if record["event"] == event:
                return record.get("detail")
        return None

    def last_detail_of(self, event: str) -> str | None:
        """The most recent `event`, for one a run records more than once.

        The window URL is read once after the navigation probe and again
        after the new-window probe, and it is the second reading that
        answers the second question. `detail_of` returns the first
        match, so asking it would have re-asserted the earlier answer
        and reported a check that could not fail.
        """
        for record in reversed(self.records()):
            if record["event"] == event:
                return record.get("detail")
        return None

    # -- driving it --------------------------------------------------

    def launch(self) -> None:
        """Opens the bundle the way a double-click does."""
        environment = dict(os.environ, SCRYBE_CONFIG=str(self.config))
        subprocess.run(
            ["open", "-n", str(self.bundle)], env=environment, check=True
        )

    def control(self, verb: str) -> None:
        """Sends one control verb and waits for the process to settle."""
        path = self.root / CONTROL_SOCKET
        stream = socket.socket(socket.AF_UNIX)
        try:
            stream.connect(str(path))
            stream.sendall(f"{verb}\n".encode())
        finally:
            stream.close()
        time.sleep(SETTLE_SECONDS / 2)

    def wait_for_control_socket(self, timeout: float = 60.0) -> bool:
        return self._wait(lambda: (self.root / CONTROL_SOCKET).is_socket(), timeout)

    def wait_for_event(self, event: str, count: int = 1, timeout: float = 20.0) -> bool:
        return self._wait(lambda: self.events().count(event) >= count, timeout)

    def wait_for_exit(self, timeout: float = 20.0) -> bool:
        return self._wait(lambda: not self.process_ids(), timeout)

    @staticmethod
    def _wait(condition: Callable[[], bool], timeout: float) -> bool:
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            if condition():
                return True
            time.sleep(POLL_SECONDS)
        return condition()

    def terminate(self) -> None:
        for pid in self.process_ids():
            subprocess.run(["kill", "-9", str(pid)], check=False)


class SocketSampler:
    """Every internet socket the application held, sampled on a cadence.

    One sample at one instant cannot see a connection opened during
    startup and closed before that instant, which is the dominant shape
    of an unwanted request. This runs from launch until the scenario
    stops it and records the union of everything it saw. It is still
    sampling: a connection opened and closed entirely inside one poll
    interval would be missed.
    """

    def __init__(self, candidate: Candidate) -> None:
        self._candidate = candidate
        self._seen: set[str] = set()
        self._stop = threading.Event()
        self._thread = threading.Thread(target=self._run, daemon=True)

    def _run(self) -> None:
        while not self._stop.is_set():
            self._seen.update(self._candidate.internet_sockets())
            self._stop.wait(SAMPLE_SECONDS)

    def start(self) -> None:
        self._thread.start()

    def stop(self) -> list[str]:
        """Stops sampling and returns everything seen, oldest call last."""
        self._stop.set()
        self._thread.join(timeout=SETTLE_SECONDS)
        self._seen.update(self._candidate.internet_sockets())
        return sorted(self._seen)


class FixtureServer:
    """An artifact source on loopback, which counts what it is asked for.

    The checked-in catalog names a half-gigabyte artifact on a public
    host. A qualification run must not request it, so the application is
    handed a manifest describing this instead — and because this server
    built the artifact, it knows its exact size and digest and can
    deliberately misstate either.

    It serves slowly on purpose. An instantaneous download cannot be
    observed part way through, and "the destination name never appears
    before verification" is a statement about the middle of a download
    rather than about its ends.
    """

    def __init__(self) -> None:
        self.body = bytes(
            (index * 37 + 11) % 251 for index in range(FIXTURE_ARTIFACT_BYTES)
        )
        self.digest = hashlib.sha256(self.body).hexdigest()
        self.requests: list[str] = []
        self._lock = threading.Lock()
        self._server = self._build()
        self._thread = threading.Thread(target=self._server.serve_forever, daemon=True)

    def _build(self) -> http.server.ThreadingHTTPServer:
        outer = self

        class Handler(http.server.BaseHTTPRequestHandler):
            protocol_version = "HTTP/1.1"

            def do_GET(self) -> None:  # noqa: N802 - the interface's own spelling
                with outer._lock:
                    outer.requests.append(self.path)
                self.send_response(200)
                self.send_header("Content-Type", "application/octet-stream")
                self.send_header("Content-Length", str(len(outer.body)))
                self.end_headers()
                chunks = max(1, len(outer.body) // FIXTURE_CHUNK_BYTES)
                pause = FIXTURE_SERVE_SECONDS / chunks
                for start in range(0, len(outer.body), FIXTURE_CHUNK_BYTES):
                    try:
                        self.wfile.write(outer.body[start : start + FIXTURE_CHUNK_BYTES])
                        self.wfile.flush()
                    except (BrokenPipeError, ConnectionResetError):
                        # A cancelled download closes the connection
                        # mid-stream. That is the behaviour under test,
                        # not a server failure.
                        return
                    time.sleep(pause)

            def log_message(self, *_: object) -> None:
                """Silence the default stderr access log."""

        class Server(http.server.ThreadingHTTPServer):
            def handle_error(self, *_: object) -> None:
                """Say nothing about a connection the client dropped.

                A cancelled download closes the socket mid-response, so
                the default handler would print a traceback for the one
                behaviour this scenario most wants to exercise.
                """

        return Server(("127.0.0.1", 0), Handler)

    @property
    def port(self) -> int:
        return self._server.server_address[1]

    def url(self, path: str = "/fixture/model.bin") -> str:
        return f"http://127.0.0.1:{self.port}{path}"

    def count(self) -> int:
        with self._lock:
            return len(self.requests)

    def start(self) -> None:
        self._thread.start()

    def stop(self) -> None:
        self._server.shutdown()
        self._server.server_close()


class ModelsSampler:
    """What the models directory held, sampled while a download ran.

    Atomic promotion is a statement about the middle of a download: the
    destination name must not exist while bytes are still arriving. A
    single look before and after cannot see that, so this looks
    repeatedly and records every distinct thing it saw.
    """

    def __init__(self, directory: Path, destination: str, size: int) -> None:
        self._directory = directory
        self._destination = destination
        self._size = size
        self._short_destination: list[int] = []
        self._saw_partial = False
        self._stop = threading.Event()
        self._thread = threading.Thread(target=self._run, daemon=True)

    def _run(self) -> None:
        while not self._stop.is_set():
            target = self._directory / self._destination
            if target.is_file():
                size = target.stat().st_size
                if size != self._size:
                    self._short_destination.append(size)
            if any(self._directory.glob("*.partial")) if self._directory.is_dir() else False:
                self._saw_partial = True
            self._stop.wait(MODELS_SAMPLE_SECONDS)

    def start(self) -> None:
        self._thread.start()

    def stop(self) -> tuple[list[int], bool]:
        """Returns the destination sizes seen that were not the full one,
        and whether a partial was ever observed."""
        self._stop.set()
        self._thread.join(timeout=SETTLE_SECONDS)
        return self._short_destination, self._saw_partial


def snapshot(path: Path) -> object:
    """A comparable fingerprint of a path the run must not touch."""
    if not path.exists():
        return "absent"
    if path.is_dir():
        return sorted(entry.name for entry in path.iterdir())
    return (path.stat().st_size, path.stat().st_mtime_ns)


def host_dependency_graph() -> set[str]:
    """Every crate the desktop host links."""
    result = subprocess.run(
        [
            "cargo", "tree", "--manifest-path", str(HOST / "Cargo.toml"),
            "--edges", "normal", "--prefix", "none", "--format", "{p}",
        ],
        capture_output=True,
        text=True,
        check=True,
    )
    return {
        match.group(1)
        for match in (re.match(r"^([A-Za-z0-9_.-]+) v", line.strip()) for line in result.stdout.splitlines())
        if match is not None
    }


# One content security policy, read out of the string pool.
#
# Tauri compiles the configuration into the executable rather than
# shipping it as data, so the policy sits in the pool with its length
# held in code and nothing at all marking where it ends — the next
# configuration string is glued straight onto its last character. A
# check that took a fixed number of bytes from the start of the policy
# would therefore be the containment test this replaces wearing a
# different shape, and containment is exactly what a relaxation
# survives: a policy formed by appending a directive still contains the
# expected one verbatim, and still carries one `default-src`.
#
# What does mark the end is the grammar. A policy is a `;`-separated
# list of directives; a directive is a name followed by space-separated
# source expressions, each of them either quoted or free of spaces,
# semicolons, and quotes. The policy therefore runs until the first
# byte that can continue neither, which is where the next configuration
# string begins.
# A directive is a name followed by space-separated source
# expressions, each either quoted or free of spaces, semicolons, and
# quotes. A policy is those, separated by semicolons.
DIRECTIVE = r"[a-z][a-z0-9-]*(?: (?:'[^']*'|[^ ;']+))*"
POLICY = re.compile(f"{DIRECTIVE}(?:; ?{DIRECTIVE})*")


def shipped_policies(binary: Path) -> list[str]:
    """Every content security policy the built executable carries.

    Tauri embeds the frontend in the executable rather than shipping it
    as a file in the bundle, and embeds the configured policy alongside
    it as the value it serves the `Content-Security-Policy` header
    from. Reading it back out of the built artifact is what makes this a
    check on what was shipped rather than on what `tauri.conf.json`
    says, which would pass whatever the file was changed to.

    The caller compares what comes back against the expected policy
    exactly, so an added, removed, or reordered directive is a
    difference rather than a longer string that still contains the old
    one.
    """
    image = binary.read_bytes()
    found = []
    for match in re.finditer(rb"default-src[\x20;]", image):
        window = image[match.start() : match.start() + POLICY_WINDOW]
        found.append(POLICY.match(window.decode("ascii", errors="replace"))[0])
    return found


def hermeticity(candidate: Candidate, run: Run, untouched: list[tuple[str, Path, object]]) -> None:
    """Whether the run stayed inside the root it was given.

    Runs on every exit from the scenario, including the early one. The
    most likely cause of that early return is a hermeticity failure —
    the configuration environment variable not reaching the launched
    process, or a configuration the service layer rejects — in which
    case the application resolved the real configuration and the real
    storage root, wrote its lifecycle record and its control socket
    there, and was then killed with `SIGKILL`, so the exit handler never
    removed the socket. Reporting one failure about a control channel
    and nothing about the root it had just written into would name the
    symptom and hide the breach.
    """
    for label, path, before in untouched:
        run.record(f"confinement: {label} is unchanged", before, snapshot(path))
    run.record(
        "confinement: nothing of this run reached the real storage root",
        [],
        sorted(
            shorten(path)
            for path in (
                REAL_STORAGE_ROOT / LIFECYCLE_RECORD,
                REAL_STORAGE_ROOT / CONTROL_SOCKET,
            )
            if path.exists()
        ),
    )


def webview_state(run: Run) -> None:
    """No session artifact reaches the directories macOS resolves for a
    WKWebView host.

    The assertion passes when the directories hold nothing, so on its
    own it would also pass if it were looking in the wrong place or
    matching nothing. The positive control below is what distinguishes
    those: it plants an artifact-shaped file and requires the same
    function to find it.
    """
    for directory in WEBVIEW_STATE:
        run.record(
            f"confinement: no session artifact reaches {shorten(directory)}",
            [],
            session_artifacts_under(directory),
        )
    with tempfile.TemporaryDirectory() as probe:
        planted = Path(probe) / "Default" / SESSION_ARTIFACTS[0]
        planted.parent.mkdir(parents=True)
        planted.write_text("a session artifact, where one must never be")
        run.record(
            "confinement: the session-artifact check finds one when it is there",
            [f"Default/{SESSION_ARTIFACTS[0]}"],
            session_artifacts_under(Path(probe)),
        )


def lifecycle(candidate: Candidate, run: Run) -> None:
    """One process, one tray, one recoverable window, one clean exit."""
    untouched = [
        ("the real configuration file", REAL_CONFIG, snapshot(REAL_CONFIG)),
        ("the default storage root", REAL_STORAGE_ROOT, snapshot(REAL_STORAGE_ROOT)),
    ]

    # (a) Launch creates one process, one tray item, one window.
    candidate.launch()
    sockets = SocketSampler(candidate)
    sockets.start()
    opened = candidate.wait_for_control_socket()
    run.assert_that(
        "launch: the control channel opens",
        opened,
        f"no control socket appeared at {candidate.root / CONTROL_SOCKET}",
    )
    if not opened:
        # Every later step drives the application through that channel,
        # so continuing would report a cascade of failures that all mean
        # this one thing. The confinement checks are not among them: a
        # control socket that never appeared is most likely a control
        # socket that appeared somewhere else, and the one place it must
        # never be is the user's real storage root.
        run.record(
            "no network: internet sockets held by the application, sampled from launch to exit",
            [],
            sockets.stop(),
        )
        hermeticity(candidate, run, untouched)
        webview_state(run)
        return
    candidate.wait_for_event("window-shown")

    run.record("launch: processes running the executable", 1, len(candidate.process_ids()))
    run.record("launch: processes registered under the bundle identifier", 1, len(candidate.registered_process_ids()))
    run.record("launch: tray items created", 1, candidate.events().count("tray-ready"))
    run.record("launch: windows shown", 1, candidate.events().count("window-shown"))
    run.record(
        "launch: `Record now` is present and not yet available",
        "record:disabled,open:enabled,quit:enabled",
        candidate.detail_of("tray-ready"),
    )

    launched_pid = candidate.process_ids()[0]

    # (b) Closing hides; the process and tray survive; the tray restores.
    candidate.control("close-window")
    run.assert_that(
        "close: the window hides",
        candidate.wait_for_event("window-hidden"),
        "the application recorded no `window-hidden`",
    )
    run.record("close: processes still running", 1, len(candidate.process_ids()))
    run.record("close: still registered under the bundle identifier", 1, len(candidate.registered_process_ids()))

    candidate.control(TRAY_OPEN)
    run.assert_that(
        "tray open: the window comes back",
        candidate.wait_for_event("window-shown", count=2),
        "the application recorded no second `window-shown`",
    )
    run.record("tray open: processes still running", 1, len(candidate.process_ids()))

    # The window can be destroyed rather than hidden. The process must
    # survive that and rebuild the window on the next open.
    candidate.control("destroy-window")
    run.assert_that(
        "destroyed window: the process does not exit with it",
        candidate.wait_for_event("implicit-exit-prevented") and bool(candidate.process_ids()),
        "the process exited when its last window was destroyed",
    )
    candidate.control(TRAY_OPEN)
    run.assert_that(
        "tray open: a destroyed window is rebuilt",
        candidate.wait_for_event("window-recreated"),
        "the application recorded no `window-recreated`",
    )
    run.record("window recreation: processes still running", 1, len(candidate.process_ids()))

    # (c) A second launch activates the first process.
    candidate.launch()
    run.assert_that(
        "second launch: the existing process is activated",
        candidate.wait_for_event("second-launch-activated"),
        "the application recorded no `second-launch-activated`",
    )
    time.sleep(SETTLE_SECONDS)
    run.record("second launch: processes running the executable", 1, len(candidate.process_ids()))
    run.record("second launch: processes registered under the bundle identifier", 1, len(candidate.registered_process_ids()))
    run.record(
        "second launch: the original process is still the owner",
        [launched_pid],
        candidate.process_ids(),
    )
    run.record(
        "second launch: no other process ever wrote to the record",
        {launched_pid},
        {record["pid"] for record in candidate.records()},
    )

    # (h) The exfiltration path no policy can close: frontend code
    # execution, then a top-level navigation carrying what the granted
    # commands return. `connect-src` does not see it and `form-action`
    # does not cover it, so what refuses it is the navigation guard —
    # driven here from the frontend and observed rather than assumed.
    candidate.control(NAVIGATE_OFFSITE)
    candidate.control(RECORD_WINDOW_URL)
    attempted = candidate.detail_of("navigation-attempted")
    refused = candidate.detail_of("navigation-refused")
    run.record(
        "navigation: the guard refuses a scripted top-level navigation off the application origin",
        attempted,
        refused,
    )
    # A refused navigation leaves the window where it was — but so does
    # an allowed one that cannot be reached, which is why the refusal
    # above is what the guard is judged on. This is the second half:
    # having refused, it also did not move.
    reached = candidate.detail_of("window-url")
    run.assert_that(
        "navigation: the window is still on the application origin afterwards",
        reached is not None and reached.startswith(APPLICATION_ORIGIN),
        f"the window reached {reached!r} after {attempted!r}",
    )

    # (h) The other exfiltration door, which the guard does not cover
    # either. `location.href` above reaches the navigation-policy
    # delegate; a script-initiated `window.open` and a click on a link
    # carrying `target="_blank"` do not — WebKit routes both to the UI
    # delegate's create-web-view method, which `wry` answers with
    # nothing *only because no new-window handler is registered*. That
    # door is therefore held shut by the absence of a call rather than
    # by anything this stack decides, and a new-window handler — a
    # detached playback window being the obvious next candidate — would
    # open it while the guard, the policy, and every check above stayed
    # green. Probing the main window's URL cannot see it, because a new
    # window would not have moved the old one.
    #
    # The reach of the webview check below stops at a handler answering
    # `NewWindowResponse::Create`, which hands back a window Tauri
    # tracks. One answering `Allow` has `wry` build the `NSWindow` and
    # the `WKWebView` itself, and Tauri never learns of them, so they
    # appear in no list readable from here — a run with such a handler
    # registered passes every check in this file. What covers that
    # answer is `src-tauri/tests/new_window_handler.rs`, which asserts
    # the handler is not registered at all.
    candidate.control(OPEN_OFFSITE)
    time.sleep(SETTLE_SECONDS)
    candidate.control(RECORD_WEBVIEWS)
    candidate.control(RECORD_WINDOW_URL)
    opened = candidate.detail_of("new-window-attempted")
    run.record(
        "new window: `window.open` and `target=_blank` create no second webview",
        MAIN_WEBVIEW,
        candidate.last_detail_of("webview-labels"),
    )
    landed = candidate.last_detail_of("window-url")
    run.assert_that(
        "new window: the main window is still on the application origin afterwards",
        landed is not None and landed.startswith(APPLICATION_ORIGIN),
        f"the window reached {landed!r} after {opened!r}",
    )

    # (d) Idle quit exits cleanly.
    candidate.control(TRAY_QUIT)
    run.assert_that(
        "quit: the process exits",
        candidate.wait_for_exit(),
        f"{len(candidate.process_ids())} process(es) still running",
    )
    run.record("quit: processes registered under the bundle identifier", [], candidate.registered_process_ids())
    run.record("quit: the exit was the one the application asked for", True, "quit-accepted" in candidate.events())
    run.record("quit: the run loop reported its exit", True, "exited" in candidate.events())

    # (h) Disposable-root confinement.
    written = sorted(path.name for path in candidate.root.iterdir())
    run.record(
        "confinement: what the application wrote into the root it was given",
        [LIFECYCLE_RECORD],
        written,
    )
    run.record(
        "confinement: the process-coordination socket is removed on exit",
        False,
        SINGLE_INSTANCE_SOCKET.exists(),
    )
    hermeticity(candidate, run, untouched)
    webview_state(run)

    # What the bundle actually ships. A developer tool or a bundled
    # runtime reaching the application is the kind of thing that only
    # shows up if something looks.
    shipped = candidate.bundle / "Contents" / "MacOS"
    run.record(
        "bundle: executables shipped in the application",
        [EXECUTABLE],
        sorted(entry.name for entry in shipped.iterdir()),
    )
    run.record(
        "bundle: files matching a Node, Chromium, or Electron runtime",
        [],
        foreign_runtime_files(candidate.bundle),
    )
    run.record(
        "bundle: frameworks embedded rather than used from the system",
        [],
        embedded_frameworks(candidate.bundle),
    )
    run.record(
        "bundle: the WebView is the system WebKit",
        True,
        links_system_webkit(shipped / EXECUTABLE),
    )

    # The application menu's quit item carries the tray's own identity,
    # which is what routes it through the shared quit decision instead
    # of the platform's native `terminate:`. `muda` refuses to build a
    # menu off the main thread, so the installed menu cannot be read
    # from a unit test; the application reports what it installed.
    run.record(
        "quit: the menu item runs the same decision the tray item runs",
        "quit:quit",
        candidate.detail_of("menu-ready"),
    )

    # (h) What the Rust host can speak, structurally. Not "nothing":
    # the shipped application offers an in-app model download, so a
    # graph with no HTTP client is not a shape it can have, and
    # asserting one over a configuration nobody ships would read as a
    # guarantee while covering nothing. What is asserted instead is
    # that the denylisted crates it carries are exactly the approved
    # transport — so a gained crate and a lost one both fail.
    #
    # This covers the host graph and nothing else: the webview carries
    # the platform's own networking stack, so the policy and the
    # navigation guard checked above are what govern it.
    graph = host_dependency_graph()
    run.record(
        "egress surface: denylisted crates in the shipped host graph",
        sorted(APPROVED_TRANSPORT),
        sorted(NETWORK_DENYLIST & graph),
    )
    run.record(
        "no network: internet sockets held by the application, sampled from launch to exit",
        [],
        sockets.stop(),
    )

    # (h) The policy the bundle ships, read back out of it.
    policies = shipped_policies(shipped / EXECUTABLE)
    run.record("policy: the bundle carries exactly one content security policy", 1, len(policies))
    run.record(
        "policy: the policy the bundle carries is the expected one, directive for directive",
        EXPECTED_CSP,
        policies[0] if len(policies) == 1 else policies,
    )

    # The debug-only channel is compiled out of a release build.
    release = build_release_binary()
    candidate_strings = strings_in(candidate.bundle / "Contents" / "MacOS" / EXECUTABLE)
    release_strings = strings_in(release)
    run.record(
        "release: debug-only lifecycle strings present in the candidate",
        sorted(DEBUG_ONLY_STRINGS),
        sorted(name for name in DEBUG_ONLY_STRINGS if name in candidate_strings),
    )
    run.record(
        "release: debug-only lifecycle strings present in the release binary",
        [],
        sorted(name for name in DEBUG_ONLY_STRINGS if name in release_strings),
    )


def shorten(path: Path) -> str:
    try:
        return f"~/{path.relative_to(Path.home())}"
    except ValueError:
        return str(path)


def session_artifacts_under(directory: Path) -> list[str]:
    if not directory.is_dir():
        return []
    return sorted(
        str(path.relative_to(directory))
        for path in directory.rglob("*")
        if path.name in SESSION_ARTIFACTS
    )


def foreign_runtime_files(bundle: Path) -> list[str]:
    return sorted(
        str(path.relative_to(bundle))
        for path in bundle.rglob("*")
        if any(marker in path.name.lower() for marker in FOREIGN_RUNTIME_MARKERS)
    )


def embedded_frameworks(bundle: Path) -> list[str]:
    frameworks = bundle / "Contents" / "Frameworks"
    if not frameworks.is_dir():
        return []
    return sorted(entry.name for entry in frameworks.iterdir())


def links_system_webkit(binary: Path) -> bool:
    result = subprocess.run(
        ["otool", "-L", str(binary)], capture_output=True, text=True, check=True
    )
    return "/System/Library/Frameworks/WebKit.framework" in result.stdout


def strings_in(binary: Path) -> str:
    result = subprocess.run(
        ["strings", "-a", str(binary)], capture_output=True, text=True, check=True
    )
    return result.stdout



def setup(candidate: Candidate, run: Run) -> None:
    """Model acquisition, driven against a local fixture on the real app.

    What this establishes that the unit tests cannot: the properties
    hold in the shipped binary, driven through the same manager the
    interface drives, with real sockets that can be watched.

    How the no-network proof works here, and how it differs from the
    one `lifecycle` makes. Both scenarios build the same shape — the
    shipped one, default features, transport included — so neither can
    argue from the absence of an HTTP client, and `lifecycle`'s
    crate-graph check is an equality against the approved transport
    rather than an absence.

    What differs is what each run drives. `lifecycle` never asks for a
    model, so its socket sampler asserts the set of destinations is
    empty: the compiled-in client stays unused unless somebody asks it
    to act. This scenario does ask, against a fixture on loopback, so
    an empty set would mean the run had not exercised what it exists to
    exercise. Its socket leg therefore asserts every member of the set
    is the fixture, on loopback, on the fixture's own port — and
    asserts separately that the set is non-empty, so it cannot pass by
    vacuity. Beside it sits the exact comparison of the shipped content
    security policy against the built artifact, which is what governs
    the webview.
    """
    untouched = [
        ("the real configuration file", REAL_CONFIG, snapshot(REAL_CONFIG)),
        ("the default storage root", REAL_STORAGE_ROOT, snapshot(REAL_STORAGE_ROOT)),
    ]
    server = FixtureServer()
    server.start()
    models = candidate.models
    try:
        _setup_checks(candidate, run, server)
    finally:
        server.stop()

    # (h) Disposable-root confinement, and the models directory it
    # redirects. A run that reached the platform models directory would
    # have written half a gigabyte into the developer's own install.
    run.record(
        "confinement: nothing of this run reached the platform models directory",
        False,
        PLATFORM_MODELS.exists() and any(PLATFORM_MODELS.glob("*harness-fixture*")),
    )
    run.record(
        "confinement: everything the run installed is under the disposable models directory",
        True,
        models.is_dir() and models.is_relative_to(candidate.workspace),
    )
    hermeticity(candidate, run, untouched)
    webview_state(run)

    # The probe is a debug affordance, not a shipped one.
    release = build_release_binary()
    release_strings = strings_in(release)
    run.record(
        "release: the model-acquisition probe is compiled out of a release build",
        [],
        sorted(name for name in MODEL_PROBE_STRINGS if name in release_strings),
    )

    # (h) The policy the bundle ships, read back out of it. Carried over
    # from `lifecycle` deliberately: with a network client compiled in,
    # what the WebView itself may reach matters more, not less.
    policies = shipped_policies(candidate.bundle / "Contents" / "MacOS" / EXECUTABLE)
    run.record("policy: the bundle carries exactly one content security policy", 1, len(policies))
    run.record(
        "policy: the policy the bundle carries is the expected one, directive for directive",
        EXPECTED_CSP,
        policies[0] if len(policies) == 1 else policies,
    )


def _setup_checks(candidate: Candidate, run: Run, server: FixtureServer) -> None:
    """Everything that needs the application running and the fixture up."""
    candidate.launch()
    sockets = SocketSampler(candidate)
    sockets.start()
    opened = candidate.wait_for_control_socket()
    run.assert_that(
        "launch: the control channel opens",
        opened,
        f"no control socket appeared at {candidate.root / CONTROL_SOCKET}",
    )
    if not opened:
        run.record("no network: every destination the application reached", [], sockets.stop())
        return
    candidate.wait_for_event("window-shown")

    models = candidate.models

    # (a) Reading the offer opens nothing. This is the call a
    # confirmation prompt is built from, so it has to be safe to make
    # before the user has agreed to anything.
    candidate.control("probe-model-offer")
    run.assert_that(
        "offer: the catalog is readable without fetching anything",
        candidate.wait_for_event("probe-model-offer"),
        "the application recorded no `model-offer`",
    )
    # There is deliberately no "requests the fixture received while
    # reading the offer" check here. It would compare a counter that
    # has never been incremented against zero, at a point in the run
    # where nothing has yet been able to increment it — it could not
    # fail, and a check that cannot fail inflates the count while
    # establishing nothing. The zero-request property is asserted below
    # against the confirmation gate, where a control exists: the
    # counter is read again after a confirmed install has provably
    # moved it.

    # (a) An unconfirmed install reaches the source no further. The
    # digest handed back is not the one on offer, which is the shape a
    # caller that skipped the prompt would produce.
    candidate.control(
        f"probe-model-install {server.url()} {FIXTURE_ARTIFACT_BYTES} {server.digest} "
        f"{FIXTURE_DESTINATION} unconfirmed"
    )
    run.assert_that(
        "confirmation: an unconfirmed install is refused",
        candidate.wait_for_event("probe-model-install"),
        "the application recorded no outcome for the unconfirmed install",
    )
    run.record(
        "confirmation: the outcome of an unconfirmed install",
        "refused:model_confirmation_required",
        candidate.last_detail_of("probe-model-install"),
    )
    run.record(
        "confirmation: requests the fixture received before any confirmation",
        0,
        server.count(),
    )
    run.record(
        "confirmation: what an unconfirmed install left in the models directory",
        [],
        listing(models),
    )

    # (b) A free-space failure is decided before any request. The size
    # is larger than any volume will report free, so the preflight
    # rejects rather than the write.
    before_space = server.count()
    candidate.control(
        f"probe-model-install {server.url()} {2**62} {server.digest} {FIXTURE_DESTINATION} confirmed"
    )
    candidate.wait_for_event("probe-model-install", count=2)
    run.record(
        "free space: the outcome when the artifact does not fit",
        "failed:insufficient_space:promoted=false",
        candidate.last_detail_of("probe-model-install"),
    )
    run.record(
        "free space: requests the fixture received for an artifact that does not fit",
        before_space,
        server.count(),
    )
    run.record(
        "free space: what the rejected install left behind",
        [],
        listing(models),
    )

    # (b) A digest that does not describe what arrives promotes nothing.
    candidate.control(
        f"probe-model-install {server.url()} {FIXTURE_ARTIFACT_BYTES} {'0' * 64} "
        f"{FIXTURE_DESTINATION} confirmed"
    )
    candidate.wait_for_event("probe-model-install", count=3, timeout=60.0)
    run.record(
        "digest: the outcome when the artifact does not match",
        "failed:digest_mismatch:promoted=false",
        candidate.last_detail_of("probe-model-install"),
    )
    run.record(
        "digest: the destination after a digest failure",
        False,
        (models / FIXTURE_DESTINATION).exists(),
    )
    run.assert_that(
        "digest: the part that arrived is kept where the user can see it",
        bool(list(models.glob("*.partial"))),
        "a failed download deleted its own partial",
    )
    for leftover in models.glob("*.partial"):
        leftover.unlink()

    # (b) A cancellation promotes nothing either.
    candidate.control(
        f"probe-model-install {server.url()} {FIXTURE_ARTIFACT_BYTES} {server.digest} "
        f"{FIXTURE_DESTINATION} confirmed"
    )
    time.sleep(FIXTURE_SERVE_SECONDS / 3)
    candidate.control("probe-model-cancel")
    candidate.wait_for_event("probe-model-install", count=4, timeout=60.0)
    run.record(
        "cancellation: the outcome when the user stops it",
        "cancelled:promoted=false",
        candidate.last_detail_of("probe-model-install"),
    )
    run.record(
        "cancellation: the destination after a cancellation",
        False,
        (models / FIXTURE_DESTINATION).exists(),
    )
    run.assert_that(
        "cancellation: the part that arrived is kept rather than deleted",
        bool(list(models.glob("*.partial"))),
        "a cancelled download deleted its own partial",
    )
    for leftover in models.glob("*.partial"):
        leftover.unlink()

    # (a)(b) A confirmed install that verifies, watched while it runs.
    watcher = ModelsSampler(models, FIXTURE_DESTINATION, FIXTURE_ARTIFACT_BYTES)
    watcher.start()
    before_install = server.count()
    candidate.control(
        f"probe-model-install {server.url()} {FIXTURE_ARTIFACT_BYTES} {server.digest} "
        f"{FIXTURE_DESTINATION} confirmed"
    )
    candidate.wait_for_event("probe-model-install", count=5, timeout=60.0)
    short, saw_partial = watcher.stop()
    run.record(
        "install: the outcome of a confirmed install of the artifact on offer",
        "ready:promoted=true",
        candidate.last_detail_of("probe-model-install"),
    )
    run.record(
        "install: requests the fixture received for the confirmed install",
        before_install + 1,
        server.count(),
    )
    run.record(
        "atomic promotion: destination sizes seen while the artifact was still arriving",
        [],
        short,
    )
    run.assert_that(
        "atomic promotion: the sampler could see the download at all",
        saw_partial,
        "no `.partial` was ever observed, so the atomicity sample proves nothing",
    )
    run.record(
        "install: the installed artifact is exactly the bytes the fixture served",
        FIXTURE_ARTIFACT_BYTES,
        (models / FIXTURE_DESTINATION).stat().st_size
        if (models / FIXTURE_DESTINATION).is_file()
        else None,
    )
    run.record(
        "install: nothing unverified is left beside it",
        [FIXTURE_DESTINATION],
        listing(models),
    )

    # (b)(c) An artifact already installed and already valid is ready
    # without a request. The one that was just installed is the
    # pre-seeded one.
    before_existing = server.count()
    candidate.control(
        f"probe-model-install {server.url()} {FIXTURE_ARTIFACT_BYTES} {server.digest} "
        f"{FIXTURE_DESTINATION} confirmed"
    )
    candidate.wait_for_event("probe-model-install", count=6, timeout=60.0)
    run.record(
        "existing model: the outcome when the artifact is already installed",
        "ready:promoted=false",
        candidate.last_detail_of("probe-model-install"),
    )
    run.record(
        "existing model: requests the fixture received for an artifact already installed",
        before_existing,
        server.count(),
    )

    # (d) Quit cleanly, then judge the sockets over the whole run.
    candidate.control(TRAY_QUIT)
    run.assert_that(
        "quit: the process exits",
        candidate.wait_for_exit(),
        f"{len(candidate.process_ids())} process(es) still running",
    )

    observed = sockets.stop()
    run.assert_that(
        "no network: the application did open sockets, so the check below is not vacuous",
        bool(observed),
        "the socket sampler saw nothing at all, including the fixture it was meant to see",
    )
    run.record(
        "no network: every destination the application reached that was not the local fixture",
        [],
        sorted(line for line in observed if not reaches_only(line, server.port)),
    )


def listing(directory: Path) -> list[str]:
    """Every file directly under `directory`, or nothing when it is absent."""
    if not directory.is_dir():
        return []
    return sorted(entry.name for entry in directory.iterdir())


def reaches_only(line: str, port: int) -> bool:
    """Whether one `lsof` line reaches nothing but the fixture.

    `lsof -i -nP` prints COMMAND PID USER FD TYPE DEVICE SIZE/OFF NODE
    NAME, with an optional state in parentheses after it. The NAME is
    the ninth field and the only one that carries an address; taking the
    last field instead would read `(ESTABLISHED)` and find no address at
    all, which would make this pass everything.

    Every host named must be an explicit loopback address, and where the
    name has a remote half — the `->` form, which is a connection rather
    than a listening socket — its port must be the fixture's. A
    connection to some other service on this machine is as much a
    failure here as one to a public host.

    "Explicit" rules out `*`, which this used to accept. A wildcard is
    not a loopback address: `*:8080 (LISTEN)` is a socket bound to every
    interface on the machine, reachable from the network, and
    `*:52000->*:51234` names a peer this check cannot see. Accepting
    either made the scenario's only no-network leg pass on sockets it
    had not actually established anything about — and because this build
    carries a compiled-in HTTP client by design, that leg is the proof,
    not a corroboration of one.
    """
    fields = line.split()
    if len(fields) < 9:
        return False
    name = fields[8]
    local, _, remote = name.partition("->")
    for endpoint in (local, remote):
        if endpoint == "":
            continue
        if not is_loopback_host(endpoint.rsplit(":", 1)[0].strip("[]")):
            return False
    if remote == "":
        return True
    return remote.rsplit(":", 1)[-1] == str(port)


def is_loopback_host(host: str) -> bool:
    """Whether `host` is literally a loopback address.

    Parsed rather than matched against a list of spellings: `lsof -nP`
    prints numeric addresses, and the whole of `127.0.0.0/8` and `::1`
    are loopback, not just the two spellings anyone thinks to write
    down. A name — including `localhost` — is not an address and is
    refused; `-n` means one should never appear, and if one does, this
    check cannot say where it points.
    """
    try:
        return ipaddress.ip_address(host).is_loopback
    except ValueError:
        return False


# Scenarios are registered here rather than enumerated at each call
# site, so later work adds `recording` or `library` by adding one entry
# and its function.
SCENARIOS: dict[str, Callable[[Candidate, Run], None]] = {
    "lifecycle": lifecycle,
    "setup": setup,
}


# What each scenario's success line claims to have covered. Stated per
# scenario rather than once, because a summary that described the wrong
# run would be the most quietly misleading line this file prints.
SCENARIO_COVERAGE: dict[str, str] = {
    "lifecycle": (
        "launch, close, restore, recreation, second launch, quit, "
        "disposable-root confinement, the shipped content security policy, "
        "navigation, and egress"
    ),
    "setup": (
        "reading the catalog without fetching, the confirmation gate, "
        "free-space rejection, digest failure, cancellation, atomic promotion, "
        "existing-model preservation, disposable model and configuration roots, "
        "the shipped content security policy, and every socket the process opened"
    ),
}


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--hermetic",
        action="store_true",
        required=True,
        help="run against a disposable storage root and configuration, touching no real user data",
    )
    parser.add_argument(
        "--scenario",
        choices=sorted(SCENARIOS),
        required=True,
        help="which qualification scenario to run",
    )
    arguments = parser.parse_args()

    if sys.platform != "darwin":
        print("this harness qualifies the macOS application; nothing else is a target yet")
        return 1
    if candidates_already_running():
        print(
            "another copy of the application is already running; a qualification run "
            "must be the only owner of its bundle identifier"
        )
        return 1

    # Every scenario builds the shape the application ships: default
    # features, no additions and no subtractions. There is deliberately
    # no per-scenario feature selection any more — `setup` used to name
    # `model-download` explicitly, back when the host's default feature
    # set did not include it, and the effect was that the only artifact
    # ever built with a transport was this candidate. Qualifying a
    # binary nobody installs proves nothing about the one they do.
    bundle = build_candidate()
    # A Unix socket path cannot exceed 104 bytes on macOS, and the
    # platform's own temporary directory is already most of that, so the
    # disposable root is created directly under `/tmp` with a short
    # prefix. The control socket lives inside it.
    workspace = Path(tempfile.mkdtemp(prefix="scrybe-q-", dir="/tmp"))
    candidate = Candidate(bundle, workspace)
    run = Run()
    try:
        SCENARIOS[arguments.scenario](candidate, run)
    finally:
        candidate.terminate()
        shutil.rmtree(workspace, ignore_errors=True)

    print()
    if run.failures:
        print(f"desktop {arguments.scenario} qualification FAILED — {len(run.failures)} checks:")
        for entry in run.failures:
            print(f"  {entry.check}")
            print(f"    expected: {entry.expected}")
            print(f"    observed: {entry.observed}")
        return 1
    print(
        f"desktop {arguments.scenario} qualification: ok — {len(run.entries)} checks "
        f"across {SCENARIO_COVERAGE[arguments.scenario]}"
    )
    return 0


def candidates_already_running() -> bool:
    result = subprocess.run(
        ["pgrep", "-f", PROCESS_PATTERN],
        capture_output=True,
        text=True,
        check=False,
    )
    return bool(result.stdout.split())


if __name__ == "__main__":
    sys.exit(main())
