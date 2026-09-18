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

Seven scenarios, one default-feature application shape. Qualification
disables updater-archive emission because it exercises the application rather
than publishing it. The debug profile exposes the private control channel; the
production code path and default feature selection are unchanged.

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

`model-download-live` is the release-only network proof. It reads the checked-in
production catalog and downloads its pinned Hugging Face artifact into a
disposable model root. It is intentionally excluded from routine CI: the
artifact is about 488 MB and depends on a third-party service.

Run locally:

    python3 scripts/qualify-desktop-app.py --hermetic --scenario lifecycle
    python3 scripts/qualify-desktop-app.py --hermetic --scenario setup
    python3 scripts/qualify-desktop-app.py --hermetic --scenario model-download-live
    python3 scripts/qualify-desktop-app.py --hermetic --scenario library
    python3 scripts/qualify-desktop-app.py --hermetic --scenario recording
    python3 scripts/qualify-desktop-app.py --hermetic --scenario installed
    python3 scripts/qualify-desktop-app.py --hermetic --scenario retention

Exit status 0 means every check held. Exit status 1 means at least one
did not; each failing check prints what was expected and what was
observed.

The ``installed`` scenario defaults to the community trust profile: a stable
project-controlled certificate, expected Gatekeeper rejection, and no
notarization ticket. Pass ``--trust-profile apple-trusted`` to retain the
Developer ID, notarization, and Gatekeeper-accepted gate. Its native
accessibility leg still requires a human-granted TCC Accessibility permission;
the harness never acquires that broad terminal permission for itself.
"""

from __future__ import annotations

import argparse
import datetime as dt
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
import tomllib
import threading
import time
from collections.abc import Callable
from dataclasses import dataclass, field
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent
DESKTOP = REPO_ROOT / "scrybe-desktop"
HOST = DESKTOP / "src-tauri"
COMMUNITY_POLICY = REPO_ROOT / "packaging" / "macos-app" / "community-release.json"
MODEL_CATALOG = REPO_ROOT / "scrybe-application" / "models.toml"

BUNDLE_IDENTIFIER = "dev.scrybe.desktop"
BUNDLE_NAME = "Scrybe.app"
EXECUTABLE = "scrybe-desktop"

# The bundle this checkout's own build produces. `build_candidate` and
# `PROCESS_PATTERN` both need this exact path, and they must agree: a
# process match built from anything looser than the candidate this run
# itself built is a match against every other checkout's candidate too.
CANDIDATE_BUNDLE = HOST / "target" / "debug" / "bundle" / "macos" / BUNDLE_NAME

# What `pgrep -f` is given, rather than the path itself.
#
# `pgrep -f` matches against whole command lines, and a `pgrep`
# invocation's own command line contains the pattern it was given. It
# excludes itself, but not another `pgrep` running the same query
# concurrently — which the socket sampler does, from its own thread, for
# the whole run. Bracketing the bundle name's first character makes
# that part of the pattern a regular expression that matches the
# application's command line and not the command line of the pgrep
# invocation itself, which carries the brackets literally.
#
# The rest of the pattern is anchored to this checkout's own absolute
# build path, not just the bundle's relative shape. A pattern built
# from the bare "Scrybe.app/Contents/MacOS/scrybe-desktop" suffix
# matches the candidate built by every worktree of this repository, and
# `terminate` sends `kill -9` to everything `pgrep` finds — so two
# concurrent checkouts each running their own qualification would
# `kill -9` each other's candidate mid-scenario. `re.escape` covers
# whatever this checkout happens to be named or nested under; it runs
# on the parent directory only, so the bracket trick above still has an
# unescaped character to work with.
PROCESS_PATTERN = (
    f"{re.escape(str(CANDIDATE_BUNDLE.parent))}/"
    f"[{BUNDLE_NAME[0]}]{re.escape(BUNDLE_NAME[1:])}"
    f"/Contents/MacOS/{re.escape(EXECUTABLE)}"
)

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

# The playback probe the `library` scenario drives, checked the same way
# and for the same reason.
PLAYBACK_PROBE_STRINGS = ["probe-playback"]

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
        "reqwest",
        "hyper",
        "hyper-util",
        "h2",
        "h3",
        "ureq",
        "attohttpc",
        "isahc",
        "surf",
        "tonic",
        "tokio-tungstenite",
        "tungstenite",
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
        "trust-dns-resolver",
        "trust-dns-proto",
        "hickory-resolver",
        "hickory-proto",
        "quinn",
        "quinn-proto",
        "quinn-udp",
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
#
# `media-src` is what governs the source an `<audio>` element may load,
# and it does not inherit from `default-src` for a custom scheme, so the
# scheme has to be named here or the player is blocked by the webview
# with nothing in this file to show for it. `connect-src` is the
# directive that sounds like it should govern this and does not: it
# covers fetch, XHR, WebSocket, and beacon, none of which is how a media
# element loads. Admitting the scheme rather than an origin is
# deliberate — a custom scheme has no meaningful host, and what may be
# reached on it is decided in `src/playback.rs`, which parses the
# session identity and opens `playback.opus` or nothing.
#
# Read this next part before changing the string above. The check below
# compares the policy in the built artifact against this constant, so it
# passes for *any* string written in both places. Naming the scheme
# under the wrong directive and updating this to match reports success
# while playback stays blocked at runtime — which is not hypothetical:
# admitting `scrybe-audio:` under `connect-src` instead passes every
# string comparison in this file and serves nothing. Only driving the
# real webview at a real `scrybe-audio://` URL and observing what the
# protocol handler served tells the two apart, which is what the
# `probe-playback` verb exists for.
EXPECTED_CSP = (
    "default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; "
    "img-src 'self' data:; media-src 'self' scrybe-audio:; "
    "connect-src 'self' ipc: http://ipc.localhost; "
    "object-src 'none'; base-uri 'none'; frame-src 'none'; form-action 'none'"
)

# The origin the bundled application is served from. A window URL with
# any other origin after the navigation probe means the guard let it go.
APPLICATION_ORIGIN = "tauri://localhost"

# The tray's own menu-item identities, which the control channel accepts
# so a run drives the same dispatch the platform drives.
TRAY_OPEN = "open"
TRAY_QUIT = "quit"
TRAY_RECORD = "record"
TRAY_STOP = "stop"

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
            Evidence(
                check, "holds", detail if condition else f"FAILED: {detail}", condition
            )
        )

    @property
    def failures(self) -> list[Evidence]:
        return [entry for entry in self.entries if not entry.ok]


def build_candidate(scenario: str, trust_profile: str) -> Path:
    """Build the exact default-feature application shape each scenario drives.

    Qualification does not publish an updater archive, so it disables that
    release-only side effect explicitly. The installed scenario signs with the
    selected trust profile; every other debug scenario remains unsigned.
    """
    environment = os.environ.copy()
    environment.pop("APPLE_SIGNING_IDENTITY", None)
    environment.pop("TAURI_SIGNING_PRIVATE_KEY", None)
    environment.pop("TAURI_SIGNING_PRIVATE_KEY_PASSWORD", None)
    if scenario == "installed":
        if trust_profile == "community":
            policy = json.loads(COMMUNITY_POLICY.read_text())
            environment["APPLE_SIGNING_IDENTITY"] = str(policy["identity"])
        else:
            identity = os.environ.get("APPLE_SIGNING_IDENTITY")
            if not identity:
                raise RuntimeError(
                    "APPLE_SIGNING_IDENTITY is required for the apple-trusted profile"
                )
            environment["APPLE_SIGNING_IDENTITY"] = identity

    subprocess.run(
        ["pnpm", "--dir", str(DESKTOP), "install", "--frozen-lockfile"],
        cwd=REPO_ROOT,
        check=True,
    )
    subprocess.run(
        [
            "pnpm",
            "--dir",
            str(DESKTOP),
            "exec",
            "tauri",
            "build",
            "--debug",
            "--bundles",
            "app",
            "--config",
            json.dumps({"bundle": {"createUpdaterArtifacts": False}}),
        ],
        cwd=REPO_ROOT,
        check=True,
        env=environment,
    )
    bundle = CANDIDATE_BUNDLE
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
            "cargo",
            "build",
            "--manifest-path",
            str(HOST / "Cargo.toml"),
            "--release",
            "--bin",
            EXECUTABLE,
            "--locked",
        ],
        cwd=REPO_ROOT,
        check=True,
    )
    return HOST / "target" / "release" / EXECUTABLE


class Candidate:
    """The running application, and the facts observable about it."""

    def __init__(self, bundle: Path, workspace: Path, trust_profile: str) -> None:
        self.bundle = bundle
        self.workspace = workspace
        self.trust_profile = trust_profile
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

    def registered_bundle_paths(self) -> list[str]:
        """The bundle each running copy was launched from.

        Every other scenario launches a bundle by path and then reasons
        about "the application" without ever checking that the process it
        is driving came out of that bundle. On a machine with more than
        one copy registered — a build tree, an install root, a real
        install — that is an assumption, and the `installed` scenario is
        the one where it stops being safe to make.
        """
        script = (
            'ObjC.import("AppKit");'
            f'const a = $.NSRunningApplication.runningApplicationsWithBundleIdentifier("{BUNDLE_IDENTIFIER}");'
            "JSON.stringify(Array.from({length: a.count}, (_, i) => {"
            "const u = a.objectAtIndex(i).bundleURL;"
            "return u.isNil() ? null : ObjC.unwrap(u.path);"
            "}))"
        )
        result = subprocess.run(
            ["osascript", "-l", "JavaScript", "-e", script],
            capture_output=True,
            text=True,
            check=True,
        )
        return json.loads(result.stdout.strip() or "[]")

    def install_into(self, root: Path) -> Path:
        """Copies the bundle out of the build tree and drives that copy.

        An installed application is not the directory `tauri build` left
        in `target/`: it has been moved, its path no longer sits under a
        build directory, and nothing about it can depend on the tree it
        was produced in. Copying and re-pointing is what makes the rest
        of this scenario a statement about an installed artifact.
        """
        installed = root / self.bundle.name
        shutil.copytree(self.bundle, installed, symlinks=True)
        self.bundle = installed
        return installed

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
            found.extend(
                line for line in result.stdout.splitlines()[1:] if line.strip()
            )
        return found

    # -- the application's own record --------------------------------

    def records(self) -> list[dict]:
        path = self.root / LIFECYCLE_RECORD
        if not path.exists():
            return []
        return [
            json.loads(line) for line in path.read_text().splitlines() if line.strip()
        ]

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
        subprocess.run(["open", "-n", str(self.bundle)], env=environment, check=True)

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
                        self.wfile.write(
                            outer.body[start : start + FIXTURE_CHUNK_BYTES]
                        )
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
            if (
                any(self._directory.glob("*.partial"))
                if self._directory.is_dir()
                else False
            ):
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


def launch_services_resolution() -> str | None:
    """The bundle Launch Services hands a launch of this identifier.

    What a Dock icon, a Spotlight result, and `open -b` all resolve to.
    Read rather than set: making an installed copy win this resolution
    needs either a real install location or the deregistration of every
    other copy, and both are changes to the machine this harness
    deliberately does not make. Reading it turns an assumption into a
    recorded fact, and a dangling registration — an entry pointing at a
    bundle that no longer exists, which is what a removed install root
    leaves behind — into a failure rather than a surprise.
    """
    script = (
        'ObjC.import("AppKit");'
        "const u = $.NSWorkspace.sharedWorkspace"
        f'.URLForApplicationWithBundleIdentifier("{BUNDLE_IDENTIFIER}");'
        'u.isNil() ? "" : ObjC.unwrap(u.path)'
    )
    result = subprocess.run(
        ["osascript", "-l", "JavaScript", "-e", script],
        capture_output=True,
        text=True,
        check=False,
    )
    resolved = result.stdout.strip()
    return resolved or None


def accessibility_grant() -> bool:
    """Whether this harness may read another process's accessibility tree.

    `AXIsProcessTrusted` answers for the *calling* process, which here is
    `osascript` running under whatever terminal started the run. The
    grant is a human checkpoint in System Settings, and on a machine
    without it there is no supported way to read the accessible name of a
    native control in another process — which is why the native
    accessibility leg below stops rather than skipping.
    """
    script = (
        'ObjC.import("ApplicationServices");$.AXIsProcessTrusted() ? "true" : "false"'
    )
    result = subprocess.run(
        ["osascript", "-l", "JavaScript", "-e", script],
        capture_output=True,
        text=True,
        check=False,
    )
    return result.stdout.strip() == "true"


def signed_artifact_verdict(bundle: Path, trust_profile: str) -> tuple[int, str]:
    """Run the selected distribution-trust contract against ``bundle``.

    The community checker requires the stable project certificate while
    explicitly expecting Gatekeeper rejection and no notarization ticket.
    The Apple-trusted checker preserves the stronger Developer ID, notarization,
    and Gatekeeper-accepted contract for a future paid release profile.
    """
    if trust_profile == "community":
        command = [
            sys.executable,
            str(REPO_ROOT / "scripts" / "check-community-artifact.py"),
            "--bundle",
            str(bundle),
            "--policy",
            str(COMMUNITY_POLICY),
        ]
    else:
        command = [
            sys.executable,
            str(REPO_ROOT / "scripts" / "check-signed-artifact.py"),
            "--bundle",
            str(bundle),
            "--assert",
            "signature",
            "--assert",
            "gatekeeper",
        ]
    result = subprocess.run(
        command,
        capture_output=True,
        text=True,
        check=False,
    )
    return result.returncode, f"{result.stdout}{result.stderr}".strip()


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
            "cargo",
            "tree",
            "--manifest-path",
            str(HOST / "Cargo.toml"),
            "--edges",
            "normal",
            "--prefix",
            "none",
            "--format",
            "{p}",
        ],
        capture_output=True,
        text=True,
        check=True,
    )
    return {
        match.group(1)
        for match in (
            re.match(r"^([A-Za-z0-9_.-]+) v", line.strip())
            for line in result.stdout.splitlines()
        )
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


def hermeticity(
    candidate: Candidate, run: Run, untouched: list[tuple[str, Path, object]]
) -> None:
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

    run.record(
        "launch: processes running the executable", 1, len(candidate.process_ids())
    )
    run.record(
        "launch: processes registered under the bundle identifier",
        1,
        len(candidate.registered_process_ids()),
    )
    run.record("launch: tray items created", 1, candidate.events().count("tray-ready"))
    run.record("launch: windows shown", 1, candidate.events().count("window-shown"))
    run.record(
        "launch: the tray offers a recording and refuses a stop with nothing running",
        "record:enabled,stop:disabled,open:enabled,quit:enabled",
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
    run.record(
        "close: still registered under the bundle identifier",
        1,
        len(candidate.registered_process_ids()),
    )

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
        candidate.wait_for_event("implicit-exit-prevented")
        and bool(candidate.process_ids()),
        "the process exited when its last window was destroyed",
    )
    candidate.control(TRAY_OPEN)
    run.assert_that(
        "tray open: a destroyed window is rebuilt",
        candidate.wait_for_event("window-recreated"),
        "the application recorded no `window-recreated`",
    )
    run.record(
        "window recreation: processes still running", 1, len(candidate.process_ids())
    )

    # (c) A second launch activates the first process.
    candidate.launch()
    run.assert_that(
        "second launch: the existing process is activated",
        candidate.wait_for_event("second-launch-activated"),
        "the application recorded no `second-launch-activated`",
    )
    time.sleep(SETTLE_SECONDS)
    run.record(
        "second launch: processes running the executable",
        1,
        len(candidate.process_ids()),
    )
    run.record(
        "second launch: processes registered under the bundle identifier",
        1,
        len(candidate.registered_process_ids()),
    )
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
    run.record(
        "quit: processes registered under the bundle identifier",
        [],
        candidate.registered_process_ids(),
    )
    run.record(
        "quit: the exit was the one the application asked for",
        True,
        "quit-accepted" in candidate.events(),
    )
    run.record(
        "quit: the run loop reported its exit", True, "exited" in candidate.events()
    )

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
    run.record(
        "policy: the bundle carries exactly one content security policy",
        1,
        len(policies),
    )
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
    run.record(
        "policy: the bundle carries exactly one content security policy",
        1,
        len(policies),
    )
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
        run.record(
            "no network: every destination the application reached", [], sockets.stop()
        )
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


# -- the library scenario ------------------------------------------

# The sessions a library run seeds, and what each one is for.
#
# Fixture audio, not a decodable Opus stream. Nothing in the path under
# test decodes anything: what is asserted is which bytes the protocol
# handler served, and a known repeating pattern makes an off-by-one in
# the range arithmetic visible where a real recording would not.
PLAYABLE = "2026-04-29-1430-acme-01HXYZ"
NO_PLAYBACK = "2026-04-28-0900-onevoice-01AAAAA"
# `audio.opus` present, no `meta.toml`: `classify()` (in
# scrybe-application/src/sessions/scan.rs) reads this as Repairable,
# not Unfinished, before it ever reaches the Unfinished branches —
# proven against this exact fixture shape by
# `test_audio_without_metadata_is_repairable_because_metadata_can_be_reconstructed`.
REPAIRABLE = "2026-04-27-1100-abandoned-01BBBBB"

# The retention run's fixtures. `SWEPT` is dated far enough back that
# any window this application ships has passed; `KEPT` is dated at the
# run itself, so one launch has to reach opposite answers about two
# folders sitting side by side.
SWEPT = "2026-01-02-0900-longgone-01CCCCC"
KEPT = "2026-04-29-1430-justnow-01DDDDD"
BY_HAND = "2026-03-03-1200-notours-01EEEEE"
# No `audio.opus` and no `journal/`: `classify()` reaches Unfinished
# here only because a surviving `transcript.md` is evidence something
# was recorded — proven against this exact fixture shape by
# `test_a_surviving_transcript_without_audio_or_journal_is_unfinished`.
UNFINISHED = "2026-04-26-0800-orphaned-01CCCCC"
PLAYBACK_BYTES = bytes(range(256)) * 16

# Paths on the scheme that the player never builds, and that a run must
# see refused against a tree where the thing each one aims at is really
# there. A refusal against a missing file would prove nothing.
FORBIDDEN_PATHS = [
    f"/{PLAYABLE}/transcript.md",
    f"/{PLAYABLE}/notes.md",
    f"/{PLAYABLE}/audio.opus",
    f"/{PLAYABLE}/meta.toml",
    "/../../etc/passwd/playback",
    "/~/playback",
]

# What a successful media response looks like. WebKit asks for metadata
# first and the representation after, so a run sees several.
SERVED_STATUSES = (200, 206)


def session_meta(title: str, session_id: str) -> str:
    return (
        f'session_id = "{session_id}"\n'
        f'title = "{title}"\n'
        'started_at = "2026-04-29T14:30:00Z"\n'
        'ended_at = "2026-04-29T15:00:00Z"\n'
        "duration_secs = 1800\n"
    )


def seed_library(candidate: Candidate) -> None:
    """Writes the sessions a library run reads.

    Five of them, because the interesting answers are the ones that
    differ: one complete session with playback audio, one complete
    session with audio and no playback artifact — which is what a mono
    capture actually leaves behind — one that is repairable because its
    audio survived but its metadata never got written, one that never
    finished recording at all — no audio, no journal, only a durable
    transcript — and a transcript long enough that a view reading it
    whole would be visible.
    """
    playable = candidate.root / PLAYABLE
    playable.mkdir(parents=True)
    (playable / "meta.toml").write_text(session_meta("Acme sync", "01HXYZ"))
    (playable / "audio.opus").write_bytes(b"")
    (playable / "playback.opus").write_bytes(PLAYBACK_BYTES)
    (playable / "notes.md").write_text("## TL;DR\n- covered widgets\n")
    (playable / "transcript.md").write_text(
        "# Acme sync\n*2026-04-29 14:30*\n\n"
        + "".join(
            f"[00:00:{line % 60:02}] Speaker: line {line}\n" for line in range(4000)
        )
    )

    mono = candidate.root / NO_PLAYBACK
    mono.mkdir(parents=True)
    (mono / "meta.toml").write_text(session_meta("One voice", "01AAAAA"))
    (mono / "audio.opus").write_bytes(b"")
    (mono / "transcript.md").write_text("# One voice\nhello\n")

    repairable = candidate.root / REPAIRABLE
    repairable.mkdir(parents=True)
    (repairable / "audio.opus").write_bytes(b"")
    # No `meta.toml`: `classify()` never reaches the Unfinished
    # branches for this one. The artifact is there and the session is
    # not complete either way — refusing it is a decision about the
    # session, not about the file.
    (repairable / "playback.opus").write_bytes(PLAYBACK_BYTES)

    orphaned = candidate.root / UNFINISHED
    orphaned.mkdir(parents=True)
    # No `audio.opus`, no `journal/`: `classify()` lands on Unfinished
    # only because the transcript below is durable evidence something
    # was recorded. `playback.opus` is present anyway, for the same
    # reason as the repairable fixture above: refusing this one is a
    # decision about the session, not about the file.
    (orphaned / "transcript.md").write_text("# orphaned\nnever got to notes\n")
    (orphaned / "playback.opus").write_bytes(PLAYBACK_BYTES)


def served(candidate: Candidate) -> list[tuple[str, int, int]]:
    """Every response the protocol handler recorded, as `(path, status, bytes)`."""
    found = []
    for record in candidate.records():
        if record["event"] != "playback-served":
            continue
        detail = record.get("detail", "")
        path, status, size = detail.rsplit(" ", 2)
        found.append((path, int(status), int(size)))
    return found


def drive(candidate: Candidate, run: Run, path: str) -> list[tuple[str, int, int]]:
    """Asks the webview for `path`, and returns what the handler served.

    A run that sees nothing here cannot distinguish a handler that
    refused from a policy that never let the request out of the webview,
    so the empty case is reported as its own failure rather than folded
    into the status assertion below.
    """
    before = len(served(candidate))
    candidate.control(f"{playback_probe_verb()} {path}")
    candidate.wait_for_event("playback-served", count=before + 1, timeout=15.0)
    # One media load is several responses: the player asks for metadata
    # first and the representation after. Waiting for the first and
    # moving on would leave the rest of them counted against whatever
    # the next probe asked for.
    time.sleep(SETTLE_SECONDS)
    responses = served(candidate)[before:]
    run.assert_that(
        f"playback: the webview reached the scheme for {path}",
        bool(responses),
        "the policy admitted no media load at all",
    )
    return responses


def playback_probe_verb() -> str:
    return PLAYBACK_PROBE_STRINGS[0]


def recording_transitions(candidate: Candidate) -> list[str]:
    """Every recording transition the application recorded, in order.

    The detail is `<from>-><to>` with the accepting surface appended
    when there was one, all of them enumerated constants. Read rather
    than inferred from artifacts, because the order two stop sources
    were resolved in is not something a session folder can show.
    """
    return [
        record["detail"]
        for record in candidate.records()
        if record.get("event") == "recording-transition" and record.get("detail")
    ]


def session_folders(root: Path) -> list[str]:
    """Every session folder directly under `root`."""
    if not root.is_dir():
        return []
    return sorted(entry.name for entry in root.iterdir() if entry.is_dir())


def recording(candidate: Candidate, run: Run) -> None:
    """One recording, driven end to end on the built application.

    What this establishes that the unit tests cannot. Every stop source
    is a platform event — a menu item, an accelerator, a signal — and
    the dispatch behind each is reachable in Rust, but *that the built
    application wires them to the dispatch at all* is not. A tray item
    built disabled, a command absent from the capability file, a
    permission the generated manifest does not carry: each of those
    leaves every unit test passing and the application inert. Only
    driving the shipped bundle's own control surface shows the
    difference.

    How much of this is proven, and how much is only read. Six of these
    checks have been made to fail by mutating the thing each claims to
    protect: the tray's record arm wired to nothing, the stop-source
    attribution dropped from the transition detail, and the two
    independent double-start guards bypassed together — which is the
    interesting one, because bypassing either alone leaves all of them
    passing, and only bypassing both writes two session folders from one
    click and trips `stop: exactly one session was written`. The rest
    were judged real by reading. None of them restates a constant this
    file defines, which is the shape that produces a check incapable of
    failing, and three releases running have shipped one of those. The
    two worth mutating first, because they encode an ordering or a
    clearing rather than an existence, are `record: prepared then began
    capturing` and `stop: settled to idle carrying nothing`.

    The recording it drives is real. The candidate's configuration
    selects the synthetic source, which is the one this host can open
    with no device and no permission grant, and the stub providers,
    which reach no network. So the session folder it leaves is a real
    session folder written by the real pipeline, and every artifact
    asserted below was produced rather than seeded.
    """
    untouched = [
        ("the real configuration file", REAL_CONFIG, snapshot(REAL_CONFIG)),
        ("the default storage root", REAL_STORAGE_ROOT, snapshot(REAL_STORAGE_ROOT)),
    ]
    _recording_checks(candidate, run)
    hermeticity(candidate, run, untouched)


def _recording_checks(candidate: Candidate, run: Run) -> None:
    """Everything that needs the application running."""
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
        run.record(
            "no network: every destination the application reached", [], sockets.stop()
        )
        return

    run.record(
        "launch: nothing has been recorded yet",
        [],
        session_folders(candidate.root),
    )

    # (a) The tray starts a recording. This is the dispatch a menu click
    # reaches, so a `Record now` left disabled or wired to nothing fails
    # here and nowhere else.
    candidate.control(TRAY_RECORD)
    started = candidate.wait_for_event("recording-transition", count=2, timeout=30.0)
    run.assert_that(
        "record: the tray started a recording",
        started,
        f"transitions observed: {recording_transitions(candidate)}",
    )
    if not started:
        run.record(
            "no network: every destination the application reached", [], sockets.stop()
        )
        return
    run.record(
        "record: the recording prepared and then began capturing",
        ["idle->preparing", "preparing->recording"],
        recording_transitions(candidate)[:2],
    )

    # (b) A second recording while one is running is refused. The
    # controller decides this, but a host that dispatched a start
    # without asking it would produce a second transition here.
    candidate.control(TRAY_RECORD)
    run.record(
        "record: a second recording while one is running adds no transition",
        ["idle->preparing", "preparing->recording"],
        recording_transitions(candidate),
    )

    # (c) Two stop sources, back to back, with no wait between them.
    # Exactly one may be accepted and exactly one finalization may
    # begin. This is the convergence claim, driven on the application
    # rather than on the controller.
    candidate.control(TRAY_STOP)
    candidate.control(TRAY_STOP)
    saved = candidate.wait_for_event("recording-transition", count=4, timeout=120.0)
    transitions = recording_transitions(candidate)
    run.assert_that(
        "stop: the recording finalized",
        saved,
        f"transitions observed: {transitions}",
    )
    run.record(
        "stop: exactly one stop was accepted, from the tray",
        1,
        len([entry for entry in transitions if entry.startswith("recording->saving,")]),
    )
    run.record(
        "stop: the accepted stop is attributed to the surface that asked",
        ["recording->saving,tray"],
        [entry for entry in transitions if entry.startswith("recording->saving")],
    )
    # The accepted surface stays on the snapshot until the recording
    # settles, so this transition carries it too — which is the point:
    # a reader told "stopped from the tray" is still told that while it
    # saves, rather than losing the attribution at the boundary.
    run.record(
        "stop: exactly one finalization completed, still attributed to the tray",
        ["saving->completed,tray"],
        [entry for entry in transitions if entry.startswith("saving->completed")],
    )
    # And `acknowledge` clears it, because the attempt is over: an idle
    # controller carrying the surface that stopped the last recording
    # would describe a recording that is no longer running.
    run.record(
        "stop: the recording settled back to idle, carrying nothing from the attempt",
        ["completed->idle"],
        [entry for entry in transitions if entry.startswith("completed->idle")],
    )

    # (d) What the recording left. Every one of these was written by the
    # pipeline during this run; the disposable root held nothing before
    # (a).
    folders = session_folders(candidate.root)
    run.record("stop: exactly one session was written", 1, len(folders))
    if len(folders) == 1:
        folder = candidate.root / folders[0]
        run.record(
            "stop: the session holds everything a reader opens",
            [],
            sorted(
                name
                for name in ("transcript.md", "notes.md", "meta.toml", "audio.opus")
                if not (folder / name).is_file()
            ),
        )
        run.record(
            "stop: nothing unfinished was left behind",
            [],
            sorted(
                entry.name
                for entry in folder.iterdir()
                if entry.name.endswith(".partial") or entry.name == "journal"
            ),
        )

    # (e) A quit with nothing in flight leaves immediately.
    candidate.control(TRAY_QUIT)
    exited = candidate.wait_for_exit()
    run.assert_that(
        "quit: the process exits once nothing is in flight",
        exited,
        "the application was still running after quit",
    )
    run.record(
        "quit: the exit was the one the application asked for",
        True,
        "quit-accepted" in candidate.events(),
    )
    run.record(
        "quit: nothing was deferred, because nothing was recording",
        0,
        candidate.events().count("quit-deferred"),
    )

    run.record(
        "no network: every destination the application reached", [], sockets.stop()
    )


def library(candidate: Candidate, run: Run) -> None:
    """Reading, playing, and refusing, driven on the real application.

    What this establishes that the unit tests cannot. The content
    security policy decides whether a media element may load a
    `scrybe-audio://` URL at all, and a policy naming the scheme under
    the wrong directive blocks the load before the handler is asked.
    Nothing in Rust can see that, and neither can the policy comparison
    a few lines below — it reads a constant this file also declares, so
    it passes for whatever string is written in both places. Only
    driving the real webview at a real URL and reading back what the
    handler served tells the two apart.

    Every refusal below aims at something that is really in the fixture
    tree: a transcript, notes, merged audio, metadata, and a second
    session's folder. A refusal earned by a missing file would say
    nothing about confinement.
    """
    untouched = [
        ("the real configuration file", REAL_CONFIG, snapshot(REAL_CONFIG)),
        ("the default storage root", REAL_STORAGE_ROOT, snapshot(REAL_STORAGE_ROOT)),
    ]
    seed_library(candidate)
    _library_checks(candidate, run)
    hermeticity(candidate, run, untouched)
    webview_state(run)

    # The probe is a debug affordance, not a shipped one.
    release_strings = strings_in(build_release_binary())
    run.record(
        "release: the playback probe is compiled out of a release build",
        [],
        sorted(name for name in PLAYBACK_PROBE_STRINGS if name in release_strings),
    )

    # The policy the bundle ships, read back out of it. The behavioural
    # checks above are what give this one meaning.
    policies = shipped_policies(candidate.bundle / "Contents" / "MacOS" / EXECUTABLE)
    run.record(
        "policy: the bundle carries exactly one content security policy",
        1,
        len(policies),
    )
    run.record(
        "policy: the policy the bundle carries is the expected one, directive for directive",
        EXPECTED_CSP,
        policies[0] if len(policies) == 1 else policies,
    )


def _library_checks(candidate: Candidate, run: Run) -> None:
    """Everything that needs the application running."""
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
        run.record(
            "no network: every destination the application reached", [], sockets.stop()
        )
        return

    # (a) The one URL the player builds, for the one session that has
    # something to play.
    responses = drive(candidate, run, f"/{PLAYABLE}/playback")
    run.record(
        "playback: every response to the player was a success",
        [],
        sorted({status for _, status, _ in responses} - set(SERVED_STATUSES)),
    )
    run.record(
        "playback: at least as many bytes were served as the artifact holds",
        True,
        sum(size for _, _, size in responses) >= len(PLAYBACK_BYTES),
    )

    # (b) A completed session whose capture was mono. The artifact is
    # genuinely absent, which is the case that used to be reported as
    # playable because availability was read off `audio.opus`.
    refusal = drive(candidate, run, f"/{NO_PLAYBACK}/playback")
    run.record(
        "playback: a session with no playback artifact is refused as absent",
        [404],
        sorted({status for _, status, _ in refusal}),
    )

    # (c) A session whose audio survived but whose metadata never got
    # written. `classify()` reads this as Repairable, not Unfinished,
    # and `playable()` refuses it the same way it refuses Unfinished:
    # neither state is Complete.
    refusal = drive(candidate, run, f"/{REPAIRABLE}/playback")
    run.record(
        "playback: a repairable session (audio without metadata) is refused as not finished",
        [409],
        sorted({status for _, status, _ in refusal}),
    )

    # (d) A session that never finished recording at all: no audio, no
    # journal, only a surviving transcript. This is the state (c) only
    # sounded like it was covering.
    refusal = drive(candidate, run, f"/{UNFINISHED}/playback")
    run.record(
        "playback: a session with no audio and no journal is refused as not finished",
        [409],
        sorted({status for _, status, _ in refusal}),
    )

    # (e) Every path the player never builds, each aimed at something
    # that really exists.
    for path in FORBIDDEN_PATHS:
        refusal = drive(candidate, run, path)
        run.record(
            f"confinement: {path} is refused",
            [404],
            sorted({status for _, status, _ in refusal}),
        )
        run.record(
            f"confinement: {path} served no document",
            [],
            sorted({size for _, _, size in refusal if size >= 256}),
        )

    # (f) Nothing on this path reaches the network. Reading a session
    # and playing it are filesystem work; a socket opened during either
    # would be a capability this surface has no mandate for.
    run.record(
        "no network: every destination the application reached", [], sockets.stop()
    )

    candidate.control(TRAY_QUIT)
    run.assert_that(
        "quit: the process exits",
        candidate.wait_for_exit(),
        "the application was still running after quit",
    )


# Scenarios are registered here rather than enumerated at each call
# site, so later work adds `recording` or `library` by adding one entry
# and its function.
def installed(candidate: Candidate, run: Run) -> None:
    """Drive a signed copy from outside the build tree.

    The scenario records and reads back a session through the installed copy,
    verifies the native tray through macOS Accessibility, and applies the
    selected distribution trust profile. ``community`` requires the stable
    project-controlled certificate while explicitly preserving Gatekeeper's
    expected rejection and the absence of notarization. ``apple-trusted``
    preserves the Developer ID and notarization gate for a future paid profile.

    The accessibility observation still needs the calling terminal to have a
    human-granted TCC Accessibility permission. The harness never grants that
    broad permission to itself and fails the leg instead of silently skipping it.
    """
    install_root = Path(tempfile.mkdtemp(prefix="scrybe-i-", dir="/tmp"))
    untouched = [
        ("the real configuration file", REAL_CONFIG, snapshot(REAL_CONFIG)),
        ("the default storage root", REAL_STORAGE_ROOT, snapshot(REAL_STORAGE_ROOT)),
    ]
    try:
        _installed_checks(candidate, run, install_root)
        hermeticity(candidate, run, untouched)
    finally:
        shutil.rmtree(install_root, ignore_errors=True)


def _installed_checks(candidate: Candidate, run: Run, install_root: Path) -> None:
    """Everything that needs the installed copy."""
    build_tree = candidate.bundle
    installed = candidate.install_into(install_root)
    run.assert_that(
        "install: the bundle was copied out of the build tree",
        installed.is_dir()
        and (installed / "Contents" / "MacOS" / EXECUTABLE).is_file(),
        f"no executable at {installed / 'Contents' / 'MacOS' / EXECUTABLE}",
    )
    run.record(
        "install: the copy being driven is under the build output directory",
        False,
        str(installed.resolve()).startswith(str(build_tree.parent.resolve())),
    )

    # The version the artifact reports about itself, read from the
    # installed copy by the gate that owns that question. A bundle built
    # before the propagation landed reports 0.0.0 here.
    version_gate = subprocess.run(
        [
            sys.executable,
            str(REPO_ROOT / "scripts" / "check-version-agreement.py"),
            "--bundle",
            str(installed),
        ],
        capture_output=True,
        text=True,
        check=False,
    )
    run.record(
        "install: the installed copy reports the workspace version in its Info.plist",
        0,
        version_gate.returncode,
    )

    # A registration pointing at a bundle that is gone is what a removed
    # install root leaves behind, and it makes every later launch of that
    # identifier fail in a way nothing here would otherwise notice.
    resolved = launch_services_resolution()
    run.assert_that(
        "install: Launch Services resolves this identifier to a bundle that is really there",
        resolved is not None and Path(resolved).is_dir(),
        f"the identifier resolves to {resolved!r}",
    )

    candidate.launch()
    sockets = SocketSampler(candidate)
    sockets.start()
    opened = candidate.wait_for_control_socket()
    run.assert_that(
        "launch: the installed copy opens its control channel",
        opened,
        f"no control socket appeared at {candidate.root / CONTROL_SOCKET}",
    )
    if not opened:
        run.record(
            "no network: every destination the application reached", [], sockets.stop()
        )
        return
    candidate.wait_for_event("window-shown")

    # The check every other scenario assumes. `open` goes through Launch
    # Services, which is free to hand the launch to any registered copy
    # of this identifier; this is what says the process being driven came
    # out of the copy that was installed.
    # Resolved on both sides: macOS symlinks `/tmp` to `/private/tmp`
    # and `NSRunningApplication` reports the resolved path, so comparing
    # the literals fails on a correct application.
    run.record(
        "launch: the process being driven came out of the installed copy",
        [str(installed.resolve())],
        [str(Path(path).resolve()) for path in candidate.registered_bundle_paths()],
    )

    _installed_accessibility(run)
    _installed_workflow(candidate, run)
    _installed_shippable(candidate, run, installed)
    run.record(
        "no network: every destination the application reached", [], sockets.stop()
    )


def _installed_workflow(candidate: Candidate, run: Run) -> None:
    """Record, stop, and read the session back through the reader."""
    run.record(
        "record: nothing has been recorded yet", [], session_folders(candidate.root)
    )

    candidate.control(TRAY_RECORD)
    started = candidate.wait_for_event("recording-transition", count=2, timeout=30.0)
    run.assert_that(
        "record: the installed copy started a recording from the tray",
        started,
        f"transitions observed: {recording_transitions(candidate)}",
    )
    if not started:
        return

    candidate.control(TRAY_STOP)
    saved = candidate.wait_for_event("recording-transition", count=4, timeout=120.0)
    run.assert_that(
        "stop: the recording finalized",
        saved,
        f"transitions observed: {recording_transitions(candidate)}",
    )
    folders = session_folders(candidate.root)
    run.record("stop: exactly one session was written", 1, len(folders))
    if not saved or len(folders) != 1:
        return

    folder = candidate.root / folders[0]
    run.record(
        "stop: the session holds everything a reader opens",
        [],
        sorted(
            name
            for name in ("transcript.md", "notes.md", "meta.toml", "audio.opus")
            if not (folder / name).is_file()
        ),
    )

    # The handoff. The reader is asked for the session the recorder just
    # produced, by the identity its own folder name forms, and what it
    # answers has to match the artifact that is really there. A reader
    # that offered playback for a session with no `playback.opus` is the
    # bug this asserts against — on a recorder-produced session rather
    # than a seeded one, which is the case that was never covered.
    # Recorded first, so a reader of the run can see which of the two
    # answers below was the right one to expect. A conditional check
    # whose branch is invisible is the shape that lets a branch quietly
    # stop running, and a qualification that cannot say which arm it
    # took is asserting something the reader cannot check.
    # Pinned rather than merely observed. The check below has two arms
    # and only one of them runs, so which one runs has to be a statement
    # that can fail: a conditional whose branch changed silently is how a
    # qualification ends up asserting nothing. `playback.opus` is written
    # only for a two-channel capture, and the synthetic source this run
    # configures is single-channel, so the refusal arm is the live one. If
    # that ever stops being true this fails and says so, instead of
    # quietly switching arms.
    has_playback = (folder / "playback.opus").is_file()
    run.record(
        "read: the recorder left no playback audio, because the configured capture is mono",
        False,
        has_playback,
    )
    responses = drive(candidate, run, f"/{folder.name}/playback")
    statuses = sorted({status for _, status, _ in responses})
    if has_playback:
        run.record(
            "read: the reader serves the recorder's own session, which has playback audio",
            [],
            sorted(set(statuses) - set(SERVED_STATUSES)),
        )
        run.record(
            "read: at least as many bytes were served as the artifact holds",
            True,
            sum(size for _, _, size in responses)
            >= (folder / "playback.opus").stat().st_size,
        )
    else:
        run.record(
            "read: the reader refuses the recorder's own session, which has no playback audio",
            [404],
            statuses,
        )

    candidate.control(TRAY_QUIT)
    run.assert_that(
        "quit: the installed copy exits once nothing is in flight",
        candidate.wait_for_exit(),
        "the application was still running after quit",
    )


# What `scripts/check-signed-artifact.py` means by each exit status, so a
# refusal here says which wall was hit rather than guessing at one.
SIGNED_ARTIFACT_STATUS = {
    0: "every assertion held",
    1: "the artifact failed an assertion",
    2: "an assertion could not be measured at all",
    3: "a required credential is absent, so nothing was attempted",
}


def _installed_accessibility(run: Run) -> None:
    """What a screen reader is told about the running application.

    The window's contents are not asserted here and do not need to be:
    `eslint-plugin-jsx-a11y` runs in `strict` mode over the frontend, and
    the suite already covers keyboard order, state carried by text rather
    than by colour, and live regions. What nothing covered is the one
    accessible surface that is not markup — the tray — and the only way
    to read it is the accessibility tree of the running process.

    The floating panel is deliberately not read here. It belongs to the
    command-line frontend: the desktop host depends on `scrybe-widgets`
    for the global hotkey alone and never builds a panel, so a panel
    assertion in this file would be a check that cannot pass and would
    say nothing about either application.
    """
    trusted = accessibility_grant()
    run.assert_that(
        "accessibility: this harness may read the application's accessibility tree",
        trusted,
        "AXIsProcessTrusted() is false for the process running this script, so no "
        "accessible name in another process can be read from here. What is missing is "
        "a TCC Accessibility grant, which is a human grant in System Settings > "
        "Privacy & Security > Accessibility. This harness must not acquire it for "
        "itself: granting Accessibility to a terminal grants it to everything run "
        "from that terminal, which is a far larger grant than this check is worth.",
    )
    if not trusted:
        return
    status_name, names = tray_accessibility()
    run.record(
        "accessibility: the menu-bar status item is announced by product name",
        "Scrybe",
        status_name,
    )
    run.record(
        "accessibility: every tray action is announced by name",
        ["Record now", "Stop  save", "Open Scrybe", "Quit Scrybe"],
        names,
    )


def _installed_shippable(candidate: Candidate, run: Run, installed: Path) -> None:
    """Verify the installed copy against the selected distribution profile."""
    status, said = signed_artifact_verdict(installed, candidate.trust_profile)
    meaning = SIGNED_ARTIFACT_STATUS.get(status, "an unrecognized status")
    if candidate.trust_profile == "community":
        contract = (
            "the stable Scrybe community identity, expected Gatekeeper rejection, "
            "and no notarization ticket"
        )
    else:
        contract = "a Developer ID identity, notarization, and Gatekeeper acceptance"
    run.assert_that(
        f"shippable: the installed copy satisfies {candidate.trust_profile} trust",
        status == 0,
        f"the {contract} contract failed: verifier exited {status} against "
        f"{installed} — {meaning}. Assertions:\n{indent(said)}",
    )


def indent(text: str) -> str:
    return "\n".join(f"      {line}" for line in text.splitlines())


def tray_accessibility() -> tuple[str, list[str]]:
    """Return the native status item's name and its menu-entry names."""
    script = (
        'const se = Application("System Events");'
        f'const proc = se.processes.byName("{BUNDLE_NAME.removesuffix(".app")}");'
        'const result = { status: "", entries: [] };'
        "proc.menuBars().forEach(bar => bar.menuBarItems().forEach(item => {"
        'if (item.description() !== "status menu") { return; }'
        'result.status = item.name() || "";'
        "item.menus().forEach(menu => menu.menuItems().forEach(entry => {"
        "const name = entry.name(); if (name) { result.entries.push(name); }"
        "}));"
        "}));"
        "JSON.stringify(result)"
    )
    result = subprocess.run(
        ["osascript", "-l", "JavaScript", "-e", script],
        capture_output=True,
        text=True,
        check=False,
    )
    observed = json.loads(result.stdout.strip() or '{"status":"","entries":[]}')
    return str(observed["status"]), [str(name) for name in observed["entries"]]


