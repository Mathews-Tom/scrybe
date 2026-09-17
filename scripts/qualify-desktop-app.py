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
- the no-network assertion has three parts, and each covers something
  the others do not. The crate-graph check covers the Rust host only: a
  host graph with no HTTP, TLS, DNS, QUIC, or WebSocket client cannot
  open a connection from Rust. It says nothing about the webview, which
  carries the platform's own networking stack, so a single remote image,
  font, stylesheet, or `fetch()` added to a future view would egress
  with no denylisted crate anywhere in the graph. What governs that is
  the content security policy, which is read out of the built bundle and
  compared against the expected value, and the navigation guard, which
  is driven from the frontend and observed rather than assumed. The
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

Run locally:

    python3 scripts/qualify-desktop-app.py --hermetic --scenario lifecycle

Exit status 0 means every check held. Exit status 1 means at least one
did not; each failing check prints what was expected and what was
observed.
"""

from __future__ import annotations

import argparse
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

SETTLE_SECONDS = 2.0
POLL_SECONDS = 0.1

# How often the socket sampler looks, once it is running. Short enough
# that a request which opens and closes across a settle is seen, long
# enough that `lsof` is not the thing being measured.
SAMPLE_SECONDS = 0.25

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
RECORD_WINDOW_URL = "record-window-url"


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
    """Builds the bundle a double-click opens, and returns its path."""
    subprocess.run(
        ["pnpm", "--dir", str(DESKTOP), "install", "--frozen-lockfile"],
        cwd=REPO_ROOT,
        check=True,
    )
    subprocess.run(
        ["pnpm", "--dir", str(DESKTOP), "tauri", "build", "--debug", "--bundles", "app"],
        cwd=REPO_ROOT,
        check=True,
    )
    bundle = HOST / "target" / "debug" / "bundle" / "macos" / BUNDLE_NAME
    if not bundle.is_dir():
        raise RuntimeError(f"the candidate bundle was not produced at {bundle}")
    return bundle


def build_release_binary() -> Path:
    """Builds the release host, for the compiled-out assertion."""
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
        self.config = workspace / "config.toml"
        self.root.mkdir(parents=True)
        self.config.write_text(f'[storage]\nroot = "{self.root}"\n')

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


def policy_occurrences(binary: Path, policy: str) -> tuple[bool, int]:
    """Whether the built executable carries `policy`, and how many
    policies it carries at all.

    Tauri embeds the frontend in the executable rather than shipping it
    as a file in the bundle, and embeds the configured policy alongside
    it as the value it injects at run time. Reading it back out of the
    built artifact is what makes this a check on what was shipped rather
    than on what `tauri.conf.json` says, which would pass whatever the
    file was changed to.
    """
    image = binary.read_bytes()
    return policy.encode() in image, image.count(b"default-src ")


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

    # (h) No network from the Rust host, structurally. This covers the
    # host graph and nothing else: the webview carries the platform's
    # own networking stack, so the policy and the navigation guard
    # checked above are what govern it.
    graph = host_dependency_graph()
    run.record(
        "no network: HTTP, TLS, DNS, QUIC, or WebSocket clients in the host graph",
        [],
        sorted(NETWORK_DENYLIST & graph),
    )
    run.record(
        "no network: internet sockets held by the application, sampled from launch to exit",
        [],
        sockets.stop(),
    )

    # (h) The policy the bundle ships, read back out of it.
    carried, policies = policy_occurrences(shipped / EXECUTABLE, EXPECTED_CSP)
    run.record("policy: the bundle carries the expected content security policy", True, carried)
    run.record("policy: the bundle carries exactly one content security policy", 1, policies)

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


# Scenarios are registered here rather than enumerated at each call
# site, so later work adds `setup`, `recording`, or `library` by adding
# one entry and its function.
SCENARIOS: dict[str, Callable[[Candidate, Run], None]] = {
    "lifecycle": lifecycle,
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
        "across launch, close, restore, recreation, second launch, quit, "
        "disposable-root confinement, the shipped content security policy, "
        "navigation, and egress"
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
