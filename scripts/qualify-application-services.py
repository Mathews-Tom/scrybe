#!/usr/bin/env python3
# Copyright 2026 Mathews Tom
# Licensed under the Apache License, Version 2.0 (the "License");
# you may not use this file except in compliance with the License.
# You may obtain a copy of the License at
#     https://www.apache.org/licenses/LICENSE-2.0
"""Parity qualification for the shared application services.

Builds one deterministic fixture session tree and drives the real
`scrybe` binary against it, asserting that the CLI and the read-only
agent surface report the *same* sessions with the *same* classification
— which is the point of having one service layer rather than three
private filesystem walks.

What it covers: list, search, show, configuration validation, doctor
findings, record start and stop, and every read-only agent tool.

Determinism:

- the fixture tree is written from literals, never from a recording;
- every session identity, title, and timestamp is fixed;
- the only recording performed uses the synthetic capture source and the
  stub notes backend, so no microphone, no system-audio permission, no
  model, and no network is touched;
- `--hermetic` builds `scrybe` with `--no-default-features`, the
  air-gappable shape `scripts/check-egress-baseline.py` also audits;
- absolute paths are redacted from the emitted evidence, so two runs on
  two machines produce identical output.

Run locally:

    python3 scripts/qualify-application-services.py --hermetic --scenario parity

Exit status 0 means every check held. Exit status 1 means at least one
did; the failing checks are printed with what was expected and what was
observed.
"""

from __future__ import annotations

import argparse
import json
import re
import shutil
import subprocess
import sys
import tempfile
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any

REPO_ROOT = Path(__file__).resolve().parent.parent

# One fixture per session state the classifier distinguishes, so a
# regression in any branch shows up as a parity failure rather than as
# silence.
COMPLETE = "2026-04-29-1430-quarterly-review-01QUALIFYAAAAAAAAAAAAAAAA"
REPAIRABLE = "2026-04-28-0900-interrupted-call-01QUALIFYBBBBBBBBBBBBBBBB"
UNFINISHED = "2026-04-27-1100-abandoned-start-01QUALIFYCCCCCCCCCCCCCCCC"
FAILED = "2026-04-26-1600-corrupt-metadata-01QUALIFYDDDDDDDDDDDDDDDD"
NOT_A_SESSION = "not-a-session"

COMPLETE_META = """session_id = "01QUALIFYAAAAAAAAAAAAAAAA"
title = "Quarterly review"
started_at = "2026-04-29T14:30:00Z"
ended_at = "2026-04-29T15:12:00Z"
duration_secs = 2520

[providers]
stt = "whisper-local"
llm = "stub"
diarizer = "binary-channel"

[audio]
channels = 2
layout = "stereo:mic-l,system-r"
sample_rate = 48000
bitrate_bps = 32000
"""

COMPLETE_TRANSCRIPT = """# Quarterly review
*2026-04-29 14:30*

**Me** [00:00:00]: opening the quarterly review
**Them** [00:00:12]: revenue landed above the forecast
**Me** [00:00:30]: capacity is the constraint next quarter
"""

COMPLETE_NOTES = """## TL;DR
- revenue above forecast
- capacity is the next constraint
"""

# A hand-written configuration carrying a comment, a credential *name*,
# and a block the settings surface does not model. All three must
# survive a configuration read, and the credential must never be echoed.
CONFIG = """schema_version = 1

# One root so a backup tool has exactly one target.
[storage]
root = "{root}"
audio_format = "opus"

[record]
source = "synthetic"
llm = "stub"

[llm]
provider = "openai-compat"
base_url = "http://127.0.0.1:11434/v1"
model = "qwen3:8b"
api_key_env = "SCRYBE_QUALIFY_LLM_KEY"

[agent_access]
enabled = true

# Advanced: unmodelled by any settings surface.
[hooks.webhook]
url = "http://127.0.0.1:9000/scrybe"
timeout_ms = 3000
"""


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

    def record(self, check: str, expected: str, observed: str) -> None:
        self.entries.append(
            Evidence(check, expected, observed, expected == observed)
        )

    def assert_that(self, check: str, condition: bool, detail: str) -> None:
        self.entries.append(
            Evidence(check, "holds", detail if condition else f"FAILED: {detail}", condition)
        )

    @property
    def failures(self) -> list[Evidence]:
        return [entry for entry in self.entries if not entry.ok]


