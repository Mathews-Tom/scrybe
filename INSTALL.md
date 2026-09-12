# Installing scrybe

scrybe ships unsigned binaries through v1.x. macOS Gatekeeper is addressed at install time rather than at build time because Apple Developer ID enrollment remains deliberately out of scope.

This document covers the qualified macOS product path. Linux and Windows recording remain parked until their hardware qualification paths resume.

---

## macOS — Cargo install

Install the complete application from crates.io without cloning the repository:

```sh
xcode-select --install   # one-time; no-op when already installed
rustup toolchain install 1.95.0
rustup run 1.95.0 cargo install scrybe
scrybe doctor
```

The crates.io package enables microphone capture, ScreenCaptureKit system audio, local Whisper, Opus, OpenAI-compatible notes, and the desktop shell. Cargo builds native dependencies locally, so this path takes longer than the prebuilt installer. Use `cargo install scrybe --locked` only to reproduce the exact dependency graph qualified for a release or troubleshoot a registry install. The release gate verifies both the normal dependency resolution and the locked package graph.

## macOS — prebuilt quick install

```sh
curl --proto '=https' --tlsv1.2 -LsSf \
  https://github.com/Mathews-Tom/scrybe/releases/latest/download/scrybe-installer.sh | sh
scrybe doctor
```

The installer detects your CPU architecture, downloads the matching tarball, verifies its SHA256 against `dist-manifest.json`, installs `scrybe` into `~/.cargo/bin/` or `~/.local/bin/`, and updates `PATH` when required.

`curl` does not attach `com.apple.quarantine`, so this path does not require a manual `xattr` command. On an interactive terminal, `scrybe doctor` resolves the configured capture backend, explains its permission scope, and offers the applicable live probe. It remains read-only when stdin or stderr is redirected unless `--fix --sign-self <identity>` is supplied explicitly.

---

## macOS — manual install (audit-friendly)

Use this path if you want to inspect every file before it lands on disk, verify each archive's SHA256 by hand, or operate in an environment where piping `curl` into `sh` is forbidden. Provenance verification via `cosign verify-blob` is documented at the end of this file.

### 1. Pick the right tarball

Each release publishes two macOS archives at `https://github.com/Mathews-Tom/scrybe/releases`:

| Archive | When to download |
|---|---|
| `scrybe-aarch64-apple-darwin.tar.xz` | Apple Silicon Macs (M1, M2, M3, M4) |
| `scrybe-x86_64-apple-darwin.tar.xz` | Intel Macs |

`uname -m` answers which one you need: `arm64` → aarch64, `x86_64` → x86_64.

### 2. Verify the archive

Each release publishes a `SHA256SUMS.txt` asset alongside the tarballs. Download it from the same release page and verify:

```sh
curl -LO https://github.com/Mathews-Tom/scrybe/releases/latest/download/SHA256SUMS.txt
shasum -a 256 -c SHA256SUMS.txt --ignore-missing
```

`--ignore-missing` lets `shasum` succeed when only one of the two macOS archives is in your working directory. A line ending in `OK` means the archive matches the manifest; any other output means abort and re-download.

### 3. Extract and place the binary

```sh
tar -xf scrybe-aarch64-apple-darwin.tar.xz
mkdir -p ~/.local/bin
mv scrybe-aarch64-apple-darwin/scrybe ~/.local/bin/
```

If `~/.local/bin` is not on `$PATH`, add it (`export PATH="$HOME/.local/bin:$PATH"` in `~/.zshrc`).

### 4. Remove the Gatekeeper quarantine attribute

Browsers attach `com.apple.quarantine` to downloaded files. Launching produces a dialog reading "Apple cannot verify this app is free of malware." Remove the attribute once, on the binary itself:

```sh
xattr -dr com.apple.quarantine ~/.local/bin/scrybe
```

`-d` deletes the attribute; `-r` also handles an extracted `.app` bundle. The first launch after this no longer prompts.

