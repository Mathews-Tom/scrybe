#!/usr/bin/env python3
# Copyright 2026 Mathews Tom
# Licensed under the Apache License, Version 2.0 (the "License");
# you may not use this file except in compliance with the License.
# You may obtain a copy of the License at
#     https://www.apache.org/licenses/LICENSE-2.0
"""Signature, Gatekeeper, and notarization assertions for a macOS bundle.

None of these can pass without a real credential, and that is the point.
They exist and are reviewable now, before any credential is available,
so that the first run with one is a run against checks somebody read
rather than checks written in the same hour they were needed.

## What was measured, and why each assertion is shaped the way it is

An ad-hoc-signed bundle was built and put through the obvious commands:

    codesign --verify --deep --strict <bundle>   → exit 0
    codesign -dvvv <bundle>                      → Signature=adhoc
                                                   TeamIdentifier=not set
                                                   no Authority line at all
    spctl -a -vv --type execute <bundle>         → "rejected", exit 3
    xcrun stapler validate <bundle>              → "does not have a ticket
                                                    stapled to it", exit 65

So `codesign --verify` returns success for an artifact Gatekeeper
refuses to run. It answers "is this signature internally consistent",
which an ad-hoc signature is; it does not answer "who signed this", which
is the only question that matters for distribution. A gate built on it
certifies an unshippable artifact. That is why nothing below calls it.

The `spctl` exit status is worse than useless: it was measured at 3 here
and at 0 on the same `rejected` verdict in the lane's own earlier
measurement. An assertion written against that exit status would have
passed there and failed here while the artifact was equally unshippable
in both. The verdict is in the text, so the text is what gets parsed.

`stapler validate` printed a plain denial and exited 65 — a case where
the text and the status agreed — but the status alone cannot distinguish
"no ticket" from "stapler is not installed", and the earlier `spctl`
measurement is enough reason not to trust a status to carry a verdict
anywhere in this file.

## The three assertions

1. **Signature.** Reads `Authority` and `TeamIdentifier` out of
   `codesign -dvvv` and requires a Developer ID Application identity with
   a ten-character team identifier that the authority line and the
   `TeamIdentifier` field agree on. Never `codesign --verify`.
2. **Gatekeeper.** Parses the verdict word out of `spctl -a -vv --type
   execute` and requires `accepted` from a notarized Developer ID source.
   Never the exit status. A run that produces no verdict at all is
   reported as undeterminable rather than as acceptance.
3. **Notarization.** Reads the status Apple returns for a submission from
   `xcrun notarytool info` and requires exactly `Accepted`, and reads the
   presence of a stapled ticket out of `xcrun stapler validate`. Never a
   submission's exit code, and never a file or flag this repository's own
   tooling wrote — a ticket is a CMS blob Apple issued, which is why its
   presence is worth reading and a pipeline's own "notarized: true" is
   not.

## Exit status

- `0` — every requested assertion held against the artifact.
- `1` — an assertion was made and the artifact failed it. Something is
  wrong with the artifact or with how it was signed.
- `2` — an assertion could not be made: a tool is missing, a tool's
  output had no verdict in it, the bundle is not there. Never read as
  success; a check that could not run has established nothing.
- `3` — a required credential is absent, so the assertion was not
  attempted. Distinct from `1` on purpose: "nobody has configured signing
  yet" and "this signed artifact is bad" call for entirely different
  responses, and collapsing them is how an unsigned artifact ends up
  described as one that passed.

## Credentials

The notarization assertion needs a submission identifier (`--submission`)
and one of these credential sets, none of which is stored in this
repository:

- `NOTARY_KEYCHAIN_PROFILE` — a profile stored by
  `xcrun notarytool store-credentials`.
- `NOTARY_API_KEY_ID`, `NOTARY_API_ISSUER_ID`, `NOTARY_API_KEY_PATH` — an
  App Store Connect API key.
- `NOTARY_APPLE_ID`, `NOTARY_TEAM_ID`, `NOTARY_PASSWORD` — an Apple ID
  with an app-specific password.

Only the names of these variables are ever printed. No value is logged,
including on failure.

Run locally:

    python3 scripts/check-signed-artifact.py --bundle ./scrybe.app
    python3 scripts/check-signed-artifact.py --bundle ./scrybe.app \\
        --assert signature --assert gatekeeper
    python3 scripts/check-signed-artifact.py --bundle ./scrybe.app \\
        --assert notarization --submission <uuid>

With no credential present, every assertion fails and says which
credential it wanted. That failing run is the reviewable artifact until a
credential exists.
"""