def build(hermetic: bool) -> Path:
    """Compiles the binary under test and returns its path."""
    args = ["cargo", "build", "-p", "scrybe", "--features", "agent-access"]
    if hermetic:
        args.append("--no-default-features")
    subprocess.run(args, cwd=REPO_ROOT, check=True)
    binary = REPO_ROOT / "target" / "debug" / "scrybe"
    if not binary.is_file():
        raise RuntimeError(f"built binary not found at {binary}")
    return binary


def write_fixture_tree(root: Path) -> None:
    """Writes one session per classifier branch, plus a non-session."""
    complete = root / COMPLETE
    complete.mkdir(parents=True)
    (complete / "meta.toml").write_text(COMPLETE_META)
    (complete / "transcript.md").write_text(COMPLETE_TRANSCRIPT)
    (complete / "notes.md").write_text(COMPLETE_NOTES)
    (complete / "audio.opus").write_bytes(b"")

    repairable = root / REPAIRABLE / "journal"
    repairable.mkdir(parents=True)
    (repairable / "manifest.toml").write_text("")

    unfinished = root / UNFINISHED / "journal"
    unfinished.mkdir(parents=True)
    (unfinished / "mic.f32").write_bytes(bytes(8))

    failed = root / FAILED
    failed.mkdir(parents=True)
    (failed / "meta.toml").write_text("session_id = \n")

    stray = root / NOT_A_SESSION
    stray.mkdir(parents=True)
    (stray / "README.md").write_text("not a meeting\n")

    (root / "model.gguf.partial").write_bytes(b"abc")


def cli(binary: Path, config: Path, root: Path, *args: str) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        [str(binary), *args, "--root", str(root)],
        cwd=REPO_ROOT,
        env={"PATH": "/usr/bin:/bin", "SCRYBE_CONFIG": str(config)},
        capture_output=True,
        text=True,
        stdin=subprocess.DEVNULL,
        check=False,
    )


def agent(binary: Path, config: Path, root: Path, requests: list[dict]) -> list[dict]:
    """Drives the read-only agent surface over stdio and returns replies."""
    payload = "".join(json.dumps(request) + "\n" for request in requests)
    completed = subprocess.run(
        [str(binary), "mcp", "--root", str(root)],
        cwd=REPO_ROOT,
        env={"PATH": "/usr/bin:/bin", "SCRYBE_CONFIG": str(config)},
        input=payload,
        capture_output=True,
        text=True,
        check=True,
    )
    return [json.loads(line) for line in completed.stdout.splitlines() if line.strip()]


def tool_payload(reply: dict) -> Any:
    """Unwraps one MCP tool envelope into the value it carries."""
    return json.loads(reply["result"]["content"][0]["text"])


def call(tool: str, request_id: int, **arguments: Any) -> dict:
    return {
        "jsonrpc": "2.0",
        "id": request_id,
        "method": "tools/call",
        "params": {"name": tool, "arguments": arguments},
    }


