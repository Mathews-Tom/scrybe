#!/usr/bin/env python3
# Copyright 2026 Mathews Tom
# Licensed under the Apache License, Version 2.0 (the "License");
# you may not use this file except in compliance with the License.
# You may obtain a copy of the License at
#     https://www.apache.org/licenses/LICENSE-2.0
"""The publish set and its order, derived from the workspace.

Which packages go to crates.io, and in what order, used to be a list
typed into `.github/workflows/ci.yml` and a second list typed into
`docs/RELEASE.md`. Neither was derived from anything. The `ci.yml` list
silently omitted `scrybe-widgets` until `cargo package -p scrybe` failed
with `no matching package named scrybe-widgets`, and at the time of
writing the `RELEASE.md` list still omits it.

This script derives both instead:

- **The set** is every workspace package whose own manifest permits
  publication. `cargo metadata` reports that as `publish: null` for a
  package that may go anywhere and `publish: []` for one that may not,
  so the set is read from each manifest's own field rather than
  restated. Marking a package publishable, or adding a publishable
  package, changes the set with nobody editing a list.
- **The order** is a topological sort of the dependency edges between
  the packages in that set: a package appears after everything it
  depends on, because `cargo publish` for it cannot resolve until its
  dependencies are on the registry. Ties are broken alphabetically, so
  the order is a function of the graph and not of dictionary iteration.

Three things are asserted. Each of them fails the run with exit status
1 — the order was computed and is not fit to publish from, which is a
different answer from "the order could not be computed":

1. **No publishable package depends on an unpublishable workspace
   member.** This is the `scrybe-widgets` failure stated as a check. A
   published manifest cannot carry a bare path dependency, so a
   publishable package that names a private one produces a packaged
   manifest that nothing can resolve. The old hardcoded list could not
   notice this; it *was* the thing that did not notice it.
2. **The graph is acyclic.** A cycle has no publication order at all,
   and the honest answer is to say so rather than emit some order.
3. **`docs/crates-io-registry-state.toml` covers exactly the derived
   set.** Registry state is the one fact here that cannot be derived
   from the workspace — see that file's own header — so it is a checked-
   in input. Requiring exact coverage means adding a publishable package
   forces its registry state to be recorded in the same change, rather
   than being discovered at publication time.

`--write` regenerates `docs/publish-order.md`, the artifact a human
reads before authorizing a publication. `--check` regenerates it in
memory and fails if the checked-in copy differs, so the artifact cannot
go stale relative to the manifests it describes.

Run locally:

    python3 scripts/publish-order.py              # the order, numbered
    python3 scripts/publish-order.py --names       # bare names, in order
    python3 scripts/publish-order.py --cargo-args  # `-p <name>` tokens
    python3 scripts/publish-order.py --check       # artifact is current
    python3 scripts/publish-order.py --write       # regenerate artifact

Run in CI: see `.github/workflows/ci.yml` job `dist-plan`, which packages
`--cargo-args` rather than a list, and asserts `--check`.

Exit status 0 means the set and order were derived and every assertion
held. Exit status 1 means an assertion failed, and what failed is
printed. Exit status 2 means the derivation could not run — `cargo
metadata` failed, the registry-state record is unreadable — which is a
failure too, because a release must not be authorized against a publish
order nobody could compute.
"""

from __future__ import annotations

import argparse
import difflib
import json
import subprocess
import sys
import tomllib
from pathlib import Path
from typing import Any

REPO_ROOT = Path(__file__).resolve().parent.parent
REGISTRY_STATE = REPO_ROOT / "docs" / "crates-io-registry-state.toml"
ARTIFACT = REPO_ROOT / "docs" / "publish-order.md"

# `cargo metadata` reports a package's `publish` field as `None` when the
# manifest permits publication to any registry and as a list of allowed
# registries otherwise — `publish = false` becomes the empty list. There
# is no third shape, and treating an unexpected one as publishable would
# be the wrong direction to fail in.
PUBLISHABLE = None

# Dependency kinds that must be on the registry before a package can be
# published. `dev` is included: `cargo publish` resolves the packaged
# manifest in full, and a dev-dependency on a workspace member that is
# not yet published fails that resolution exactly like a normal one.
BLOCKING_KINDS = (None, "build", "dev")


class Undeterminable(Exception):
    """An input could not be read, so nothing is derived and nothing claimed."""


class Refused(Exception):
    """The set or order was derived and is not fit to publish from."""


def metadata() -> dict[str, Any]:
    try:
        result = subprocess.run(
            ["cargo", "metadata", "--no-deps", "--format-version", "1", "--locked"],
            cwd=REPO_ROOT,
            capture_output=True,
            text=True,
            check=True,
        )
    except (OSError, subprocess.CalledProcessError) as error:
        detail = getattr(error, "stderr", "") or str(error)
        raise Undeterminable(f"cargo metadata failed: {detail}") from error
    return json.loads(result.stdout)


