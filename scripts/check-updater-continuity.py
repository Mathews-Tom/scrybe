#!/usr/bin/env python3
# Copyright 2026 Mathews Tom
# Licensed under the Apache License, Version 2.0 (the "License");
"""Verify identity, TCC binding, and data continuity across a macOS update."""

from __future__ import annotations

import argparse
import hashlib
import plistlib
import re
import subprocess
import tempfile
from pathlib import Path
from typing import Any

VERSION = re.compile(r"^[0-9]+(?:\.[0-9]+)*$")


class ProbeError(Exception):
    """The continuity probe could not establish its required invariant."""


def observe(command: list[str]) -> str:
    try:
        result = subprocess.run(command, capture_output=True, text=True, check=False)
    except OSError as error:
        raise ProbeError(f"cannot run `{' '.join(command)}`: {error}") from error
    if result.returncode != 0:
        raise ProbeError(
            f"`{' '.join(command)}` exited {result.returncode}: "
            f"{result.stdout}{result.stderr}".strip()
        )
    return f"{result.stdout}{result.stderr}"


def info(app: Path) -> dict[str, Any]:
    path = app / "Contents" / "Info.plist"
    try:
        with path.open("rb") as handle:
            value = plistlib.load(handle)
    except (OSError, plistlib.InvalidFileException) as error:
        raise ProbeError(f"cannot read {path}: {error}") from error
    if not isinstance(value, dict):
        raise ProbeError(f"{path} does not contain a property-list dictionary")
    return value


def requirement(app: Path) -> str:
    output = observe(["codesign", "-d", "-r-", str(app)])
    for line in output.splitlines():
        if line.startswith("designated =>"):
            return line.removeprefix("designated =>").strip()
    raise ProbeError(f"codesign reported no designated requirement for {app}")


def certificate_sha256(app: Path) -> str:
    with tempfile.TemporaryDirectory(prefix="scrybe-continuity-cert-") as directory:
        prefix = Path(directory) / "certificate"
        observe(["codesign", "-d", f"--extract-certificates={prefix}", str(app)])
        leaf = Path(f"{prefix}0")
        if not leaf.is_file():
            raise ProbeError(f"codesign extracted no leaf certificate from {app}")
        return hashlib.sha256(leaf.read_bytes()).hexdigest()


def entitlements(app: Path) -> dict[str, Any]:
    output = observe(["codesign", "-d", "--entitlements", ":-", str(app)])
    start = output.find("<?xml")
    if start == -1:
        return {}
    try:
        value = plistlib.loads(output[start:].encode())
    except plistlib.InvalidFileException as error:
        raise ProbeError(f"codesign returned invalid entitlements for {app}") from error
    if not isinstance(value, dict):
        raise ProbeError(f"codesign returned non-dictionary entitlements for {app}")
    return value


def version(value: object, app: Path) -> tuple[int, ...]:
    if not isinstance(value, str) or VERSION.fullmatch(value) is None:
        raise ProbeError(f"{app} has invalid CFBundleShortVersionString {value!r}")
    return tuple(int(part) for part in value.split("."))


def file_sha256(path: Path) -> str:
    try:
        return hashlib.sha256(path.read_bytes()).hexdigest()
    except OSError as error:
        raise ProbeError(f"cannot read data sentinel {path}: {error}") from error


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--before", required=True, type=Path, help="installed app before update"
    )
    parser.add_argument(
        "--after", required=True, type=Path, help="installed app after update"
    )
    parser.add_argument(
        "--sentinel", required=True, type=Path, help="existing user-data sentinel"
    )
    parser.add_argument(
        "--sentinel-sha256",
        required=True,
        help="SHA-256 captured from the sentinel before the update",
    )
    arguments = parser.parse_args()

    try:
        before_info = info(arguments.before)
        after_info = info(arguments.after)
        before_identifier = before_info.get("CFBundleIdentifier")
        after_identifier = after_info.get("CFBundleIdentifier")
        if before_identifier != after_identifier:
            raise ProbeError(
                f"bundle identifier changed from {before_identifier!r} to {after_identifier!r}"
            )
        before_version = version(
            before_info.get("CFBundleShortVersionString"), arguments.before
        )
        after_version = version(
            after_info.get("CFBundleShortVersionString"), arguments.after
        )
        if after_version <= before_version:
            raise ProbeError(
                f"application version did not advance: {before_version!r} -> {after_version!r}"
            )
        before_requirement = requirement(arguments.before)
        after_requirement = requirement(arguments.after)
        if before_requirement != after_requirement:
            raise ProbeError(
                "designated requirement changed; macOS may treat the update as a different "
                f"application: {before_requirement!r} -> {after_requirement!r}"
            )
        before_certificate = certificate_sha256(arguments.before)
        after_certificate = certificate_sha256(arguments.after)
        if before_certificate != after_certificate:
            raise ProbeError(
                "code-signing certificate changed: "
                f"{before_certificate} -> {after_certificate}"
            )
        if entitlements(arguments.before) != entitlements(arguments.after):
            raise ProbeError("signed entitlements changed across the update")
        sentinel = file_sha256(arguments.sentinel)
        if sentinel != arguments.sentinel_sha256.lower():
            raise ProbeError(
                f"user-data sentinel changed: {sentinel} != {arguments.sentinel_sha256.lower()}"
            )
    except ProbeError as error:
        print(f"updater continuity FAILED: {error}")
        return 1

    print(f"bundle identifier: {before_identifier}")
    print(
        f"version: {'.'.join(map(str, before_version))} -> {'.'.join(map(str, after_version))}"
    )
    print(f"designated requirement: {before_requirement}")
    print(f"certificate SHA-256: {before_certificate}")
    print(f"data sentinel SHA-256: {sentinel}")
    print("updater continuity: ok")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
