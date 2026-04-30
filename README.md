# scrybe

[![Crates.io](https://img.shields.io/crates/v/scrybe.svg?label=version)](https://crates.io/crates/scrybe)
[![License: Apache-2.0](https://img.shields.io/badge/license-Apache--2.0-blue.svg)](LICENSE)

> Open-source meeting transcription and notes. Captures audio without bots, runs on your laptop, your data on disk.

**scrybe** records your meetings without joining them. Audio capture happens on your machine, system audio plus microphone, and is transcribed and summarised locally with whisper.cpp, or via any OpenAI-compatible STT/LLM provider you choose. Notes land as markdown on disk. No accounts, no cloud sync, no bots, no telemetry. Works on macOS, Windows, Linux, and Android.

## Install

```sh
curl --proto '=https' --tlsv1.2 -LsSf \
  https://github.com/Mathews-Tom/scrybe/releases/latest/download/scrybe-cli-installer.sh | sh
```

See [`INSTALL.md`](INSTALL.md) for the manual-tarball path, the audit-friendly `cargo install` path, and an explanation of why the quick-install command bypasses Gatekeeper without notarization.