from __future__ import annotations

import argparse
import json
import os
import re
import subprocess
import sys
from dataclasses import dataclass
from pathlib import Path

# A Developer ID Application leaf authority, and the team identifier
# Apple encodes in its common name. Ten upper-case alphanumerics is the
# Apple team identifier format; `not set` is what `codesign` prints for
# an ad-hoc signature and is the string this assertion exists to reject.
DEVELOPER_ID_AUTHORITY = re.compile(
    r"^Authority=Developer ID Application: (?P<name>.+) \((?P<team>[A-Z0-9]{10})\)$"
)
TEAM_IDENTIFIER = re.compile(r"^TeamIdentifier=(?P<team>.+)$", re.MULTILINE)

# `Signature=adhoc` is printed only for an ad-hoc signature. A bundle
# signed by a real identity prints `Signature size=<n>` instead and no
# `Signature=<kind>` line at all — measured on a bundle signed with the
# self-signed `scrybe-local-signing` identity, which printed an
# `Authority` line and no `Signature=` line. So this is read as an
# explanation when it is there and never required: requiring it made the
# assertion report a self-signed bundle as undeterminable instead of as
# the wrong kind of identity.
ADHOC_SIGNATURE = re.compile(r"^Signature=adhoc$", re.MULTILINE)

# Every authority line, Developer ID or not, so a refusal can say what
# did sign the artifact rather than only that the right thing did not.
ANY_AUTHORITY = re.compile(r"^Authority=(?P<authority>.+)$", re.MULTILINE)

# `spctl` prints `<path>: <verdict>` and, on acceptance, a `source=` line
# naming what admitted it. Both are parsed; neither is inferred from the
# exit status.
SPCTL_VERDICT = re.compile(r"^.*: (?P<verdict>accepted|rejected)\s*$", re.MULTILINE)
SPCTL_SOURCE = re.compile(r"^source=(?P<source>.+)$", re.MULTILINE)

# The only `spctl` source that means "Apple notarized this". A Developer
# ID signature that was never notarized reports `source=Developer ID`
# and is refused: recent macOS will not run it without a prompt the
# distribution story cannot include.
NOTARIZED_SOURCE = "Notarized Developer ID"

# The status `notarytool` reports for a submission Apple accepted. Every
# other value — `In Progress`, `Invalid`, `Rejected` — is a failure, and
# an absent `status` key is undeterminable rather than either.
ACCEPTED_STATUS = "Accepted"

# What `stapler validate` says when a ticket is there and when it is not.
# Matched positively in both directions, so output matching neither is
# reported as undeterminable instead of being read as one of them.
# `notarytool` refuses locally, before any request, when the credential
# it was pointed at is not usable — measured as exit 69 and `Error: No
# Keychain password item found for profile: <name>` against a profile
# name no keychain holds. That is a credential problem, not an
# unmeasurable one, and reporting it as unmeasurable would bury the one
# thing the operator can act on.
NOTARY_CREDENTIAL_ERRORS = (
    "No Keychain password item found",
    "Failed to authenticate",
    "Unable to validate your credentials",
)

TICKET_PRESENT = "The validate action worked!"
TICKET_ABSENT = "does not have a ticket stapled to it"

CREDENTIAL_SETS: dict[str, tuple[str, ...]] = {
    "keychain profile": ("NOTARY_KEYCHAIN_PROFILE",),
    "App Store Connect API key": (
        "NOTARY_API_KEY_ID",
        "NOTARY_API_ISSUER_ID",
        "NOTARY_API_KEY_PATH",
    ),
    "Apple ID with an app-specific password": (
        "NOTARY_APPLE_ID",
        "NOTARY_TEAM_ID",
        "NOTARY_PASSWORD",
    ),
}

HELD, FAILED, UNDETERMINABLE, NO_CREDENTIAL = 0, 1, 2, 3


