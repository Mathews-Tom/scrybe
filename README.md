# scrybe

[![Crates.io](https://img.shields.io/crates/v/scrybe.svg?label=version)](https://crates.io/crates/scrybe)
[![License: Apache-2.0](https://img.shields.io/badge/license-Apache--2.0-blue.svg)](LICENSE)

> Local-first meeting transcription and notes. No bot, no account, no vendor cloud by default.

scrybe is an open-source meeting transcription tool built around one constraint: the meeting artifacts belong on the user's machine as ordinary files. It captures audio locally, transcribes it with either local Whisper or a user-configured OpenAI-compatible provider, generates Markdown notes, and writes everything under `~/scrybe/`.

Current release: `v1.0.4`.

## What Works Today

The supported user path today is macOS:

- `scrybe init` writes the default local macOS profile on macOS.
- `scrybe init --profile default` writes the hermetic synthetic profile used by CI and cross-platform smoke tests.
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

mkdir -p ~/Library/Application\ Support/dev.scrybe.scrybe/models
curl -L -o ~/Library/Application\ Support/dev.scrybe.scrybe/models/ggml-base.en.bin \
  https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-base.en.bin

ollama pull gemma4:latest
scrybe init
```

On macOS, bare `scrybe init` writes the local recording profile:

- `[record].source = "mic+system"`
- `[record].whisper_model = "<platform-data-dir>/models/ggml-base.en.bin"`
- `[record].llm = "openai-compat"`
- `[llm].model = "gemma4:latest"`

The macOS platform data path is
`~/Library/Application Support/dev.scrybe.scrybe/`. Other platforms resolve
the model path through their native data directory convention. Pass
`--profile default` for the synthetic smoke-test profile, or override local
model choices with `--whisper-model <PATH>` and `--llm-model <MODEL>`.

If a config file already exists, `scrybe init` refuses to overwrite it; pass
`--force` only when you intentionally want to replace the existing config with
fresh profile defaults.

Record:

```sh
scrybe record
# Press Ctrl-C to stop.
scrybe list
scrybe show <session-id>
```

For cloud or hosted-compatible LLMs, configure `[llm]` with a base URL, model, and an environment-variable name for the API key. Secrets stay in the environment, not in `config.toml`.

## Storage Model

Every session is a directory:

```text
~/scrybe/
└── 2026-05-02-1430-acme-discovery-01HXY7K9RZ/
    ├── audio.opus
    ├── transcript.md
    ├── notes.md
    ├── meta.toml
    ├── pid.lock
    ├── transcript.partial.jsonl
    └── .stignore
```

The filesystem is the database. `meta.toml` and `notes.md` use atomic replace. `transcript.md` and `audio.opus` are append-only. Audio is treated as the source of truth so failed or improved transcription can be regenerated later.

## Architecture

scrybe is a Rust workspace with a small core and platform adapters:

| Crate | Role |
|---|---|
| `scrybe` | Published placeholder crate and public package identity |
| `scrybe-core` | Session orchestration, storage, config, providers, hooks, diarization, pipeline |
| `scrybe-cli` | CLI binary: `init`, `record`, `list`, `show`, `doctor`, `bench` |
| `scrybe-capture-mac` | macOS Core Audio Taps adapter |
| `scrybe-capture-mic` | Cross-platform microphone adapter via `cpal` |
| `scrybe-capture-linux` | PipeWire/Pulse adapter surface |
| `scrybe-capture-win` | WASAPI adapter surface |
| `scrybe-android` | Android FFI adapter surface |

The important public seams are:

- `AudioCapture` for platform audio.
- `ContextProvider` for meeting metadata.
- `SttProvider` and `LlmProvider` for transcription and notes backends.
- `Diarizer` for speaker attribution.
- `Hook` for post-session actions.

The Tier-1 stability contract is documented in [`docs/system-design.md`](docs/system-design.md). In short: `AudioCapture`, `MeetingContext`, `LifecycleEvent`, `ConsentAttestation`, the `meta.toml` schema, storage invariants, and the Apache-2.0 license are frozen for the v1 series.

## Privacy and Network Posture

- Default builds keep the network provider graph out of the binary.
- Cloud STT/LLM is opt-in through OpenAI-compatible config.
- API keys are read from named environment variables.
- There is no account system, sync service, telemetry, hosted backend, or bot that joins calls.
- Courtesy notification is part of the recording flow and is recorded in `meta.toml`.

Run the egress audit locally:

```sh
cargo build --release --no-default-features
python3 scripts/check-egress-baseline.py
```

## Current Limitations

- macOS is the only polished binary distribution target today.
- `--source mic+system` requires macOS 14.4+ and Audio Capture permission.
- Linux, Windows, and Android adapters are in-tree but still need real end-user validation and packaging.
- Tray and global-hotkey shell support exists behind `cli-shell`; the headless `record` path remains the reliable path.
- The crates.io `scrybe` package is the public package identity. The implementation crates are currently `publish = false`; install the CLI from GitHub releases or build from source.
- Native macOS notarization and Windows Authenticode signing are out of scope for the v1 line. Release artifacts are verified with checksums and cosign provenance instead.

## Development

Required toolchain is pinned by the repository:

```sh
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo check --all-targets
cargo test --workspace
```

Additional gates used by CI include `cargo audit`, `cargo deny check`, coverage, LoC budget, egress audit, release planning, and advisory reproducibility checks.

Project docs:

- [`docs/pitch.md`](docs/pitch.md) — product framing and market position.
- [`docs/system-overview.md`](docs/system-overview.md) — user-facing system explanation.
- [`docs/system-design.md`](docs/system-design.md) — engineering contract and stability tiers.
- [`INSTALL.md`](INSTALL.md) — installation, source builds, and release verification.
- [`MAINTENANCE.md`](MAINTENANCE.md) — v1 maintenance commitments.
- [`CHANGELOG.md`](CHANGELOG.md) — release history.

## License

Apache-2.0. See [`LICENSE`](LICENSE).
