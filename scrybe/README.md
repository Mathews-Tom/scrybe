# scrybe (placeholder)

[![Crates.io](https://img.shields.io/crates/v/scrybe.svg?label=version)](https://crates.io/crates/scrybe)
[![License: Apache-2.0](https://img.shields.io/badge/license-Apache--2.0-blue.svg)](https://github.com/Mathews-Tom/scrybe/blob/main/LICENSE)

This crate reserves the `scrybe` namespace on crates.io while the
functional library, CLI, and platform capture adapters are under
construction in the upstream repository.

There is no public API at this version. The crate exposes three
constants (`NAME`, `VERSION`, `REPOSITORY`) for `cargo search`
discoverability and nothing else.

## What scrybe is

An open-source meeting notetaker that runs entirely on the user's own
machine. Audio capture is local, transcription runs against
`whisper.cpp` locally or any OpenAI-compatible endpoint the user
configures, and notes land as markdown on disk. macOS, Windows, Linux,
and Android are the target platforms; iOS is excluded by Apple's
sandbox.

Detailed product framing, architecture, and delivery roadmap live in
the upstream repository:

- Product framing: [`docs/pitch.md`](https://github.com/Mathews-Tom/scrybe/blob/main/docs/pitch.md)
- System overview: [`docs/system-overview.md`](https://github.com/Mathews-Tom/scrybe/blob/main/docs/system-overview.md)
- Engineering contract: [`docs/system-design.md`](https://github.com/Mathews-Tom/scrybe/blob/main/docs/system-design.md)
- Legal posture: [`docs/LEGAL.md`](https://github.com/Mathews-Tom/scrybe/blob/main/docs/LEGAL.md)

## When does this crate ship something runnable

`scrybe-core`, `scrybe-capture-mac`, and `scrybe-cli` are scheduled to
ship at `v0.1.0` as a macOS-only alpha. Track progress on the upstream
repository's release tags.

## License

Apache-2.0. See [LICENSE](https://github.com/Mathews-Tom/scrybe/blob/main/LICENSE).