@dataclass
class Outcome:
    """One assertion's result, and the reason behind it."""

    assertion: str
    status: int
    reason: str

    @property
    def label(self) -> str:
        return {
            HELD: "held",
            FAILED: "FAILED",
            UNDETERMINABLE: "UNDETERMINABLE",
            NO_CREDENTIAL: "NO CREDENTIAL",
        }[self.status]


def observe(command: list[str]) -> tuple[str, int]:
    """Runs a tool and returns everything it said, plus its exit status.

    The status is returned because it is worth printing when a tool could
    not run at all. No assertion in this file derives a verdict from it;
    `codesign`, `spctl`, and `stapler` were each measured returning a
    status that contradicts their own output.
    """
    try:
        result = subprocess.run(command, capture_output=True, text=True, check=False)
    except OSError as error:
        raise Undeterminable(f"cannot run `{' '.join(command)}`: {error}") from error
    return f"{result.stdout}{result.stderr}", result.returncode


class Undeterminable(Exception):
    """A tool could not run or said nothing a verdict can be read from."""


def first_line(said: str) -> str:
    """The first non-empty line a tool printed.

    Quoted into messages so a reader sees the tool's own words rather
    than only this file's interpretation of them. Credential *values*
    never reach here: every message that could carry one names the
    environment variable instead, and `notarytool` prints profile names
    and error text, not secrets.
    """
    for line in said.splitlines():
        if line.strip():
            return line.strip()
    return "nothing at all"


def signature(bundle: Path) -> Outcome:
    """Requires a Developer ID Application identity, read by name."""
    name = ASSERTION_NAMES["signature"]
    said, status = observe(["codesign", "-dvvv", str(bundle)])
    if "code object is not signed at all" in said:
        return Outcome(
            name,
            NO_CREDENTIAL,
            "this bundle carries no signature at all, so no identity signed it. "
            "A Developer ID Application certificate in the keychain and a "
            "`codesign --sign` run against it are what this assertion is waiting for.",
        )
    authorities = [
        match
        for line in said.splitlines()
        if (match := DEVELOPER_ID_AUTHORITY.match(line.strip())) is not None
    ]
    declared = TEAM_IDENTIFIER.search(said)
    team = declared.group("team") if declared else "absent"
    if not authorities:
        signers = [match.group("authority") for match in ANY_AUTHORITY.finditer(said)]
        describe = (
            "an ad-hoc signature, which names nobody"
            if ADHOC_SIGNATURE.search(said)
            else f"authorities {signers}"
            if signers
            else f"no authority at all (`codesign -dvvv` exited {status})"
        )
        return Outcome(
            name,
            NO_CREDENTIAL,
            f"no Developer ID Application authority signed this bundle: it carries "
            f"{describe}, with TeamIdentifier={team}. Note that `codesign --verify "
            "--deep --strict` returns success for exactly this artifact, which is why "
            "this assertion does not call it. A Developer ID Application certificate "
            "is what is missing.",
        )
    if team != authorities[0].group("team"):
        return Outcome(
            name,
            FAILED,
            f"the authority line names team {authorities[0].group('team')} but "
            f"TeamIdentifier reports {team}; a signature whose two statements of its "
            "own team disagree cannot be attributed to anybody",
        )
    return Outcome(
        name,
        HELD,
        f"signed by Developer ID Application: {authorities[0].group('name')} "
        f"(team {team})",
    )


def gatekeeper(bundle: Path) -> Outcome:
    """Requires the verdict word, never the exit status."""
    name = ASSERTION_NAMES["gatekeeper"]
    said, status = observe(["spctl", "-a", "-vv", "--type", "execute", str(bundle)])
    verdict = SPCTL_VERDICT.search(said)
    if verdict is None:
        raise Undeterminable(
            f"`spctl -a -vv --type execute` printed no accepted/rejected verdict "
            f"(exit {status}); it said {said.strip()!r}. A missing verdict is not "
            "acceptance, so this is reported as undeterminable rather than passed"
        )
    if verdict.group("verdict") != "accepted":
        return Outcome(
            name,
            NO_CREDENTIAL,
            f"Gatekeeper says {verdict.group('verdict')} (the exit status was {status}, "
            "which carries no verdict: the same `rejected` was measured with exit 0 and "
            "with exit 3). A Developer ID signature and a notarization ticket are what "
            "would change this answer.",
        )
    source = SPCTL_SOURCE.search(said)
    origin = source.group("source").strip() if source else "unstated"
    if origin != NOTARIZED_SOURCE:
        return Outcome(
            name,
            FAILED,
            f"Gatekeeper accepted this artifact from source {origin!r} rather than "
            f"{NOTARIZED_SOURCE!r}; an acceptance that does not rest on notarization "
            "will not survive distribution to a machine that has never seen it",
        )
    return Outcome(name, HELD, f"Gatekeeper accepted it, source {origin!r}")


