#!/usr/bin/env python3
# Copyright 2026 Mathews Tom
# Licensed under the Apache License, Version 2.0 (the "License");
# you may not use this file except in compliance with the License.
# You may obtain a copy of the License at
#     https://www.apache.org/licenses/LICENSE-2.0
"""Version-agreement gate: one version, and every surface that repeats it.

`[workspace.package].version` in the root `Cargo.toml` is the only place
a Scrybe version number is written by a human. Every workspace member
takes it with `version.workspace = true`, so Cargo resolves it and no
member can hold a different one.

Three classes of surface cannot inherit, and each is a place the number
used to be restated by hand:

1. `scrybe-desktop/src-tauri/Cargo.toml` declares its own `[workspace]`,
   deliberately, so the root's jobs can build the library and CLI graph
   without pulling a WebView host into them. A separate workspace cannot
   inherit from the root one.
2. `scrybe-desktop/src-tauri/tauri.conf.json` is a JSON literal. It is
   the value Tauri stamps into the bundle's `CFBundleShortVersionString`,
   so it is the one that decides what the artifact reports about itself.
3. Eleven intra-workspace dependency pins of the form
   `version = "=1.6.0"`. A path dependency in a manifest that will be
   published needs an exact version alongside the path, because the
   packaged manifest keeps only the version. These are the least visible
   restatements in the tree and the most likely to be missed: every one
   of them has to move in lockstep with the workspace version or
   `cargo package` stops resolving.

This gate reads the source of truth and compares it to all three, and —
given `--bundle` — to what a built bundle reports in its `Info.plist`.
That last one is the only check here that measures the artifact rather
than the source, which is why it is the one worth running after a build
rather than in a manifest-only lane.

`--write` propagates the source of truth into the surfaces in classes 1
to 3 rather than asking a human to edit eleven-plus-two places
consistently. It is the mechanism; the default check is what makes the
mechanism's failure visible. Neither replaces the other: `--write`
without a check is a script nobody runs, and a check without `--write`
is a gate that tells you about thirteen edits and then makes you do them
by hand.

The check fails on drift. To see that rather than take it on trust,
change any one of the surfaces and re-run:

    python3 - <<'EOF'
    from pathlib import Path
    p = Path("scrybe-desktop/src-tauri/tauri.conf.json")
    p.write_text(p.read_text().replace('"version": "1.6.0"', '"version": "1.6.1"'))
    EOF
    python3 scripts/check-version-agreement.py   # exits 1, names the surface

Run locally:

    python3 scripts/check-version-agreement.py
    python3 scripts/check-version-agreement.py --write
    python3 scripts/check-version-agreement.py \
        --bundle scrybe-desktop/src-tauri/target/debug/bundle/macos/Scrybe.app

Run in CI: see `.github/workflows/ci.yml` job `version-agreement`.

Exit status 0 means every surface agrees. Exit status 1 means at least
one does not, and the disagreement is printed with the file that holds
it. Exit status 2 means the gate could not run — a missing manifest, an
unreadable `Info.plist`, a `cargo metadata` that failed — which is a
failure too, because a gate that cannot read a surface must never report
agreement about it.
"""

from __future__ import annotations

import argparse
import json
import plistlib
import re
import subprocess
import sys
import tomllib
from dataclasses import dataclass
from pathlib import Path
from typing import Any

REPO_ROOT = Path(__file__).resolve().parent.parent
ROOT_MANIFEST = REPO_ROOT / "Cargo.toml"
HOST_MANIFEST = REPO_ROOT / "scrybe-desktop" / "src-tauri" / "Cargo.toml"
BUNDLE_CONFIG = REPO_ROOT / "scrybe-desktop" / "src-tauri" / "tauri.conf.json"

# Dependency tables a member manifest may declare a path dependency in.
# `[target.<cfg>.dependencies]` is reached separately because its key is
# a `cfg` expression rather than a fixed name.
DEPENDENCY_TABLES = ("dependencies", "dev-dependencies", "build-dependencies")

