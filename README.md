# scrybe

[![Crates.io](https://img.shields.io/crates/v/scrybe.svg?label=version)](https://crates.io/crates/scrybe)
[![License: Apache-2.0](https://img.shields.io/badge/license-Apache--2.0-blue.svg)](LICENSE)

> Local-first meeting transcription and notes. No bot, no account, no vendor cloud by default.

scrybe is an open-source meeting transcription tool built around one constraint: the meeting artifacts belong on the user's machine as ordinary files. It captures audio locally, transcribes it with either local Whisper or a user-configured OpenAI-compatible provider, generates Markdown notes, and writes everything under `~/scrybe/`.

Current release: `v1.0.4`.

## What Works Today

The supported user path today is macOS:

- `scrybe init` writes a local profile and validates the environment.
- `scrybe record` creates a session folder with `audio.opus`, `transcript.md`, `notes.md`, and `meta.toml`.
- `--source synthetic` runs the hermetic smoke path used by CI.
- `--source mic` records the default microphone when the binary is built with `mic-capture`.
- `--source mic+system` records microphone plus macOS system audio through Core Audio Taps when built with `mic-capture,system-capture-mac` on macOS 14.4+.
- `--whisper-model <PATH>` enables local whisper.cpp transcription when built with `whisper-local`.
- `--llm openai-compat` enables real notes through Ollama, vLLM, OpenAI, Groq, Together, or any compatible `/chat/completions` endpoint when built with `llm-openai-compat`.
- `scrybe list`, `scrybe show <id>`, `scrybe doctor`, and `scrybe bench` are available in the CLI.

Linux, Windows, and Android crates are present in the workspace as adapter surfaces and scaffolds. They are not the polished end-user install path yet. The project keeps those adapters in-tree so the trait contracts, config, tests, and packaging work stay cross-platform from the start.

## Install

macOS quick install:

```sh
curl --proto '=https' --tlsv1.2 -LsSf \
  https://github.com/Mathews-Tom/scrybe/releases/latest/download/scrybe-cli-installer.sh | sh
scrybe doctor
```

The installer downloads the matching macOS tarball, verifies the release checksum manifest, and installs the `scrybe` binary on `PATH`. It does not require notarization because `curl` downloads do not carry the browser quarantine attribute.

Manual tarball installation, release verification, and source builds are documented in [`INSTALL.md`](INSTALL.md).

## Real Local Recording Build

The release tarball is intentionally conservative. For the full local macOS path with microphone, system audio, Whisper, Opus, and OpenAI-compatible notes:

```sh
cargo install --path scrybe-cli \
  --features cli-shell,hook-git,mic-capture,system-capture-mac,whisper-local,encoder-opus,llm-openai-compat
```

Initialize a local profile:

```sh
scrybe init --profile mac-local --force \
  --whisper-model ~/Library/Application\ Support/scrybe/models/ggml-base.en.bin \
  --llm-model gemma4:latest
```

Record:

```sh
scrybe record
# Press Ctrl-C to stop.
scrybe list
scrybe show <session-id>
```

For cloud or hosted-compatible LLMs, configure `[llm]` with a base URL, model, and an environment-variable name for the API key. Secrets stay in the environment, not in `config.toml`.

## License

Apache-2.0. See [`LICENSE`](LICENSE).