def workspace_packages() -> dict[str, dict[str, Any]]:
    return {package["name"]: package for package in metadata()["packages"]}


def publishable(packages: dict[str, dict[str, Any]]) -> set[str]:
    return {
        name for name, package in packages.items() if package.get("publish") is PUBLISHABLE
    }


def workspace_edges(
    packages: dict[str, dict[str, Any]], name: str
) -> set[str]:
    """Every workspace member `name` needs on the registry before it.

    Read from path dependencies, because a path is what makes a
    dependency a workspace member rather than something already on the
    registry. Optional dependencies count: `cargo publish` resolves the
    whole manifest regardless of which features are on, which is why an
    optional `scrybe-widgets` still had to be packaged alongside
    `scrybe`.
    """
    members = set(packages)
    return {
        dependency["name"]
        for dependency in packages[name]["dependencies"]
        if dependency.get("path")
        and dependency.get("kind") in BLOCKING_KINDS
        and dependency["name"] in members
    }


def assert_closed(packages: dict[str, dict[str, Any]], publish_set: set[str]) -> None:
    """Refuses a set that depends on something outside itself."""
    leaks: list[str] = []
    for name in sorted(publish_set):
        for dependency in sorted(workspace_edges(packages, name) - publish_set):
            leaks.append(
                f"{name} is publishable and depends on {dependency}, which is not: "
                f"the packaged manifest for {name} would name a version no registry has"
            )
    if leaks:
        raise Refused(
            "the publish set is not closed under its dependencies:\n  " + "\n  ".join(leaks)
        )


def order(packages: dict[str, dict[str, Any]], publish_set: set[str]) -> list[str]:
    """Topological order, alphabetical within each ready set."""
    pending = {
        name: workspace_edges(packages, name) & publish_set for name in publish_set
    }
    sequence: list[str] = []
    while pending:
        ready = sorted(name for name, blockers in pending.items() if not blockers)
        if not ready:
            raise Refused(
                "the publishable packages form a dependency cycle, so they have no "
                f"publication order at all: {sorted(pending)}"
            )
        chosen = ready[0]
        sequence.append(chosen)
        del pending[chosen]
        for blockers in pending.values():
            blockers.discard(chosen)
    return sequence


def registry_state(publish_set: set[str]) -> dict[str, dict[str, str]]:
    """The checked-in record of what is already on crates.io."""
    try:
        recorded = tomllib.loads(REGISTRY_STATE.read_text()).get("packages", {})
    except OSError as error:
        raise Undeterminable(f"cannot read {rel(REGISTRY_STATE)}: {error}") from error
    except tomllib.TOMLDecodeError as error:
        raise Undeterminable(f"{rel(REGISTRY_STATE)} is not valid TOML: {error}") from error
    missing = sorted(publish_set - set(recorded))
    extra = sorted(set(recorded) - publish_set)
    if missing or extra:
        complaints = []
        if missing:
            complaints.append(
                f"no registry state recorded for {missing}; a publication cannot be "
                f"authorized against a package nobody has looked up"
            )
        if extra:
            complaints.append(
                f"registry state recorded for {extra}, which this workspace does not "
                f"publish; remove the entry or mark the package publishable"
            )
        raise Refused(
            f"{rel(REGISTRY_STATE)} does not cover the derived set:\n  "
            + "\n  ".join(complaints)
        )
    return recorded


def rel(path: Path) -> str:
    return str(path.relative_to(REPO_ROOT))