def parity(binary: Path, root: Path, config: Path) -> Run:
    """Runs the parity scenario and returns its evidence."""
    run = Run()

    # --- list ------------------------------------------------------
    listing = cli(binary, config, root, "list")
    run.record("list.exit_status", "0", str(listing.returncode))
    rows = [line for line in listing.stdout.splitlines() if line.startswith("2026-")]
    run.record("list.session_count", "4", str(len(rows)))
    run.assert_that(
        "list.omits_non_session_folder",
        NOT_A_SESSION not in listing.stdout,
        f"{NOT_A_SESSION} absent from the listing",
    )
    run.assert_that(
        "list.repairable_offers_repair",
        any(REPAIRABLE in row and "scrybe repair" in row for row in rows),
        "the recoverable session is told to run repair",
    )
    run.assert_that(
        "list.unfinished_offers_no_repair",
        any(UNFINISHED in row and "scrybe repair" not in row for row in rows),
        "the unrecoverable session is not told to run repair",
    )
    run.assert_that(
        "list.failed_is_reported_failed",
        any(FAILED in row and "FAILED" in row for row in rows),
        "the corrupt session is reported failed",
    )

    # --- search ----------------------------------------------------
    agent_search = agent(
        binary,
        config,
        root,
        [call("search_meetings", 1, query="capacity")],
    )
    hits = tool_payload(agent_search[0])
    run.record("search.transcript_match_count", "1", str(len(hits)))
    run.record(
        "search.transcript_match_folder",
        COMPLETE,
        hits[0]["folder"] if hits else "<none>",
    )

    incomplete_content = agent(
        binary,
        config,
        root,
        [call("search_meetings", 2, query="interrupted")],
    )
    by_name = tool_payload(incomplete_content[0])
    run.record("search.incomplete_matches_by_name", "1", str(len(by_name)))
    run.record(
        "search.incomplete_status",
        "unfinished",
        by_name[0]["status"] if by_name else "<none>",
    )

    # --- show ------------------------------------------------------
    shown = cli(binary, config, root, "show", "01QUALIFYAAAAAAAAAAAAAAAA")
    run.record("show.exit_status", "0", str(shown.returncode))
    run.assert_that(
        "show.renders_transcript",
        "capacity is the constraint next quarter" in shown.stdout,
        "the durable transcript is rendered",
    )
    run.assert_that(
        "show.renders_notes",
        "capacity is the next constraint" in shown.stdout,
        "the durable notes are rendered",
    )

    escaped = cli(binary, config, root, "show", str(root / COMPLETE))
    run.assert_that(
        "show.refuses_an_absolute_path",
        escaped.returncode != 0 and "absolute path" in escaped.stderr,
        "an absolute path is refused at the service boundary",
    )
    traversal = cli(binary, config, root, "show", "../../etc")
    run.assert_that(
        "show.refuses_traversal",
        traversal.returncode != 0,
        "a traversal identity is refused at the service boundary",
    )

    # --- configuration validation ----------------------------------
    doctor = cli(binary, config, root, "doctor")
    run.record("doctor.exit_status", "0", str(doctor.returncode))
    run.assert_that(
        "config.credential_name_never_echoed",
        "SCRYBE_QUALIFY_LLM_KEY" not in doctor.stdout,
        "no credential variable name reaches the diagnosis output",
    )
    run.assert_that(
        "config.unmodelled_block_survives",
        "[hooks.webhook]" in config.read_text(),
        "a block the settings surface does not model is still on disk",
    )
    run.assert_that(
        "config.comment_survives",
        "# One root so a backup tool has exactly one target." in config.read_text(),
        "a hand-written comment is still on disk",
    )

    # --- doctor findings -------------------------------------------
    run.assert_that(
        "doctor.reports_recoverable_session",
        REPAIRABLE in doctor.stdout,
        "the recoverable session appears in the diagnosis",
    )
    run.assert_that(
        "doctor.reports_orphaned_partial",
        "model.gguf.partial" in doctor.stdout,
        "the orphaned partial download appears in the diagnosis",
    )
    run.assert_that(
        "doctor.does_not_delete_the_partial",
        (root / "model.gguf.partial").exists(),
        "diagnosis left the partial download in place",
    )
    run.assert_that(
        "doctor.reports_local_egress",
        "llm egress: no egress" in doctor.stdout,
        "the loopback notes provider is reported as local",
    )

    # --- record start and stop -------------------------------------
    before = {path.name for path in root.iterdir()}
    recorded = subprocess.run(
        [
            str(binary),
            "rec",
            "--title",
            "Qualification recording",
            "--synthetic-secs",
            "1",
            "--consent",
            "quick",
            "--root",
            str(root),
        ],
        cwd=REPO_ROOT,
        env={"PATH": "/usr/bin:/bin", "SCRYBE_CONFIG": str(config)},
        input="y\n",
        capture_output=True,
        text=True,
        check=False,
    )
    run.record("record.exit_status", "0", str(recorded.returncode))
    created = sorted({path.name for path in root.iterdir()} - before)
    run.record("record.sessions_created", "1", str(len(created)))
    if created:
        session = root / created[0]
        for artifact in ("meta.toml", "transcript.md", "notes.md", "audio.opus"):
            run.assert_that(
                f"record.durable_{artifact}",
                (session / artifact).is_file(),
                f"{artifact} is durable after the stop",
            )
        run.assert_that(
            "record.journal_removed",
            not (session / "journal").exists(),
            "the journal is gone after a verified merge",
        )

    # --- every read-only agent tool --------------------------------
    replies = agent(
        binary,
        config,
        root,
        [
            {"jsonrpc": "2.0", "id": 10, "method": "initialize", "params": {}},
            {"jsonrpc": "2.0", "id": 11, "method": "ping"},
            {"jsonrpc": "2.0", "id": 12, "method": "tools/list"},
            call("list_recent_meetings", 13, limit=10),
            call("search_meetings", 14, query="quarterly"),
            call("get_meeting", 15, id=COMPLETE),
            call("get_meeting_notes", 16, id=COMPLETE),
            call("get_meeting_transcript", 17, id=COMPLETE),
            call("get_meeting", 18, id=REPAIRABLE),
            call("get_meeting_notes", 19, id=REPAIRABLE),
            call("get_meeting", 20, id="/etc/passwd"),
            call("repair_session", 21, id=COMPLETE),
        ],
    )
    answered = {reply["id"]: reply for reply in replies}

    tools = [tool["name"] for tool in answered[12]["result"]["tools"]]
    run.record(
        "agent.tool_surface",
        "get_meeting,get_meeting_notes,get_meeting_transcript,"
        "list_recent_meetings,search_meetings",
        ",".join(sorted(tools)),
    )
    run.assert_that(
        "agent.exposes_no_mutation_tool",
        not any(
            word in name
            for name in tools
            for word in ("repair", "write", "delete", "update", "regenerate", "set")
        ),
        "no advertised tool names a mutation",
    )
    run.assert_that(
        "agent.refuses_an_unknown_mutation_tool",
        answered[21]["result"]["isError"] is True,
        "a mutation tool call is refused",
    )
    run.assert_that(
        "agent.refuses_an_absolute_path_identity",
        answered[20]["result"]["isError"] is True,
        "an absolute-path identity is refused",
    )

    agent_listing = tool_payload(answered[13])
    run.record(
        "agent.list_matches_cli_count",
        str(len(rows) + len(created)),
        str(len(agent_listing)),
    )
    run.record(
        "agent.list_is_most_recent_first",
        "true",
        str(
            agent_listing == sorted(
                agent_listing, key=lambda row: row["folder"], reverse=True
            )
        ).lower(),
    )

    detail = tool_payload(answered[15])
    run.record("agent.detail_status", "finished", detail["status"])
    run.record("agent.detail_title", "Quarterly review", detail["title"])

    notes = tool_payload(answered[16])
    run.record(
        "agent.notes_match_cli",
        COMPLETE_NOTES,
        notes["content"] or "",
    )
    transcript = tool_payload(answered[17])
    run.record(
        "agent.transcript_matches_cli",
        COMPLETE_TRANSCRIPT,
        transcript["content"] or "",
    )

    incomplete_detail = tool_payload(answered[18])
    run.record("agent.incomplete_status", "unfinished", incomplete_detail["status"])
    incomplete_notes = tool_payload(answered[19])
    run.assert_that(
        "agent.never_serves_incomplete_content",
        incomplete_notes.get("content") is None,
        "content of a session that never completed is withheld",
    )

    return run