def seed_retention(candidate: Candidate) -> None:
    """Writes a trash the sweep has to reach two different answers about.

    Three folders. Two were put there by this application, so the index
    records when: one long enough ago that the window has passed, one at
    this instant. The third was put there by a reader, has no index
    entry, and is a complete session by content — the case where "remove
    what is old" and "remove what we moved" disagree, and the only one
    that can tell them apart.

    Days are simulated by writing the index the application writes,
    rather than by waiting. The alternative is a test that takes a week.
    """
    listed = candidate.root / PLAYABLE
    listed.mkdir(parents=True)
    (listed / "meta.toml").write_text(session_meta("Acme sync", "01HXYZ"))
    (listed / "audio.opus").write_bytes(b"")

    trash = candidate.root / "trash"
    trash.mkdir(parents=True)
    for folder, title, ident in (
        (SWEPT, "Long gone", "01CCCCC"),
        (KEPT, "Just now", "01DDDDD"),
        (BY_HAND, "Not ours", "01EEEEE"),
    ):
        path = trash / folder
        path.mkdir()
        (path / "meta.toml").write_text(session_meta(title, ident))
        (path / "audio.opus").write_bytes(b"")
    (trash / "a-note-to-self.txt").write_text("keep this\n")

    now = dt.datetime.now(dt.timezone.utc)
    swept_marker = "01ARZ3NDEKTSV4RRFFQ69G5FAV"
    kept_marker = "01ARZ3NDEKTSV4RRFFQ69G5FAW"
    (trash / SWEPT / f".scrybe-trash-entry-{swept_marker}").write_text(swept_marker)
    (trash / KEPT / f".scrybe-trash-entry-{kept_marker}").write_text(kept_marker)
    (trash / "retention.toml").write_text(
        f'[trashed."{SWEPT}"]\n'
        f'trashed_at = "{(now - dt.timedelta(days=400)).isoformat()}"\n'
        "retention_days = 7\n"
        f'marker = "{swept_marker}"\n'
        "purging = false\n\n"
        f'[trashed."{KEPT}"]\n'
        f'trashed_at = "{now.isoformat()}"\n'
        "retention_days = 7\n"
        f'marker = "{kept_marker}"\n'
        "purging = false\n"
    )

    archived = candidate.root / "archive" / NO_PLAYBACK
    archived.mkdir(parents=True)
    (archived / "meta.toml").write_text(session_meta("Set aside", "01AAAAA"))
    (archived / "audio.opus").write_bytes(b"")