If you re-download the archive in a new browser session, repeat this step. Quarantine is per-download, not per-binary. The quick-install path above does not need this step because `curl` does not attach the attribute.

### 5. Verify it runs

```sh
scrybe --version
scrybe doctor
```

`scrybe doctor` reports configuration, storage, egress posture, the selected capture backend, and any missing prerequisites. It asks before a live permission probe or Core Audio Tap bundle repair. Declining leaves the filesystem unchanged and prints the explicit command for later.

---

## macOS — build from a source checkout

The crates.io command above is the supported Cargo installation path. For development against a checkout:

```sh
xcode-select --install   # one-time; no-op when already installed
git clone https://github.com/Mathews-Tom/scrybe.git
cd scrybe
cargo install --path scrybe-cli --locked
```

The application package's default features match the prebuilt release. Whisper-rs compiles native whisper.cpp and Apple's Metal support; expect the first build to take several minutes.

Audit the explicit hermetic build separately:

```sh
cargo build -p scrybe --release --no-default-features
python3 scripts/check-egress-baseline.py
```

The egress audit measures that no-default-feature dependency graph and rejects
HTTP, TLS, DNS, QUIC, and forbidden Tokio networking/process features. The
published default build intentionally includes OpenAI-compatible provider
support, but no provider runs until selected in configuration.

Cargo-built binaries are local build products and do not carry Gatekeeper's
download quarantine attribute.

---

## macOS — system audio capture (`--source mic+system`)


The default application build already includes the live microphone and system-audio adapters:

```sh
cargo install scrybe
```

### ScreenCaptureKit default: macOS 13+

`[record].system_backend = "sck"` is the default. It works from the invoking terminal and does not need an `.app` bundle, signing identity, or Launch Services.

Run `scrybe doctor` from an interactive terminal after configuring Scrybe. Doctor reports `system audio backend: ScreenCaptureKit` and offers the live system-audio permission check. An affirmative response runs the existing probe directly; a decline performs no mutation and prints the explicit command:

```sh
scrybe doctor --check-sck
# expected: sck probe: frames=N peak=0.00… → OK
```

The first probe or `scrybe record "client-call"` asks for **Screen & System Audio Recording**. This is broader than audio-only consent: macOS categorizes the grant as screen recording even though Scrybe registers only an audio output handler. Grant it in System Settings → Privacy & Security → Screen & System Audio Recording.

Do not grant this permission when its screen-recording scope is unacceptable. Use microphone-only capture instead:

```sh
scrybe record "client-call" --source mic
```

### Core Audio Tap recovery path: macOS 14.4+

Set `[record].system_backend = "tap"` or pass `scrybe rec --system-backend tap` only to recover from a ScreenCaptureKit failure or compare adapters. Tap requires the narrower Audio Capture permission and a signed `.app` bundle because TCC cannot attach its grant to a bare CLI binary. ScreenCaptureKit and microphone-only use do not require this bundle.

Run `scrybe doctor`. Doctor inspects the Tap bundle, reports whether it is missing, invalid, stale, or ready, and offers repair before the live probe. Repair looks only for the project identity `scrybe-local-signing`; it never creates an identity or auto-selects an unrelated Developer ID certificate.

Create the identity once when Doctor reports that it is unavailable:

```text
Keychain Access → Certificate Assistant → Create a Certificate
  Name: scrybe-local-signing
  Identity Type: Self Signed Root
  Certificate Type: Code Signing
  Let me override defaults → Continue
  Validity period: 3650 days → Continue through remaining defaults
```

Then rerun `scrybe doctor` and confirm the displayed destination and identity. For an explicit probe with non-interactive repair:

```sh
scrybe doctor --check-tap --fix --sign-self scrybe-local-signing
# expected: tap probe: frames=N peak=0.00… → OK
```

`--fix` and `--sign-self` are a pair: each requires the other. The direct `scrybe install-macos-bundle --sign-self scrybe-local-signing` operation remains available for packaging and advanced diagnosis. Both paths build and verify a temporary sibling bundle before replacing the destination, preserving an existing valid bundle if candidate creation or validation fails.

