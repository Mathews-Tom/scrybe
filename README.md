# scrybe

[![Crates.io](https://img.shields.io/crates/v/scrybe.svg?label=version)](https://crates.io/crates/scrybe)
[![License: Apache-2.0](https://img.shields.io/badge/license-Apache--2.0-blue.svg)](LICENSE)

> Local-first meeting transcription and notes. No bot, no account, no vendor cloud by default.

scrybe is an open-source meeting transcription tool built around one constraint: the meeting artifacts belong on the user's machine as ordinary files. It captures audio locally, transcribes it with either local Whisper or a user-configured OpenAI-compatible provider, generates Markdown notes, and writes everything under `~/scrybe/`.

Current release: `v1.3.2`.

## What Works Today

The supported user path today is macOS:

- `scrybe init` writes the default local macOS profile on macOS.
- `scrybe init --profile default` writes the hermetic synthetic profile used by CI and cross-platform smoke tests.
- `scrybe record` creates a session folder with source-separated `audio.opus`, centered `playback.opus`, `transcript.md`, `notes.md`, and `meta.toml`.
- `--source synthetic` runs the hermetic smoke path used by CI.
- `--source mic` records Core Audio's default microphone resolved once at session start; `--input-device <uid>` or `[record].input_device` pins an exact macOS Core Audio device UID.
- `scrybe devices` lists macOS input-device UIDs and identifies the current default.
- `--source mic+system` records microphone plus macOS system audio through ScreenCaptureKit when built with `mic-capture,system-capture-mac` on macOS 13+. It requires the broader **Screen & System Audio Recording** permission.
- `--whisper-model <PATH>` enables local whisper.cpp transcription when built with `whisper-local`.
- `--llm openai-compat` enables real notes through Ollama, vLLM, OpenAI, Groq, Together, or any compatible `/chat/completions` endpoint when built with `llm-openai-compat`.
- `scrybe list`, `scrybe show <id>`, `scrybe doctor`, `scrybe repair <session>`, `scrybe notes <session>`, and `scrybe bench` are available in the CLI.
- `scrybe bench stt --corpus <MANIFEST> --whisper-model <FILE> --sherpa-model <DIR>` compares both local providers on a checksum-validated English paired corpus when built with `whisper-local,stt-sherpa` and an explicitly provisioned native runtime. [Manual acquisition and measurement scope](INSTALL.md#optional-streaming-zipformer-and-english-paired-stt-benchmark). Whisper remains the default; the historical multilingual corpus is Whisper-only.

Linux, Windows, and Android crates are present in the workspace as adapter surfaces and scaffolds. They are not the polished end-user install path yet. The project keeps those adapters in-tree so the trait contracts, config, tests, and packaging work stay cross-platform from the start.

## Install

Install the full macOS application from crates.io:

```sh
cargo install scrybe --locked
scrybe doctor
```

This builds Scrybe locally with microphone capture, ScreenCaptureKit system audio, Whisper, Opus, and OpenAI-compatible notes enabled. It requires Rust 1.95 and Xcode Command Line Tools.

For a faster prebuilt installation:

```sh
curl --proto '=https' --tlsv1.2 -LsSf \
  https://github.com/Mathews-Tom/scrybe/releases/latest/download/scrybe-installer.sh | sh
scrybe doctor
```

The GitHub installer downloads the matching macOS tarball, verifies the release checksum manifest, and installs `scrybe` on `PATH`. Both installation paths provide the same production capabilities. Manual tarball installation, release verification, and source builds are documented in [`INSTALL.md`](INSTALL.md).

## First Local Recording Setup
```sh
mkdir -p ~/Library/Application\ Support/dev.scrybe.scrybe/models
curl -L -o ~/Library/Application\ Support/dev.scrybe.scrybe/models/ggml-small.en.bin \
  https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-small.en.bin

ollama pull gemma4:latest
scrybe init
```

On macOS, bare `scrybe init` writes the local recording profile:

- `[record].source = "mic+system"`
- `[record].input_device = "<Core Audio UID from scrybe devices>"` (pins the meeting microphone instead of following later OS default changes)
- `[record].llm = "openai-compat"`
- `[record].system_backend = "sck"` (ScreenCaptureKit; use `"tap"` only for the macOS 14.4+ legacy Core Audio Tap recovery path)
- `[stt].model = "small.en"` and `[stt].language = "en"`
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
scrybe record "client-call"
# Press Ctrl-C to stop.
scrybe list
scrybe show <session-id>
```

The ergonomic `scrybe record TITLE` resolves capture source, system-audio backend, Whisper model, and LLM kind from your config and platform probes. ScreenCaptureKit runs directly from the invoking terminal; the legacy `tap` backend auto-launches through the `.app` bundle so its Audio Capture TCC grant binds correctly. Use `scrybe rec --title TITLE --source … --system-backend … --whisper-model … --llm …` when you need explicit flag control (CI, debugging, alternate hardware setups).

Select a microphone explicitly:

```sh
scrybe devices
scrybe rec --title "client-call" --source mic --input-device <uid>
```

The terminal prints each accepted transcript chunk while recording, then reports transcript flush, audio encoding, notes generation, and metadata-writing progress after the first `Ctrl-C` or `SIGTERM`. A second signal aborts finalization immediately. Run `scrybe repair <session-folder>` to recover unfinished audio or reconstruct missing metadata, then `scrybe notes <session-folder>` to regenerate missing notes.

For cloud or hosted-compatible LLMs, configure `[llm]` with a base URL, model, and an environment-variable name for the API key. Secrets stay in the environment, not in `config.toml`.

## Storage Model

Every session is a directory:

```text
~/scrybe/
└── 2026-05-02-1430-acme-discovery-01HXY7K9RZ/
    ├── audio.opus
    ├── playback.opus
    ├── transcript.md
    ├── notes.md
    ├── meta.toml
    ├── pid.lock
    ├── transcript.partial.jsonl
    └── .stignore
```

The filesystem is the database. `meta.toml`, `notes.md`, `audio.opus`, and `playback.opus` use atomic replace. `transcript.md` is append-only. Audio is treated as the source of truth so failed or improved transcription can be regenerated later.

For `--source mic+system`, `audio.opus` preserves the source master with the user's microphone on the left channel and system audio on the right. `playback.opus` contains the same meeting as a centered stereo listening mix, preventing either speaker from playing in only one ear. The exact source layout remains recorded in `meta.toml` under `[audio].layout` as `stereo:mic-l,system-r`; downstream transcription and archival tools must use `audio.opus`, not the convenience playback mix. Mono sessions produce only `audio.opus`.

## Architecture

scrybe is a Rust workspace with a small core and platform adapters:

| Package | Role |
|---|---|
| `scrybe` | Published application and `scrybe` binary (`scrybe-cli/`) |
| `scrybe-meeting-core` | Published session, storage, config, provider, hook, diarization, and pipeline library |
| `scrybe-meeting-capture-mac` | Published macOS microphone and system-audio adapter |
| `scrybe-meeting-capture-mic` | Published microphone adapter via `cpal` |
| `scrybe-capture-linux` | Private PipeWire/Pulse adapter surface |
| `scrybe-capture-win` | Private WASAPI adapter surface |
| `scrybe-android` | Private Android FFI adapter surface |

The important public seams are:

- `AudioCapture` for platform audio.
- `ContextProvider` for meeting metadata.
- `SttProvider` and `LlmProvider` for transcription and notes backends.
- `Diarizer` for speaker attribution.
- `Hook` for post-session actions.

The Tier-1 stability contract is documented in [`docs/system-design.md`](docs/system-design.md). In short: `AudioCapture`, `MeetingContext`, `LifecycleEvent`, `ConsentAttestation`, the `meta.toml` schema, storage invariants, and the Apache-2.0 license are frozen for the v1 series.

## Privacy and Network Posture

- The published application includes OpenAI-compatible provider support, but no network provider runs unless selected in configuration.
- API keys are read from named environment variables.
- There is no account system, sync service, telemetry, hosted backend, or bot that joins calls.
- Courtesy notification is part of the recording flow and is recorded in `meta.toml`.
- `scrybe mcp` (feature `agent-access`, off by default) is a read-only local-agent surface over `~/scrybe/`: it exposes `list_recent_meetings`, `search_meetings`, `get_meeting`, `get_meeting_notes`, and `get_meeting_transcript` as MCP tools over stdio JSON-RPC. There is no write, delete, or mutate capability anywhere in the module, and no network listener — stdio only, reachable only by spawning it as a child process. The server refuses to start unless `[agent_access].enabled = true` is set explicitly in `config.toml`.

Run the egress audit locally:

```sh
cargo build -p scrybe --release --no-default-features
python3 scripts/check-egress-baseline.py
```

## Current Limitations

- macOS is the only polished binary distribution target today.
- `--source mic+system` defaults to ScreenCaptureKit on macOS 13+ and requires **Screen & System Audio Recording**. This privacy permission covers screen recording in addition to system audio; deny it if that scope is unacceptable.
- The macOS 14.4+ Core Audio Tap backend remains available as `[record].system_backend = "tap"` for recovery. It requires the narrower Audio Capture permission and a signed `.app` bundle. A failed or silent Tap switches once to ScreenCaptureKit after a 1.5 s startup window, so a quiet desktop can switch before external audio begins.
- Tray and global-hotkey shell support exists behind `cli-shell`; the headless `record` path remains the reliable path.
- crates.io installation supports the polished macOS application. Linux and Windows recording remain parked until hardware-qualified release paths exist.
- Native macOS notarization and Windows Authenticode signing are out of scope for the v1 line. Release artifacts are verified with checksums and cosign provenance instead.

## Development

Required toolchain is pinned by the repository:

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets --no-default-features -- -D warnings
cargo check --workspace --all-targets --no-default-features
cargo test --workspace --all-targets --no-default-features
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