# The two keys in a macOS `Info.plist` that carry a version. Tauri fills
# both from `tauri.conf.json`'s `version`, so both are checked: a bundle
# whose marketing version agrees and whose build version does not is a
# bundle whose propagation is half wired.
PLIST_VERSION_KEYS = ("CFBundleShortVersionString", "CFBundleVersion")


class Unreadable(Exception):
    """A surface could not be read, so nothing may be claimed about it."""


@dataclass(frozen=True)
class Surface:
    """One place a version number appears, and what it says there."""

    name: str
    where: str
    found: str

    def holds(self, expected: str) -> bool:
        return self.found == expected


def workspace_version() -> str:
    """The one version a human writes."""
    manifest = load_toml(ROOT_MANIFEST)
    package = manifest.get("workspace", {}).get("package", {})
    version = package.get("version")
    if not isinstance(version, str):
        raise Unreadable(
            f"{rel(ROOT_MANIFEST)} declares no `[workspace.package].version`; "
            "there is no source of truth to compare anything against"
        )
    return version


def load_toml(path: Path) -> dict[str, Any]:
    try:
        return tomllib.loads(path.read_text())
    except OSError as error:
        raise Unreadable(f"cannot read {rel(path)}: {error}") from error
    except tomllib.TOMLDecodeError as error:
        raise Unreadable(f"{rel(path)} is not valid TOML: {error}") from error


def rel(path: Path) -> str:
    try:
        return str(path.relative_to(REPO_ROOT))
    except ValueError:
        return str(path)


def members() -> list[str]:
    manifest = load_toml(ROOT_MANIFEST)
    declared = manifest.get("workspace", {}).get("members")
    if not isinstance(declared, list) or not declared:
        raise Unreadable(f"{rel(ROOT_MANIFEST)} declares no `[workspace].members`")
    return [str(member) for member in declared]


def inheritance_surfaces() -> list[Surface]:
    """What each member manifest says its own version is.

    A member that inherits reports the literal `version.workspace`, which
    no version string can collide with. A member that hardcodes reports
    the string it hardcoded, and fails against the source of truth even
    when the string happens to match today — because matching today is
    exactly the property this PR exists to stop relying on.
    """
    surfaces: list[Surface] = []
    for member in members():
        manifest_path = REPO_ROOT / member / "Cargo.toml"
        package = load_toml(manifest_path).get("package", {})
        declared = package.get("version")
        if isinstance(declared, dict) and declared.get("workspace") is True:
            found = "version.workspace"
        elif isinstance(declared, str):
            found = f"hardcoded {declared!r}"
        else:
            found = "absent"
        surfaces.append(Surface(f"{member} inherits", rel(manifest_path), found))
    return surfaces


def resolved_surfaces() -> list[Surface]:
    """What Cargo resolves each member's version to.

    `inheritance_surfaces` reads the text; this reads the resolution. Both
    are needed: text alone would pass a `version.workspace = true` that
    resolved against a different workspace root, and resolution alone
    would pass a manifest that hardcodes the right number today.
    """
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
        raise Unreadable(f"cargo metadata failed, so no version resolves: {detail}") from error
    payload = json.loads(result.stdout)
    return [
        Surface(
            f"{package['name']} resolves",
            "cargo metadata",
            str(package["version"]),
        )
        for package in sorted(payload["packages"], key=lambda entry: entry["name"])
    ]


def host_surface() -> Surface:
    package = load_toml(HOST_MANIFEST).get("package", {})
    version = package.get("version")
    return Surface(
        "desktop host manifest",
        rel(HOST_MANIFEST),
        version if isinstance(version, str) else "absent",
    )


def bundle_config_surface() -> Surface:
    try:
        config = json.loads(BUNDLE_CONFIG.read_text())
    except OSError as error:
        raise Unreadable(f"cannot read {rel(BUNDLE_CONFIG)}: {error}") from error
    except json.JSONDecodeError as error:
        raise Unreadable(f"{rel(BUNDLE_CONFIG)} is not valid JSON: {error}") from error
    version = config.get("version")
    return Surface(
        "bundle configuration",
        rel(BUNDLE_CONFIG),
        version if isinstance(version, str) else "absent",
    )