def retention(candidate: Candidate, run: Run) -> None:
    """The launch sweep, on the real application.

    What this establishes that the unit tests cannot. The service's own
    tests prove which indexed folders a sweep removes, and the host's
    prove it preserves each entry's confirmed deadline — both by calling
    the sweep themselves. Neither can show that launching the application
    calls it at all, or that it happens before the window a reader would
    be looking at. A sweep wired to nothing passes every test in both
    suites.
    """
    untouched = [
        ("the real configuration file", REAL_CONFIG, snapshot(REAL_CONFIG)),
        ("the default storage root", REAL_STORAGE_ROOT, snapshot(REAL_STORAGE_ROOT)),
    ]
    seed_retention(candidate)

    candidate.launch()
    run.record(
        "launch: the application reached its control socket",
        True,
        candidate.wait_for_control_socket(),
    )
    run.record(
        "launch: the sweep ran, and ran once",
        1,
        candidate.events().count("retention-swept"),
    )

    trash = candidate.root / "trash"
    run.record(
        "sweep: the session past its window is gone",
        False,
        (trash / SWEPT).exists(),
    )
    run.record(
        "sweep: the session inside its window is still there",
        True,
        (trash / KEPT).is_dir(),
    )
    run.record(
        "sweep: a session the application never moved is left alone",
        True,
        (trash / BY_HAND).is_dir(),
    )
    run.record(
        "sweep: a reader's own file in the trash is left alone",
        True,
        (trash / "a-note-to-self.txt").is_file(),
    )
    run.record(
        "sweep: the index forgets what it removed and keeps what it did not",
        [False, True],
        [
            SWEPT in (trash / "retention.toml").read_text(),
            KEPT in (trash / "retention.toml").read_text(),
        ],
    )
    run.record(
        "sweep: the archive is untouched",
        True,
        (candidate.root / "archive" / NO_PLAYBACK).is_dir(),
    )
    run.record(
        "listing: the session outside the retention directories survived",
        True,
        (candidate.root / PLAYABLE).is_dir(),
    )

    # A second launch must reach the same answers: the index no longer
    # names the folder it removed, so nothing is removed twice and the
    # one inside its window is not aged by having been looked at.
    candidate.control("quit")
    candidate.wait_for_exit()
    candidate.launch()
    candidate.wait_for_control_socket()
    run.record(
        "relaunch: the session inside its window survived a second sweep",
        True,
        (trash / KEPT).is_dir(),
    )

    candidate.control("quit")
    candidate.wait_for_exit()
    hermeticity(candidate, run, untouched)