def render(sequence: list[str], packages: dict[str, dict[str, Any]], recorded: dict[str, dict[str, str]]) -> str:
    """The artifact. Generated, so nothing in it is transcribed."""
    publish_set = set(sequence)
    never = [name for name in sequence if not recorded[name]["latest"]]
    lines = [
        "<!-- Generated by `python3 scripts/publish-order.py --write`. Do not edit. -->",
        "<!-- `python3 scripts/publish-order.py --check` fails when this file is stale. -->",
        "",
        "# Publish Order",
        "",
        "Read this before authorizing a publication. Everything below except the registry",
        "state is derived from the workspace: the set comes from each manifest's own",
        "`publish` field and the order from the dependency edges between the packages in",
        "that set. No list here was typed by hand, so marking a package publishable, or",
        "adding a publishable package, changes this file rather than being missed by it.",
        "",
    ]

    if never:
        lines.extend(
            [
                "## Read this first: first publications in this set",
                "",
                "The following packages **have never been published**. Whenever a release version",
                "is assigned, their first publication happens as part of it. A first publication",
                "claims the name on crates.io permanently, cannot be undone, and has no prior",
                "version anyone can diff against or roll back to.",
                "",
            ]
        )
        for name in never:
            lines.append(f"- `{name}` — not on the registry as of {recorded[name]['measured']}")
        lines.extend(
            [
                "",
                "Registry state is recorded in [`crates-io-registry-state.toml`](crates-io-registry-state.toml)",
                "and re-measured with `cargo info <name> --registry crates-io`. It is an input to",
                "this file rather than something derivable from the workspace, because no manifest",
                "knows what a registry already holds.",
                "",
            ]
        )

    lines.extend(
        [
            "## Order",
            "",
            "Publish in this order. Each package appears after everything it depends on,",
            "because `cargo publish` for it cannot resolve until those are on the registry.",
            "",
            "| # | package | on registry | waits for |",
            "| - | ------- | ----------- | --------- |",
        ]
    )
    for position, name in enumerate(sequence, start=1):
        blockers = sorted(workspace_edges(packages, name) & publish_set)
        waits = ", ".join(f"`{blocker}`" for blocker in blockers) if blockers else "nothing"
        latest = recorded[name]["latest"] or "**never published**"
        lines.append(f"| {position} | `{name}` | {latest} | {waits} |")

    private = sorted(set(packages) - publish_set)
    lines.extend(
        [
            "",
            "## Not published",
            "",
            "These workspace members declare `publish = false` and are deliberately absent",
            "from the set above. A publishable package that depended on one of them would",
            "fail `scripts/publish-order.py`, because its packaged manifest would name a",
            "version no registry has — which is the failure the previous hardcoded list",
            "could not detect.",
            "",
        ]
    )
    lines.extend(f"- `{name}`" for name in private)
    lines.extend(
        [
            "",
            "## No version is assigned here",
            "",
            "This file records an order, not a decision to publish. The workspace version is",
            "held to one place and checked by `scripts/check-version-agreement.py`; assigning",
            "a release version, tagging, and publishing are separate acts that need a human.",
            "",
        ]
    )
    return "\n".join(lines)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    mode = parser.add_mutually_exclusive_group()
    mode.add_argument(
        "--cargo-args",
        action="store_true",
        help="emit `-p <name>` tokens, one per line, for `cargo package` or `cargo publish`",
    )
    mode.add_argument(
        "--names",
        action="store_true",
        help="emit package names, one per line, in publication order",
    )
    mode.add_argument(
        "--check",
        action="store_true",
        help=f"fail if {rel(ARTIFACT)} does not match what the workspace derives",
    )
    mode.add_argument(
        "--write",
        action="store_true",
        help=f"regenerate {rel(ARTIFACT)}",
    )
    arguments = parser.parse_args()

    try:
        packages = workspace_packages()
        publish_set = publishable(packages)
        if not publish_set:
            raise Undeterminable(
                "no workspace package is publishable; this workspace publishes six, so "
                "an empty set means the `publish` field is not being read as expected"
            )
        assert_closed(packages, publish_set)
        sequence = order(packages, publish_set)
        recorded = registry_state(publish_set)
    except Undeterminable as error:
        print(f"publish order could not be derived: {error}", file=sys.stderr)
        return 2
    except Refused as error:
        print(f"publish order refused: {error}", file=sys.stderr)
        return 1

    if arguments.cargo_args:
        for name in sequence:
            print("-p")
            print(name)
        return 0

    if arguments.names:
        for name in sequence:
            print(name)
        return 0

    rendered = render(sequence, packages, recorded)
    if arguments.write:
        ARTIFACT.write_text(rendered)
        print(f"wrote {rel(ARTIFACT)}: {len(sequence)} packages")
        return 0
    if arguments.check:
        return check(rendered)

    for position, name in enumerate(sequence, start=1):
        print(f"{position}. {name}")
    return 0


def check(rendered: str) -> int:
    try:
        existing = ARTIFACT.read_text()
    except OSError as error:
        print(
            f"{rel(ARTIFACT)} is missing or unreadable ({error}); run "
            f"`python3 scripts/publish-order.py --write`",
            file=sys.stderr,
        )
        return 1
    if existing == rendered:
        print(f"{rel(ARTIFACT)} matches what the workspace derives")
        return 0
    print(f"{rel(ARTIFACT)} is stale relative to the workspace:")
    for line in difflib.unified_diff(
        existing.splitlines(),
        rendered.splitlines(),
        fromfile=f"{rel(ARTIFACT)} (checked in)",
        tofile="derived from the workspace",
        lineterm="",
    ):
        print(f"  {line}")
    print()
    print("Run `python3 scripts/publish-order.py --write` and commit the result.")
    return 1


if __name__ == "__main__":
    sys.exit(main())