def dependency_specs(manifest: dict[str, Any]) -> list[tuple[str, dict[str, Any]]]:
    """Every dependency spec in a manifest, with the name it is keyed by."""
    specs: list[tuple[str, dict[str, Any]]] = []
    tables: list[dict[str, Any]] = [
        table for name in DEPENDENCY_TABLES if isinstance(table := manifest.get(name), dict)
    ]
    for platform in (manifest.get("target") or {}).values():
        if not isinstance(platform, dict):
            continue
        tables.extend(
            table for name in DEPENDENCY_TABLES if isinstance(table := platform.get(name), dict)
        )
    for table in tables:
        specs.extend((key, spec) for key, spec in table.items() if isinstance(spec, dict))
    return specs


def pin_surfaces() -> list[Surface]:
    """Every exact pin a member places on another member.

    Restricted to path dependencies that point inside the workspace, so
    the third-party exact pins in the tree — `tokenizers`, `url`,
    `sherpa-onnx` — are neither read nor rewritten. Those are pinned for
    reasons of their own and have nothing to do with this version.
    """
    member_dirs = set(members())
    surfaces: list[Surface] = []
    for member in members():
        manifest_path = REPO_ROOT / member / "Cargo.toml"
        for key, spec in dependency_specs(load_toml(manifest_path)):
            path = spec.get("path")
            if not isinstance(path, str) or Path(path).name not in member_dirs:
                continue
            version = spec.get("version")
            surfaces.append(
                Surface(
                    f"{member} pins {key}",
                    rel(manifest_path),
                    version.removeprefix("=") if isinstance(version, str) else "absent",
                )
            )
    if not surfaces:
        raise Unreadable(
            "no intra-workspace dependency pin was found; this tree has eleven, "
            "so finding none means the manifests are not being read as expected"
        )
    return surfaces


def bundle_surfaces(bundle: Path) -> list[Surface]:
    """What a built bundle reports about itself.

    The source surfaces above all read files a human edits. This one
    reads the artifact, which is the only one of them that can disagree
    with every other while every other agrees — a stale bundle, a build
    that did not pick the configuration up, a bundler that filled the
    key from somewhere else.
    """
    plist_path = bundle / "Contents" / "Info.plist"
    try:
        with plist_path.open("rb") as handle:
            plist = plistlib.load(handle)
    except OSError as error:
        raise Unreadable(f"cannot read {plist_path}: {error}") from error
    except plistlib.InvalidFileException as error:
        raise Unreadable(f"{plist_path} is not a readable property list: {error}") from error
    return [
        Surface(
            f"built bundle {key}",
            str(plist_path),
            str(plist.get(key, "absent")),
        )
        for key in PLIST_VERSION_KEYS
    ]


def propagate(version: str) -> list[str]:
    """Writes the source of truth into every surface that cannot inherit.

    Line-level substitution rather than a TOML or JSON round-trip: these
    files carry comments and an order that document why they are shaped
    the way they are, and a serializer would discard both.
    """
    member_dirs = set(members())
    written: list[str] = []
    written.extend(
        rewrite(HOST_MANIFEST, r'(?m)^version = "[^"]*"', f'version = "{version}"')
    )
    written.extend(
        rewrite(BUNDLE_CONFIG, r'"version": "[^"]*"', f'"version": "{version}"')
    )
    for member in members():
        written.extend(repin(REPO_ROOT / member / "Cargo.toml", member_dirs, version))
    return written


def rewrite(path: Path, pattern: str, replacement: str) -> list[str]:
    original = path.read_text()
    updated, count = re.subn(pattern, replacement, original)
    if count == 0 or updated == original:
        return []
    path.write_text(updated)
    return [f"{rel(path)}: {count} occurrence(s) rewritten to {replacement!r}"]


