# scrybe

[![Crates.io](https://img.shields.io/crates/v/scrybe.svg?label=version)](https://crates.io/crates/scrybe)
[![License: Apache-2.0](https://img.shields.io/badge/license-Apache--2.0-blue.svg)](LICENSE)

> Open-source meeting transcription and notes. Captures audio without bots, runs on your laptop, your data on disk.

**scrybe** records your meetings without joining them. Audio capture happens on your machine, system audio plus microphone, and is transcribed and summarised locally with whisper.cpp, or via any OpenAI-compatible STT/LLM provider you choose. Notes land as markdown on disk. No accounts, no cloud sync, no bots, no telemetry. Works on macOS, Windows, Linux, and Android.

## Install

See [`INSTALL.md`](INSTALL.md) for the macOS unsigned-binary install flow (download tarball → `xattr -dr com.apple.quarantine`) and the audit-friendly `cargo install --path scrybe-cli` path.
