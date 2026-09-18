#!/usr/bin/env python3
# Copyright 2026 Mathews Tom
# Licensed under the Apache License, Version 2.0 (the "License");
"""Verify the explicit trust contract for a community macOS app bundle.

This profile is deliberately different from ``check-signed-artifact.py``.
That existing gate requires Apple Developer ID signing, notarization, and a
Gatekeeper ``accepted`` verdict. The community profile requires a stable,
project-controlled self-signed identity and records Gatekeeper ``rejected`` as
an expected platform-trust limitation. It never presents rejection as Apple
acceptance.

The certificate fingerprint is public release metadata. The private key stays
outside the repository and is required only when producing an artifact.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import plistlib
import re
import subprocess
import sys
import tempfile
from dataclasses import dataclass
from pathlib import Path
from typing import Any

HELD = 0
FAILED = 1
UNDETERMINABLE = 2

AUTHORITY = re.compile(r"^Authority=(?P<authority>.+)$", re.MULTILINE)
ADHOC_SIGNATURE = re.compile(r"^Signature=adhoc$", re.MULTILINE)
SPCTL_VERDICT = re.compile(r"^.*: (?P<verdict>accepted|rejected)\s*$", re.MULTILINE)
TICKET_ABSENT = "does not have a ticket stapled to it"


class Undeterminable(Exception):
    """A required tool or machine-readable observation was unavailable."""


@dataclass(frozen=True)
class Policy:
    identity: str
    certificate_sha256: str
    bundle_identifier: str
    apple_trusted: bool
    notarized: bool
    expected_gatekeeper_verdict: str


@dataclass(frozen=True)
class Outcome:
    check: str
    status: int
    reason: str

    @property
    def label(self) -> str:
        return {HELD: "held", FAILED: "FAILED", UNDETERMINABLE: "UNDETERMINABLE"}[
            self.status
        ]


def observe(command: list[str]) -> tuple[str, int]:
    try:
        result = subprocess.run(command, capture_output=True, text=True, check=False)
    except OSError as error:
        raise Undeterminable(f"cannot run `{' '.join(command)}`: {error}") from error
    return f"{result.stdout}{result.stderr}", result.returncode


def load_policy(path: Path) -> Policy:
    try:
        raw: Any = json.loads(path.read_text(encoding="utf-8"))
        identity = raw["identity"]
        fingerprint = raw["certificate_sha256"]
        bundle_identifier = raw["bundle_identifier"]
        apple_trusted = raw["apple_trusted"]
        notarized = raw["notarized"]
        expected_gatekeeper_verdict = raw["expected_gatekeeper_verdict"]
    except (OSError, json.JSONDecodeError, KeyError, TypeError) as error:
        raise Undeterminable(
            f"cannot read community signing policy {path}: {error}"
        ) from error
    if not isinstance(identity, str) or not identity:
        raise Undeterminable("community signing policy has no non-empty `identity`")
    if not isinstance(bundle_identifier, str) or not bundle_identifier:
        raise Undeterminable(
            "community signing policy has no non-empty `bundle_identifier`"
        )
    if not isinstance(fingerprint, str) or not re.fullmatch(
        r"[0-9a-fA-F]{64}", fingerprint
    ):
        raise Undeterminable(
            "community signing policy `certificate_sha256` must contain 64 hex digits"
        )
    if apple_trusted is not False or notarized is not False:
        raise Undeterminable(
            "community signing policy must declare `apple_trusted` and `notarized` false"
        )
    if expected_gatekeeper_verdict != "rejected":
        raise Undeterminable(
            "community signing policy must declare Gatekeeper verdict `rejected`"
        )
    return Policy(
        identity,
        fingerprint.lower(),
        bundle_identifier,
        apple_trusted,
        notarized,
        expected_gatekeeper_verdict,
    )


def internal_signature(artifact: Path, policy: Policy) -> Outcome:
    output, status = observe(
        ["codesign", "--verify", "--deep", "--strict", "--verbose=2", str(artifact)]
    )
    if status != 0:
        return Outcome(
            "internal code signature",
            FAILED,
            f"codesign rejected the artifact (exit {status}): {output.strip()!r}",
        )

    details, _ = observe(["codesign", "-dvvv", str(artifact)])
    if ADHOC_SIGNATURE.search(details):
        return Outcome(
            "internal code signature",
            FAILED,
            "the artifact is ad-hoc signed and therefore has no stable project identity",
        )
    authorities = [match.group("authority") for match in AUTHORITY.finditer(details)]
    if authorities != [policy.identity]:
        return Outcome(
            "internal code signature",
            FAILED,
            f"embedded authorities are {authorities!r}, expected exactly [{policy.identity!r}]",
        )
    return Outcome(
        "internal code signature",
        HELD,
        f"the signature is valid and names {policy.identity!r}",
    )


def embedded_certificate(artifact: Path, policy: Policy) -> Outcome:
    with tempfile.TemporaryDirectory(prefix="scrybe-codesign-cert-") as directory:
        prefix = Path(directory) / "certificate"
        output, status = observe(
            ["codesign", "-d", f"--extract-certificates={prefix}", str(artifact)]
        )
        leaf = Path(f"{prefix}0")
        if status != 0 or not leaf.is_file():
            raise Undeterminable(
                "codesign did not extract the embedded leaf certificate "
                f"(exit {status}): {output.strip()!r}"
            )
        actual = hashlib.sha256(leaf.read_bytes()).hexdigest()
    if actual != policy.certificate_sha256:
        return Outcome(
            "embedded certificate fingerprint",
            FAILED,
            f"leaf SHA-256 is {actual}, expected {policy.certificate_sha256}",
        )
    return Outcome(
        "embedded certificate fingerprint",
        HELD,
        f"leaf SHA-256 matches {policy.certificate_sha256}",
    )


def bundle_identity(bundle: Path, policy: Policy) -> Outcome:
    info_path = bundle / "Contents" / "Info.plist"
    try:
        with info_path.open("rb") as handle:
            info = plistlib.load(handle)
    except (OSError, plistlib.InvalidFileException) as error:
        raise Undeterminable(f"cannot read {info_path}: {error}") from error
    actual = info.get("CFBundleIdentifier")
    if actual != policy.bundle_identifier:
        return Outcome(
            "bundle identity",
            FAILED,
            f"CFBundleIdentifier is {actual!r}, expected {policy.bundle_identifier!r}",
        )
    requirement, status = observe(["codesign", "-d", "-r-", str(bundle)])
    if status != 0 or policy.bundle_identifier not in requirement:
        return Outcome(
            "bundle identity",
            FAILED,
            "the designated requirement does not bind the expected bundle identifier: "
            f"{requirement.strip()!r}",
        )
    return Outcome(
        "bundle identity",
        HELD,
        f"bundle identifier and designated requirement bind {policy.bundle_identifier!r}",
    )


def expected_gatekeeper_rejection(bundle: Path) -> Outcome:
    output, status = observe(["spctl", "-a", "-vv", "--type", "execute", str(bundle)])
    verdict = SPCTL_VERDICT.search(output)
    if verdict is None:
        raise Undeterminable(
            f"spctl printed no accepted/rejected verdict (exit {status}): {output.strip()!r}"
        )
    actual = verdict.group("verdict")
    if actual != "rejected":
        return Outcome(
            "declared Gatekeeper state",
            FAILED,
            f"Gatekeeper says {actual!r}; the community profile must not imply Apple trust",
        )
    return Outcome(
        "declared Gatekeeper state",
        HELD,
        "Gatekeeper says 'rejected', the documented first-launch state for this "
        "self-signed and unnotarized distribution",
    )


def no_notarization_ticket(bundle: Path) -> Outcome:
    output, status = observe(["xcrun", "stapler", "validate", str(bundle)])
    if TICKET_ABSENT not in output:
        return Outcome(
            "declared notarization state",
            FAILED,
            "the artifact did not report the required no-ticket state "
            f"(stapler exit {status}): {output.strip()!r}",
        )
    return Outcome(
        "declared notarization state",
        HELD,
        "Apple reports no stapled notarization ticket, matching the community release disclosure",
    )


def mounted_dmg_outcomes(dmg: Path, policy: Policy) -> list[Outcome]:
    verified, verify_status = observe(["hdiutil", "verify", str(dmg)])
    if verify_status != 0:
        return [
            Outcome(
                "DMG integrity",
                FAILED,
                f"hdiutil rejected the disk image: {verified.strip()!r}",
            )
        ]

    outcomes = [
        Outcome("DMG integrity", HELD, "hdiutil verified every disk-image checksum"),
        internal_signature(dmg, policy),
        embedded_certificate(dmg, policy),
    ]
    with tempfile.TemporaryDirectory(prefix="scrybe-community-dmg-") as directory:
        mountpoint = Path(directory) / "mounted"
        mountpoint.mkdir()
        try:
            attached = subprocess.run(
                [
                    "hdiutil",
                    "attach",
                    "-readonly",
                    "-nobrowse",
                    "-mountpoint",
                    str(mountpoint),
                    "-plist",
                    str(dmg),
                ],
                capture_output=True,
                check=False,
            )
        except OSError as error:
            raise Undeterminable(f"cannot run hdiutil attach: {error}") from error
        if attached.returncode != 0:
            raise Undeterminable(
                "cannot mount DMG: "
                + attached.stderr.decode("utf-8", errors="replace").strip()
            )
        try:
            try:
                plistlib.loads(attached.stdout)
            except plistlib.InvalidFileException as error:
                raise Undeterminable(
                    "hdiutil attach returned an invalid plist"
                ) from error
            app = mountpoint / "Scrybe.app"
            applications = mountpoint / "Applications"
            if not app.is_dir() or not applications.is_symlink():
                outcomes.append(
                    Outcome(
                        "DMG install layout",
                        FAILED,
                        "mounted image must contain Scrybe.app and an Applications symlink",
                    )
                )
            elif applications.readlink() != Path("/Applications"):
                outcomes.append(
                    Outcome(
                        "DMG install layout",
                        FAILED,
                        f"Applications symlink points to {applications.readlink()}, not /Applications",
                    )
                )
            else:
                outcomes.append(
                    Outcome(
                        "DMG install layout",
                        HELD,
                        "mounted image contains Scrybe.app and an /Applications shortcut",
                    )
                )
                outcomes.extend(
                    [
                        internal_signature(app, policy),
                        embedded_certificate(app, policy),
                        bundle_identity(app, policy),
                    ]
                )
        finally:
            detached, detach_status = observe(["hdiutil", "detach", str(mountpoint)])
            if detach_status != 0:
                raise Undeterminable(f"cannot detach mounted DMG: {detached.strip()!r}")
    return outcomes


def report(bundle: Path, dmg: Path | None, outcomes: list[Outcome]) -> int:
    print(f"app artifact: {bundle}")
    if dmg is not None:
        print(f"DMG artifact: {dmg}")
    print("trust profile: community (self-signed, unnotarized)")
    print()
    for outcome in outcomes:
        print(f"  {outcome.label:<15} {outcome.check}")
        print(f"                  {outcome.reason}")
        print()
    if all(outcome.status == HELD for outcome in outcomes):
        print(f"community-artifact assertions: ok — {len(outcomes)} held")
        return HELD
    if any(outcome.status == FAILED for outcome in outcomes):
        print("community-artifact assertions FAILED")
        return FAILED
    print("community-artifact assertions UNDETERMINABLE")
    return UNDETERMINABLE


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--bundle", required=True, type=Path, help="the .app bundle to verify"
    )
    parser.add_argument("--dmg", type=Path, help="the optional signed DMG to verify")
    parser.add_argument(
        "--policy",
        type=Path,
        default=Path("packaging/macos-app/community-release.json"),
        help="public community signing policy",
    )
    arguments = parser.parse_args()

    if sys.platform != "darwin":
        print("community artifact verification requires macOS", file=sys.stderr)
        return UNDETERMINABLE
    if not arguments.bundle.is_dir():
        print(f"no bundle at {arguments.bundle}", file=sys.stderr)
        return UNDETERMINABLE
    if arguments.dmg is not None and not arguments.dmg.is_file():
        print(f"no DMG at {arguments.dmg}", file=sys.stderr)
        return UNDETERMINABLE
    try:
        policy = load_policy(arguments.policy)
        outcomes = [
            internal_signature(arguments.bundle, policy),
            embedded_certificate(arguments.bundle, policy),
            bundle_identity(arguments.bundle, policy),
            expected_gatekeeper_rejection(arguments.bundle),
            no_notarization_ticket(arguments.bundle),
        ]
        if arguments.dmg is not None:
            outcomes.extend(mounted_dmg_outcomes(arguments.dmg, policy))
    except Undeterminable as error:
        print(f"community-artifact assertions UNDETERMINABLE: {error}", file=sys.stderr)
        return UNDETERMINABLE
    return report(arguments.bundle, arguments.dmg, outcomes)


if __name__ == "__main__":
    raise SystemExit(main())