def missing_credentials() -> list[str]:
    """Names every credential set that is not fully present.

    Values are never read into the message. A set counts as present only
    when every variable in it is set and non-empty, so a half-configured
    key reads as absent rather than being attempted and failing inside
    `notarytool` with something less legible.
    """
    complaints: list[str] = []
    for label, variables in CREDENTIAL_SETS.items():
        absent = [name for name in variables if not os.environ.get(name)]
        if not absent:
            return []
        complaints.append(f"{label}: {', '.join(absent)} not set")
    return complaints


def notarytool_credential_arguments() -> list[str]:
    if os.environ.get("NOTARY_KEYCHAIN_PROFILE"):
        return ["--keychain-profile", os.environ["NOTARY_KEYCHAIN_PROFILE"]]
    if os.environ.get("NOTARY_API_KEY_ID"):
        return [
            "--key-id",
            os.environ["NOTARY_API_KEY_ID"],
            "--issuer",
            os.environ["NOTARY_API_ISSUER_ID"],
            "--key",
            os.environ["NOTARY_API_KEY_PATH"],
        ]
    return [
        "--apple-id",
        os.environ["NOTARY_APPLE_ID"],
        "--team-id",
        os.environ["NOTARY_TEAM_ID"],
        "--password",
        os.environ["NOTARY_PASSWORD"],
    ]


def notarization(bundle: Path, submission: str | None) -> Outcome:
    """Requires Apple's own status, and a ticket Apple issued."""
    name = ASSERTION_NAMES["notarization"]
    absent = missing_credentials()
    if absent:
        return Outcome(
            name,
            NO_CREDENTIAL,
            "no notarization credential is present, so Apple's status for this "
            "artifact was never requested and nothing is claimed about it. One "
            "complete set is required:\n      " + "\n      ".join(absent),
        )
    if submission is None:
        return Outcome(
            name,
            NO_CREDENTIAL,
            "a notarization credential is present but no submission identifier was "
            "given, so there is nothing to ask Apple about. Pass `--submission <uuid>` "
            "from the `xcrun notarytool submit` that uploaded this artifact.",
        )
    return apple_status(name, bundle, submission)


def apple_status(name: str, bundle: Path, submission: str) -> Outcome:
    said, status = observe(
        [
            "xcrun",
            "notarytool",
            "info",
            submission,
            "--output-format",
            "json",
            *notarytool_credential_arguments(),
        ]
    )
    if any(marker in said for marker in NOTARY_CREDENTIAL_ERRORS):
        return Outcome(
            name,
            NO_CREDENTIAL,
            "a notarization credential was configured but `xcrun notarytool` would not "
            f"use it, so Apple was never asked and nothing is claimed. It said: "
            f"{first_line(said)!r}",
        )
    try:
        reported = json.loads(said).get("status")
    except json.JSONDecodeError as error:
        raise Undeterminable(
            f"`xcrun notarytool info` did not return JSON (exit {status}); it said "
            f"{first_line(said)!r} ({error})"
        ) from error
    if reported is None:
        raise Undeterminable(
            f"`xcrun notarytool info` returned no `status` for submission {submission} "
            f"(exit {status}); an absent status is not an acceptance"
        )
    if reported != ACCEPTED_STATUS:
        return Outcome(
            name,
            FAILED,
            f"Apple reports status {reported!r} for submission {submission}, not "
            f"{ACCEPTED_STATUS!r}",
        )
    return ticket(name, bundle, submission)