def redact(text: str, root: Path, config: Path) -> str:
    """Replaces machine-specific paths so evidence is machine-independent."""
    text = text.replace(str(config), "<CONFIG>")
    text = text.replace(str(root), "<ROOT>")
    return re.sub(r"01[0-9A-HJKMNP-TV-Z]{24}", "<SESSION-ULID>", text)


def emit(run: Run, root: Path, config: Path) -> None:
    width = max(len(entry.check) for entry in run.entries)
    print(f"{'check':<{width}}  status  observed")
    print(f"{'-' * width}  ------  --------")
    for entry in run.entries:
        status = "ok" if entry.ok else "FAIL"
        observed = redact(entry.observed, root, config).replace("\n", "\\n")
        if len(observed) > 60:
            observed = observed[:57] + "..."
        print(f"{entry.check:<{width}}  {status:<6}  {observed}")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--hermetic",
        action="store_true",
        help="build with --no-default-features: no model, no network, no permission prompt",
    )
    parser.add_argument(
        "--scenario",
        choices=["parity"],
        required=True,
        help="which qualification scenario to run",
    )
    args = parser.parse_args()

    binary = build(args.hermetic)
    workspace = Path(tempfile.mkdtemp(prefix="scrybe-qualify-"))
    try:
        root = workspace / "sessions"
        root.mkdir()
        write_fixture_tree(root)
        config = workspace / "config.toml"
        config.write_text(CONFIG.format(root=root))

        run = parity(binary, root, config)
        emit(run, root, config)

        print()
        if run.failures:
            print(f"application-service parity FAILED — {len(run.failures)} checks:")
            for entry in run.failures:
                print(f"  {entry.check}")
                print(f"    expected: {redact(entry.expected, root, config)!r}")
                print(f"    observed: {redact(entry.observed, root, config)!r}")
            return 1
        print(
            f"application-service parity: ok — {len(run.entries)} checks across "
            "list, search, show, config, doctor, record, and every read-only agent tool"
        )
        return 0
    finally:
        shutil.rmtree(workspace, ignore_errors=True)


if __name__ == "__main__":
    sys.exit(main())