def model_download_live(candidate: Candidate, run: Run) -> None:
    """Download the production catalog artifact through the shipped transport."""
    catalog = tomllib.loads(MODEL_CATALOG.read_text(encoding="utf-8"))
    models = catalog.get("model", [])
    matches = [model for model in models if model.get("id") == "whisper-small-en"]
    run.record("catalog: one production whisper-small-en entry exists", 1, len(matches))
    if len(matches) != 1:
        return
    model = matches[0]
    source_url = str(model["source_url"])
    source_revision = str(model["source_revision"])
    destination_name = str(model["destination"])
    expected_size = int(model["size_bytes"])
    expected_digest = str(model["sha256"])
    destination = candidate.models / destination_name
    partial = destination.with_name(f"{destination.name}.partial")
    untouched = [
        ("the real configuration file", REAL_CONFIG, snapshot(REAL_CONFIG)),
        ("the default storage root", REAL_STORAGE_ROOT, snapshot(REAL_STORAGE_ROOT)),
        ("the platform model directory", PLATFORM_MODELS, snapshot(PLATFORM_MODELS)),
    ]

    run.record(
        "catalog: the production URL is pinned to its declared immutable revision",
        True,
        f"/resolve/{source_revision}/" in source_url,
    )
    candidate.launch()
    run.record(
        "launch: the application reached its control socket",
        True,
        candidate.wait_for_control_socket(),
    )
    candidate.control(
        f"probe-model-install {source_url} {expected_size} {expected_digest} "
        f"{destination_name} confirmed"
    )
    completed = candidate.wait_for_event("probe-model-install", timeout=1800.0)
    run.assert_that(
        "download: the production artifact completed within 30 minutes",
        completed,
        "no model-install outcome was recorded before the release-only timeout",
    )
    if completed:
        run.record(
            "download: the application atomically promoted the verified artifact",
            "ready:promoted=true",
            candidate.last_detail_of("probe-model-install"),
        )
    run.record("download: the destination exists", True, destination.is_file())
    if destination.is_file():
        run.record(
            "download: the installed byte count matches the production catalog",
            expected_size,
            destination.stat().st_size,
        )
        digest = hashlib.sha256()
        with destination.open("rb") as stream:
            for block in iter(lambda: stream.read(1024 * 1024), b""):
                digest.update(block)
        run.record(
            "download: the installed digest matches the production catalog",
            expected_digest,
            digest.hexdigest(),
        )
    run.record("download: no partial file remains", False, partial.exists())
    run.record(
        "confinement: the installed model is under the disposable model root",
        True,
        destination.resolve().is_relative_to(candidate.models.resolve()),
    )
    candidate.control("quit")
    run.assert_that(
        "quit: the process exits after the production download",
        candidate.wait_for_exit(),
        "the application was still running after quit",
    )
    hermeticity(candidate, run, untouched)