`scrybe record` detects the legacy Tap backend and relaunches through an available bundle. Direct `scrybe rec --system-backend tap` remains for advanced diagnosis. If Tap fails to start or emits only zero-valued frames during its 1.5 s startup window, Scrybe stops it and switches once to ScreenCaptureKit; a quiet desktop can therefore switch before external audio begins.

---

## Record from a real microphone with local Whisper transcription

The hermetic `default` profile uses a synthetic 440 Hz sine, while the macOS `mac-local` profile records microphone plus system audio. Build with the `mic-capture` and `whisper-local` features and configure a model path:

```sh
# Build with all opt-in features (mic + system audio + Whisper + Opus
# + OpenAI-compat LLM for real notes summaries)
cargo install --path scrybe-cli \
  --features cli-shell,hook-git,mic-capture,system-capture-mac,whisper-local,encoder-opus,llm-openai-compat

# Download a whisper.cpp model into scrybe's platform data directory
# (one-time; pick a size that fits your RAM).
mkdir -p ~/Library/Application\ Support/dev.scrybe.scrybe/models
curl -L -o ~/Library/Application\ Support/dev.scrybe.scrybe/models/ggml-small.en.bin \
  https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-small.en.bin

# Write the one-time local Mac profile. This lands at
# ~/Library/Application Support/dev.scrybe.scrybe/config.toml unless
# SCRYBE_CONFIG or --path overrides it.
scrybe init --force

# Capture your voice plus the meeting counterparty's audio. transcript.md
# attributes utterances as `Me:` (mic) and `Them:` (system) via the
# binary-channel diarizer. No per-run source/model flags are required
# once the profile has been written.
scrybe record "client-call"
# Press Ctrl-C to stop.

scrybe list                       # shows the new session
scrybe show <session-id>          # renders transcript + notes

# audio.opus is the source master. For mic+system sessions,
# playback.opus is the centered listening mix.
ffprobe ~/scrybe/<session>/audio.opus
ffprobe ~/scrybe/<session>/playback.opus
```

The first run of `--source mic+system` triggers **Microphone** and **Screen & System Audio Recording** permission prompts for the microphone and ScreenCaptureKit adapters. Grant both via System Settings → Privacy & Security and re-run.

Pin the microphone so connecting Bluetooth headphones cannot silently change the meeting input:

```toml
[record]
input_device = "BuiltInMicrophoneDevice" # exact UID from `scrybe devices`

[stt]
model = "small.en"
language = "en"
```

For `--source mic+system`, `audio.opus` is stereo with microphone on the left and system audio on the right. `playback.opus` centers the combined meeting in both ears. Keep `audio.opus` as the source-separated transcription and archival master.

What runs:

- `scrybe-capture-mac::NativeMicCapture` opens the exact configured Core Audio device UID. If no UID is configured, scrybe resolves and reports Core Audio's current default once at session start.
- The pipeline chunks audio at the configured VAD-aware boundaries and transcribes each accepted chunk with one persistent whisper.cpp model context.
- Accepted transcript chunks print in the terminal while recording. After capture stops, the terminal reports transcript, audio, notes, and metadata finalization phases.
- The notes step uses the backend in `[record].llm`. The `mac-local` profile sets it to `openai-compat`; the default profile keeps the stub LLM for hermetic smoke tests. If finalization is interrupted, run `scrybe repair <session-folder>` followed by `scrybe notes <session-folder>`.

Whisper model sizes (English-only, `.en` suffix; multilingual variants are larger):

| Model | File size | RAM use | Speed on M1 Pro | Use when |
|---|---|---|---|---|
| `ggml-tiny.en.bin` | ~75 MB | ~390 MB | ~30× realtime | Quick smoke test only |
| `ggml-base.en.bin` | ~150 MB | ~500 MB | ~16× realtime | Lower-memory fallback |
| `ggml-small.en.bin` | ~470 MB | ~1.0 GB | ~6× realtime | Default for English meetings |
| `ggml-large-v3-turbo.bin` | ~1.5 GB | ~3.0 GB | ~2× realtime | Maximum local accuracy when latency permits |