def repin(path: Path, member_dirs: set[str], version: str) -> list[str]:
    """Rewrites only the exact pins that point at another workspace member.

    Line-scoped, and conditioned on the path the same line carries, so
    the third-party exact pins sitting in these very tables —
    `tokenizers`, `url`, `sherpa-onnx` — are neither matched nor
    touched. Those are pinned for reasons of their own.
    """
    lines = path.read_text().splitlines(keepends=True)
    changed = 0
    for index, line in enumerate(lines):
        target = re.search(r'path = "\.\./([^"]+)"', line)
        if target is None or target.group(1) not in member_dirs:
            continue
        updated, count = re.subn(r'version = "=[^"]*"', f'version = "={version}"', line)
        if count and updated != line:
            lines[index] = updated
            changed += count
    if changed == 0:
        return []
    path.write_text("".join(lines))
    return [f"{rel(path)}: {changed} intra-workspace pin(s) set to ={version}"]


def tabulate(heading: str, expected: str, surfaces: list[Surface]) -> list[Surface]:
    """Prints one table of surfaces and returns the ones that disagree."""
    width = max(len(surface.name) for surface in surfaces)
    print(f"{heading} (expected: {expected})")
    for surface in surfaces:
        status = "ok" if surface.holds(expected) else "DISAGREES"
        print(f"  {surface.name:<{width}}  {surface.found:<22}  {status}")
    print()
    return [surface for surface in surfaces if not surface.holds(expected)]


def explain(disagreements: list[tuple[Surface, str]]) -> None:
    print(f"version agreement FAILED — {len(disagreements)} surface(s) disagree:")
    for surface, expected in disagreements:
        print(f"  {surface.name}")
        print(f"    in:       {surface.where}")
        print(f"    expected: {expected}")
        print(f"    found:    {surface.found}")
    print()
    print("Run `python3 scripts/check-version-agreement.py --write` to propagate the")
    print("source of truth into every surface that cannot inherit it, or correct")
    print("`[workspace.package].version` if the source of truth is what is wrong.")


def collect(bundle: Path | None) -> list[Surface]:
    surfaces = inheritance_surfaces()
    surfaces.extend(resolved_surfaces())
    surfaces.append(host_surface())
    surfaces.append(bundle_config_surface())
    surfaces.extend(pin_surfaces())
    if bundle is not None:
        surfaces.extend(bundle_surfaces(bundle))
    return surfaces


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--write",
        action="store_true",
        help="propagate the workspace version into every surface that cannot inherit it",
    )
    parser.add_argument(
        "--bundle",
        type=Path,
        default=None,
        help="also check the version a built `.app` reports in its Info.plist",
    )
    arguments = parser.parse_args()

    try:
        version = workspace_version()
        if arguments.write:
            written = propagate(version)
            if not written:
                print(f"every surface already reports {version}; nothing written")
            for line in written:
                print(line)
            print()
        surfaces = collect(arguments.bundle)
    except Unreadable as error:
        print(f"version agreement could not be established: {error}", file=sys.stderr)
        return 2

    print(f"source of truth: {rel(ROOT_MANIFEST)} [workspace.package].version = {version}")
    print()
    # A member manifest that inherits reports the literal
    # `version.workspace`, not a version, so it is held to that literal
    # rather than to the number. Two tables rather than one, because a
    # single table with two different expectations in it is the kind of
    # output a reader misreads.
    inheriting = [surface for surface in surfaces if surface.name.endswith(" inherits")]
    versioned = [surface for surface in surfaces if not surface.name.endswith(" inherits")]
    disagreements: list[tuple[Surface, str]] = [
        (surface, "version.workspace")
        for surface in tabulate("member manifests", "version.workspace", inheriting)
    ]
    disagreements.extend(
        (surface, version)
        for surface in tabulate("surfaces that cannot inherit", version, versioned)
    )
    if disagreements:
        explain(disagreements)
        return 1
    print(f"version agreement: ok — {len(surfaces)} surfaces, no drift")
    return 0


if __name__ == "__main__":
    sys.exit(main())