def ticket(name: str, bundle: Path, submission: str) -> Outcome:
    said, status = observe(["xcrun", "stapler", "validate", str(bundle)])
    if TICKET_PRESENT in said:
        return Outcome(
            name,
            HELD,
            f"Apple reports {ACCEPTED_STATUS!r} for submission {submission} and a "
            "ticket Apple issued is stapled to the bundle",
        )
    if TICKET_ABSENT in said:
        return Outcome(
            name,
            FAILED,
            f"Apple accepted submission {submission} but no ticket is stapled to this "
            "bundle, so a machine that cannot reach Apple will refuse it. Run "
            "`xcrun stapler staple` against this artifact.",
        )
    raise Undeterminable(
        f"`xcrun stapler validate` said {said.strip()!r} (exit {status}), which states "
        "neither that a ticket is present nor that it is absent"
    )


ASSERTION_NAMES = {
    "signature": "signature (Developer ID identity)",
    "gatekeeper": "gatekeeper (parsed verdict)",
    "notarization": "notarization (Apple's status + stapled ticket)",
}
ASSERTIONS = tuple(ASSERTION_NAMES)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--bundle", type=Path, required=True, help="the `.app` bundle to assert against"
    )
    parser.add_argument(
        "--assert",
        dest="assertions",
        action="append",
        choices=ASSERTIONS,
        default=None,
        help="run one assertion; repeatable. Default: all three, in order.",
    )
    parser.add_argument(
        "--submission",
        default=None,
        help="the notarytool submission identifier this artifact was uploaded under",
    )
    arguments = parser.parse_args()

    if sys.platform != "darwin":
        print(
            "these assertions read macOS code-signing tools; nothing else can run them",
            file=sys.stderr,
        )
        return UNDETERMINABLE
    if not arguments.bundle.is_dir():
        print(
            f"no bundle at {arguments.bundle}; there is nothing to assert against",
            file=sys.stderr,
        )
        return UNDETERMINABLE

    requested = arguments.assertions or list(ASSERTIONS)
    outcomes: list[Outcome] = []
    for assertion in requested:
        try:
            if assertion == "signature":
                outcomes.append(signature(arguments.bundle))
            elif assertion == "gatekeeper":
                outcomes.append(gatekeeper(arguments.bundle))
            else:
                outcomes.append(notarization(arguments.bundle, arguments.submission))
        except Undeterminable as error:
            outcomes.append(Outcome(ASSERTION_NAMES[assertion], UNDETERMINABLE, str(error)))
    return report(arguments.bundle, outcomes)


def report(bundle: Path, outcomes: list[Outcome]) -> int:
    print(f"artifact: {bundle}")
    print()
    for outcome in outcomes:
        print(f"  {outcome.label:<15} {outcome.assertion}")
        print(f"                  {outcome.reason}")
        print()
    unmet = [outcome for outcome in outcomes if outcome.status != HELD]
    if not unmet:
        print(f"signed-artifact assertions: ok — {len(outcomes)} held")
        return HELD
    # Precedence, stated rather than arithmetic: the numbers are labels,
    # not severities, and taking their maximum reported a real failure
    # under the credential-absent code. An artifact that failed an
    # assertion dominates, because it is the most actionable answer; a
    # check that could not run dominates credential-absence, because
    # "nobody configured signing" cannot be offered as the explanation
    # for something nobody managed to measure; and the credential-absent
    # code is reached only when every unmet assertion is credential-absent.
    if any(outcome.status == FAILED for outcome in unmet):
        print(
            f"signed-artifact assertions FAILED — {len(unmet)} of {len(outcomes)} did "
            "not hold, and at least one is the artifact being wrong rather than a "
            "credential being absent. Read each reason above."
        )
        return FAILED
    if any(outcome.status == UNDETERMINABLE for outcome in unmet):
        print(
            f"signed-artifact assertions UNDETERMINABLE — {len(unmet)} of "
            f"{len(outcomes)} could not be measured. Nothing is claimed either way; a "
            "check that could not run has established nothing."
        )
        return UNDETERMINABLE
    print(
        f"signed-artifact assertions NOT MET — {len(unmet)} of {len(outcomes)} were "
        "not attempted or could not pass because a credential is absent. This is "
        "not a defect in the artifact and not a defect in these checks: nothing "
        "here has been told who signs Scrybe. Until it is, no path may treat this "
        "artifact as shippable."
    )
    return NO_CREDENTIAL


if __name__ == "__main__":
    sys.exit(main())