The `--whisper-model` flag rejects `*.partial` paths so an interrupted download cannot silently produce a corrupt transcript.

`meta.toml` records the loaded model in `[providers].stt` (for example, `whisper-local:ggml-small.en`).

System audio capture on macOS uses ScreenCaptureKit by default, with the signed Core Audio Tap bundle retained as a recovery backend.

---

## Optional streaming Zipformer and English paired STT benchmark

Whisper remains the default STT backend. Sherpa is an explicit English-only option, not a replacement for multilingual Whisper. Build with both `whisper-local,stt-sherpa` to compare the shipped providers; neither `scrybe bench stt` nor the provider downloads models, corpora, or native libraries. The existing `scrybe bench --criterion-dir … --print` harvest mode is unchanged.

### Manually provision the native runtime

Before building, manually obtain and extract the matching **1.13.7 static-library archive** from the [official Sherpa release](https://github.com/k2-fsa/sherpa-onnx/releases/tag/v1.13.7). Verify the archive before extraction:

| Platform | Archive | SHA-256 |
| --- | --- | --- |
| macOS arm64 | `sherpa-onnx-v1.13.7-osx-arm64-static-lib.tar.bz2` | `126daa2e8c09a4c5d54dc985722c43bd22f598adc56445905b377454b1b27e38` |
| Linux x64 | `sherpa-onnx-v1.13.7-linux-x64-static-lib.tar.bz2` | `d1be7a69ac2b30120058d8302e624239a3064085383cfa47994a14fdc44c32d6` |
| Windows x64 | `sherpa-onnx-v1.13.7-win-x64-static-MT-Release-lib.tar.bz2` | `04734146fb3a21a297604c586ea826346dbb167c19b9ccc79c1f85d39f490395` |

Set `SHERPA_ONNX_LIB_DIR` to the extracted `lib` directory in both the build shell and the benchmark shell. It must contain the Sherpa C API, core, and ONNX Runtime static libraries and their companion libraries. Use `shasum -a 256` on macOS, `sha256sum` on Linux, or `Get-FileHash -Algorithm SHA256` on Windows to compare the archive digest. An absent environment variable deliberately resolves to a failing Cargo sentinel; do not remove that protection or use the dependency's downloader.

### Manually acquire the pinned model

Use the Apache-2.0 model [`csukuangfj/sherpa-onnx-streaming-zipformer-en-2023-06-26`](https://huggingface.co/csukuangfj/sherpa-onnx-streaming-zipformer-en-2023-06-26/tree/672fbf1b30579d6585301139bb363f42a0ad4a24), revision **`672fbf1b30579d6585301139bb363f42a0ad4a24`**. Obtain exactly these four files, preserving filenames:

| Artifact | Bytes |
| --- | ---: |
| `encoder-epoch-99-avg-1-chunk-16-left-128.int8.onnx` | 71,083,163 |
| `decoder-epoch-99-avg-1-chunk-16-left-128.onnx` | 2,092,621 |
| `joiner-epoch-99-avg-1-chunk-16-left-128.int8.onnx` | 259,335 |
| `tokens.txt` | 5,048 |
| **Total** | **73,440,167** |

Place them under `<platform-data>/models/sherpa-onnx-streaming-zipformer-en-2023-06-26/`. Platform data is `~/Library/Application Support/dev.scrybe.scrybe/` on macOS, `$XDG_DATA_HOME/scrybe/` (normally `~/.local/share/scrybe/`) on Linux, and `%APPDATA%\scrybe\scrybe\data\` on Windows. Model placement is a convention, not discovery: the benchmark requires explicit model paths.

Manual macOS acquisition commands (run intentionally; not executed by scrybe):

```sh
MODEL_DIR="$HOME/Library/Application Support/dev.scrybe.scrybe/models/sherpa-onnx-streaming-zipformer-en-2023-06-26"
MODEL_URL="https://huggingface.co/csukuangfj/sherpa-onnx-streaming-zipformer-en-2023-06-26/resolve/672fbf1b30579d6585301139bb363f42a0ad4a24"
mkdir -p "$MODEL_DIR"
for file in \
  encoder-epoch-99-avg-1-chunk-16-left-128.int8.onnx \
  decoder-epoch-99-avg-1-chunk-16-left-128.onnx \
  joiner-epoch-99-avg-1-chunk-16-left-128.int8.onnx \
  tokens.txt
do
  curl --fail --location "$MODEL_URL/$file" --output "$MODEL_DIR/$file.partial" &&
    mv "$MODEL_DIR/$file.partial" "$MODEL_DIR/$file" || exit 1
done
```

Acquire a whisper.cpp-compatible model separately using the Whisper instructions above. Keep its exact source revision and checksum with the benchmark evidence; a local filename alone does not establish model provenance.

### Manually acquire and freeze an English paired corpus

Use English audio for which redistribution/use rights and verbatim references are known, such as a selected subset of [LibriSpeech test-clean](https://www.openslr.org/12). Acquire audio and its source transcripts manually; record the dataset release, source URL, utterance ID, licence, and any trimming/conversion in each clip's `provenance`. Convert each selected utterance to **16 kHz, mono, signed 16-bit PCM RIFF WAV** before hashing it. The benchmark performs no conversion or acquisition. Keep the cohort fixed before inspecting either backend's results.

Create `<platform-data>/bench/english-paired/MANIFEST.toml` alongside its WAV files. This is a separate strict TOML schema from the historical multilingual manifest:

| Field | Requirement |
| --- | --- |
| `schema_version` | Top-level integer `1` |
| `[[clips]]` | At least one entry; every entry runs on both backends |
| `id` | Non-empty, unique clip identifier |
| `language` | Exactly `"en"` |
| `audio` | Non-empty relative WAV path within the corpus directory |
| `sha256` | SHA-256 of the final WAV bytes, 64 hexadecimal characters |
| `reference` | Non-empty verbatim transcript with scoreable words |
| `provenance` | Non-empty source/revision/licence and preparation record |

Compute each WAV's checksum after conversion (`shasum -a 256 clip.wav` on macOS). Do not use invented references or substitute synthesized speech for release evidence. The loader rejects unknown fields, duplicate IDs, non-English entries, missing provenance, checksum mismatches, and malformed/empty audio before measurement.

The existing `tests/fixtures/multilingual/MANIFEST.toml` remains the **20-clip Whisper-only** corpus. It is not a paired Sherpa cohort and cannot be passed as the new manifest.

### Run and interpret the paired benchmark

With `SHERPA_ONNX_LIB_DIR` explicitly exported to the manually provisioned runtime:

```sh
cargo build --release -p scrybe --no-default-features --features whisper-local,stt-sherpa
target/release/scrybe bench stt \
  --corpus "$HOME/Library/Application Support/dev.scrybe.scrybe/bench/english-paired/MANIFEST.toml" \
  --whisper-model "$HOME/Library/Application Support/dev.scrybe.scrybe/models/ggml-base.en.bin" \
  --sherpa-model "$MODEL_DIR"
```

Success prints one versioned JSON report containing both backends for every clip, hypotheses, WER, audio duration, provider-lifecycle time, and realtime factor. No partial report is emitted on a backend error or incomplete result set. Aggregate WER is reference-word-weighted, not a mean of clip percentages; aggregate realtime factor is summed provider-lifecycle seconds divided by summed audio seconds. Lower values are better; a realtime factor below `1` means the measured cold provider lifecycle completed faster than the audio duration.

**Timing scope:** `measurement_scope.lifecycle` is `cold-provider-per-clip`. For every clip and backend, `provider_lifecycle_secs` starts before constructing a fresh provider and stops after its single `SttProvider::transcribe` call; `total_provider_lifecycle_secs` is its per-backend sum. Whisper's model load happens inside `transcribe`; Sherpa's recognizer initialization happens in its constructor. Both costs are therefore included under the same lifecycle. Corpus loading, checksum/format validation, and JSON aggregation remain outside every clip timer. OS filesystem caches and accelerator state can remain warm across clips, so this is provider-lifecycle cold timing, not a cold-machine startup measurement, decoder-only throughput, or live partial latency. Production provider behavior is unchanged.

An English cohort does not establish multilingual quality or justify a default flip. Whisper remains selectable and remains the default regardless of these measurements. Release evidence requires a complete captured report from the manually acquired real-audio cohort; deterministic test fixtures do not satisfy that gate.

---

## Real notes summaries via Ollama / OpenAI-compat

`notes.md` is generated by the LLM provider at `SessionEnd`. When no
title is supplied, scrybe first asks the same LLM for a short factual
session title, rewrites the transcript/notes/meta headers with that
title, and renames the folder to `YYYY-MM-DD-HHMM-title-ULID`.

The hermetic configuration default is `stub` so CI smoke tests remain deterministic. On macOS, `scrybe init` selects `openai-compat`; the published application already contains that provider.

```sh
# With Ollama already serving your chosen model on localhost:11434
ollama pull gemma4:latest
scrybe init --force

# Record a session with real summarization and generated title
scrybe record
```

Any OpenAI-compatible `/chat/completions` endpoint works — Ollama (default), vLLM, OpenAI itself, Groq, Together. Point `[llm].base_url` at the upstream and (when required) set `[llm].api_key_env` to the name of the env var holding your API key:

```toml
[llm]
provider = "openai-compat"
base_url = "https://api.groq.com/openai/v1"
model = "llama3-70b-8192"
api_key_env = "GROQ_API_KEY"
```

`scrybe record` then reads `$GROQ_API_KEY` from the process environment at start time when `[llm].api_key_env` is configured. An empty / unset env var sends no `Authorization` header (the documented Ollama / self-hosted vLLM path).

`meta.toml` records the active LLM in `[providers].llm` as `<provider>:<model>` (e.g. `ollama:llama3.1:8b`, `openai-compat:llama3-70b-8192`). The retry policy in `[llm].retry` (max attempts, exponential backoff with cap) covers transient 429 / 5xx upstream failures; permanent 4xx short-circuits without retries.

A `--no-default-features` source build rejects `--llm openai-compat` rather than silently falling back to the stub.

## Read-only local-agent access (`scrybe mcp`)

`scrybe mcp` serves `list_recent_meetings`, `search_meetings`, `get_meeting`, `get_meeting_notes`, and `get_meeting_transcript` as MCP tools over newline-delimited JSON-RPC on stdin/stdout. It is off by default at two layers: the binary needs the `agent-access` build feature, and the running config needs an explicit opt-in.

```sh
cargo install scrybe --features agent-access
```

Enable it in `config.toml`:

```toml
[agent_access]
enabled = true
```

```sh
scrybe mcp --root ~/scrybe
```

Without `[agent_access].enabled = true`, `scrybe mcp` exits immediately with an error rather than starting — the surface has no write, delete, or mutate capability and no network listener, but reading meeting content is still privacy-sensitive, so it stays opt-in. See `README.md`'s Privacy and Network Posture section.

---

## Why no notarization?

macOS notarization requires an Apple Developer ID enrollment ($99/year) and ties the project's release pipeline to a vendor account. `.docs/development-plan.md` §13.1 documents the trade: until scrybe has demonstrated longevity, vendor-tied trust dependencies stay deferred. Three documented install paths sidestep Gatekeeper entirely:

| Path | Quarantine bypass |
|---|---|
| Quick install (`curl \| sh`) | `curl` does not attach `com.apple.quarantine` |
| Manual install (browser tarball) | `xattr -dr com.apple.quarantine` step |
| Build from source (`cargo install`) | Local builds are never quarantined |

This posture is reviewed post-v1.0 if first-run friction is shown to materially block adoption.

---

## Verify a release with cosign

Each GitHub Release ships a cosign-signed `SHA256SUMS.txt` covering every artifact and a separately-signed `scrybe-sbom.cdx.json` (CycloneDX SBOM). Verifying the manifest's signature transitively covers every asset whose hash appears in the file — there is no need to verify each tarball individually.

Install cosign once (any 2.x release works):

```sh
brew install cosign            # macOS
# or download from https://github.com/sigstore/cosign/releases
```

Download the manifest, its signature, and its certificate from the release page:

```sh
TAG=v1.3.2   # the release you are verifying
BASE="https://github.com/Mathews-Tom/scrybe/releases/download/${TAG}"
curl -LO "${BASE}/SHA256SUMS.txt"
curl -LO "${BASE}/SHA256SUMS.txt.sig"
curl -LO "${BASE}/SHA256SUMS.txt.pem"
```

Verify keylessly. The `--certificate-identity` and `--certificate-oidc-issuer` flags pin the trust chain to the GitHub Actions release workflow on the upstream repository:

```sh
cosign verify-blob \
  --certificate SHA256SUMS.txt.pem \
  --signature SHA256SUMS.txt.sig \
  --certificate-identity-regexp "^https://github.com/Mathews-Tom/scrybe/.github/workflows/release.yml@refs/tags/${TAG}$" \
  --certificate-oidc-issuer https://token.actions.githubusercontent.com \
  SHA256SUMS.txt
```

`Verified OK` on stdout means the manifest was produced by the release workflow at this exact tag. Any other output means abort. With the manifest verified, the per-tarball checksum check from §2 of the manual-install path covers the binary you are about to run.

The same recipe works for the SBOM:

```sh
cosign verify-blob \
  --certificate scrybe-sbom.cdx.json.pem \
  --signature scrybe-sbom.cdx.json.sig \
  --certificate-identity-regexp "^https://github.com/Mathews-Tom/scrybe/.github/workflows/release.yml@refs/tags/${TAG}$" \
  --certificate-oidc-issuer https://token.actions.githubusercontent.com \
  scrybe-sbom.cdx.json
```

cosign is artifact-level CI provenance, not OS-level code signing. Gatekeeper's "Apple cannot verify" prompt and Windows SmartScreen are unaffected by a cosign-verified tarball — the install paths above remain the way to handle each.

---

## Verify reproducibility

`.github/workflows/reproducibility.yml` builds each release tarball twice on a fresh `macos-26` runner from divergent workspace paths and compares SHA256 across legs. The release workflow pins `SOURCE_DATE_EPOCH=1714464000`, remaps the workspace path to `/build`, preserves the Mach-O UUID required by dyld, and uses Rust 1.95.0.

The lane remains advisory because Mach-O UUIDs and cargo-dist archive metadata are not yet bit-identical across independent builds. Both legs upload their artifacts for `diffoscope` analysis.

Local reproduction recipe (matches the CI inputs):

```sh
git clone --branch v1.3.2 https://github.com/Mathews-Tom/scrybe.git scrybe
cd scrybe
SOURCE_DATE_EPOCH=1714464000 \
  RUSTFLAGS="--remap-path-prefix=$(pwd)=/build" \
  cargo dist build --artifacts=local --target=aarch64-apple-darwin
shasum -a 256 target/distrib/scrybe-aarch64-apple-darwin.tar.xz
```

Comparison against a published release tag's `SHA256SUMS.txt` is informative but not yet authoritative — until the v1.0.x reproducibility-hardening lands, divergences here are expected. File an issue with `xcodebuild -showsdks` and `rustc -vV` if you investigate; the diffoscope output is the load-bearing artifact.

---

## Linux and Windows

Linux and Windows recording remain parked until each platform has a maintainer-owned hardware qualification path. The v1.3.2 crates.io installation contract is macOS-only; do not present a successful cross-platform compile as recording support.
