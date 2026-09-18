#!/usr/bin/env python3
# Copyright 2026 Mathews Tom
# Licensed under the Apache License, Version 2.0 (the "License");
"""Build Tauri's static ``latest.json`` from signed update archives."""

from __future__ import annotations

import argparse
import json
from pathlib import Path
from urllib.parse import urlparse

SUPPORTED_PLATFORMS = {"darwin-aarch64", "darwin-x86_64"}


def release_url(value: str) -> str:
    parsed = urlparse(value)
    if parsed.scheme != "https" or not parsed.netloc:
        raise argparse.ArgumentTypeError(
            "update artifact URLs must be absolute HTTPS URLs"
        )
    return value


def signature(path: Path) -> str:
    try:
        value = path.read_text(encoding="utf-8").strip()
    except OSError as error:
        raise ValueError(f"cannot read updater signature {path}: {error}") from error
    if not value:
        raise ValueError(f"updater signature {path} is empty")
    return value


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--version", required=True, help="released application version")
    parser.add_argument("--notes", required=True, help="plain-text release notes")
    parser.add_argument(
        "--published-at", required=True, help="RFC 3339 release timestamp"
    )
    parser.add_argument(
        "--output", required=True, type=Path, help="latest.json destination"
    )
    parser.add_argument(
        "--platform",
        action="append",
        nargs=3,
        metavar=("TARGET", "URL", "SIGNATURE"),
        required=True,
        help="Tauri target, HTTPS archive URL, and .sig path; repeat once per target",
    )
    arguments = parser.parse_args()

    platforms: dict[str, dict[str, str]] = {}
    for target, raw_url, signature_path in arguments.platform:
        if target not in SUPPORTED_PLATFORMS:
            parser.error(
                f"unsupported target {target!r}; expected one of {sorted(SUPPORTED_PLATFORMS)}"
            )
        if target in platforms:
            parser.error(f"duplicate updater target {target!r}")
        try:
            url = release_url(raw_url)
            signed = signature(Path(signature_path))
        except (argparse.ArgumentTypeError, ValueError) as error:
            parser.error(str(error))
        platforms[target] = {"signature": signed, "url": url}

    missing = SUPPORTED_PLATFORMS - platforms.keys()
    if missing:
        parser.error(f"missing updater target(s): {', '.join(sorted(missing))}")

    manifest = {
        "version": arguments.version,
        "notes": arguments.notes,
        "pub_date": arguments.published_at,
        "platforms": platforms,
    }
    arguments.output.parent.mkdir(parents=True, exist_ok=True)
    temporary = arguments.output.with_suffix(f"{arguments.output.suffix}.tmp")
    temporary.write_text(json.dumps(manifest, indent=2) + "\n", encoding="utf-8")
    temporary.replace(arguments.output)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