SCENARIOS: dict[str, Callable[[Candidate, Run], None]] = {
    "lifecycle": lifecycle,
    "setup": setup,
    "model-download-live": model_download_live,
    "library": library,
    "recording": recording,
    "installed": installed,
    "retention": retention,
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
    "model-download-live": (
        "the production catalog pin, a real Hugging Face TLS transfer, exact byte "
        "count and SHA-256 verification, atomic promotion, disposable model and "
        "configuration roots, and clean exit"
    ),
    "recording": (
        "preflight, starting and stopping a recording from the tray, "
        "the artifacts it leaves, the convergence of two stop sources on one "
        "finalization, refusal of a second recording, the deferred quit, and egress"
    ),
    "retention": (
        "the sweep at launch, a window read per session, a folder the "
        "application never moved, a reader's own file in the trash, the "
        "index after a removal, the archive, and a second launch"
    ),
    "library": (
        "the webview reaching the playback scheme at all, the bytes served for a "
        "session that has playback audio, the refusals for one that has none, for "
        "one that is repairable, and for one that never finished recording at "
        "all, every path the player never builds aimed at a file that is really "
        "there, a disposable storage root, the shipped content security policy, "
        "and every socket the process opened"
    ),
    "installed": (
        "a copy driven from outside the build tree, its Info.plist version and "
        "process identity, recorder-to-reader handoff, the menu-bar item's native "
        "accessible name and actions, and the selected distribution trust profile"
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
    parser.add_argument(
        "--trust-profile",
        choices=("community", "apple-trusted"),
        default="community",
        help="distribution trust contract used by the installed scenario",
    )
    arguments = parser.parse_args()

    if sys.platform != "darwin":
        print(
            "this harness qualifies the macOS application; nothing else is a target yet"
        )
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
    bundle = build_candidate(arguments.scenario, arguments.trust_profile)
    # A Unix socket path cannot exceed 104 bytes on macOS, and the
    # platform's own temporary directory is already most of that, so the
    # disposable root is created directly under `/tmp` with a short
    # prefix. The control socket lives inside it.
    workspace = Path(tempfile.mkdtemp(prefix="scrybe-q-", dir="/tmp"))
    candidate = Candidate(bundle, workspace, arguments.trust_profile)
    run = Run()
    try:
        SCENARIOS[arguments.scenario](candidate, run)
    finally:
        candidate.terminate()
        shutil.rmtree(workspace, ignore_errors=True)

    print()
    if run.failures:
        # The total as well as the count that failed. A reader of a failing run
        # otherwise cannot tell one refusal out of forty from one out of three.
        print(
            f"desktop {arguments.scenario} qualification FAILED — "
            f"{len(run.failures)} of {len(run.entries)} checks, across "
            f"{SCENARIO_COVERAGE[arguments.scenario]}:"
        )
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
