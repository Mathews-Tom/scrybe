# Installing scrybe

scrybe ships unsigned binaries through v1.0. macOS Gatekeeper and the equivalent SmartScreen prompt on Windows are addressed at install time rather than at build time — Apple Developer ID enrollment and Windows code-signing certificates are deliberately out of scope (`.docs/development-plan.md` §13.1).

This document covers macOS today. Linux and Windows installation paths land in their respective releases.

---

## macOS — quick install (recommended)

```sh
curl --proto '=https' --tlsv1.2 -LsSf \
  https://github.com/Mathews-Tom/scrybe/releases/latest/download/scrybe-cli-installer.sh | sh
scrybe doctor
```

The installer detects your CPU architecture, downloads the matching tarball, verifies its SHA256 against the release's checksum manifest (`dist-manifest.json`), extracts the binary into `~/.cargo/bin/` (or `~/.local/bin/` if cargo is not present), and adds that directory to your `PATH` if needed.

`curl` does not attach `com.apple.quarantine` to its downloads, so Gatekeeper's "Apple cannot verify" dialog never fires for binaries installed this way — you do not need to run `xattr` by hand.

`scrybe doctor` confirms permission state, disk space, and any missing prerequisites before your first session.

---

## macOS — manual install (audit-friendly)

Use this path if you want to inspect every file before it lands on disk, verify each archive's SHA256 by hand, or operate in an environment where piping `curl` into `sh` is forbidden. Provenance verification via `cosign verify-blob` is documented at the end of this file.

### 1. Pick the right tarball

Each release publishes two macOS archives at `https://github.com/Mathews-Tom/scrybe/releases`:

| Archive | When to download |
|---|---|
| `scrybe-cli-aarch64-apple-darwin.tar.xz` | Apple Silicon Macs (M1, M2, M3, M4) |
| `scrybe-cli-x86_64-apple-darwin.tar.xz` | Intel Macs |

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
tar -xf scrybe-cli-aarch64-apple-darwin.tar.xz
mkdir -p ~/.local/bin
mv scrybe-cli-aarch64-apple-darwin/scrybe ~/.local/bin/
```

If `~/.local/bin` is not on `$PATH`, add it (`export PATH="$HOME/.local/bin:$PATH"` in `~/.zshrc`).

### 4. Remove the Gatekeeper quarantine attribute

Browsers attach `com.apple.quarantine` to downloaded files. Launching produces a dialog reading "Apple cannot verify this app is free of malware." Remove the attribute once, on the binary itself:

```sh
xattr -dr com.apple.quarantine ~/.local/bin/scrybe
```

`-d` deletes the attribute; `-r` recurses if you ever extract into a `.app` bundle (scrybe-cli ships a single binary, so the recurse flag is harmless). The first launch after this no longer prompts.

If you re-download the archive in a new browser session, repeat this step. Quarantine is per-download, not per-binary. The quick-install path above does not need this step because `curl` does not attach the attribute.

### 5. Verify it runs

```sh
scrybe --version
scrybe doctor
```

`scrybe doctor` checks permission state, disk space, and reports any missing prerequisites.

---

## macOS — build from source

The Apache-2.0 source builds cleanly on a stock macOS install with Xcode Command Line Tools and `rustup`:

```sh
xcode-select --install   # one-time, prompts a UI
brew install rustup-init && rustup-init -y
git clone https://github.com/Mathews-Tom/scrybe.git
cd scrybe
cargo install --path scrybe-cli --features cli-shell,hook-git
```

`whisper-local` is opt-in. To compile the local Whisper provider into your build, add it to the feature list:

```sh
cargo install --path scrybe-cli --features cli-shell,hook-git,whisper-local
```

Whisper-rs links a vendored libwhisper and Apple's Metal framework. Expect a longer compile (~5 min on M1 Pro on first build).

To audit that the default-feature build has zero outbound network capability:

```sh
cargo build --release --no-default-features
python3 scripts/check-egress-baseline.py
```

The egress audit walks `scrybe-cli`'s default-feature dependency graph and asserts that no HTTP, TLS, DNS, or QUIC crate is linked in. The CI gate is the same script.

`cargo install` builds the binary on your machine; it never interacts with Gatekeeper, so no `xattr` step is needed for this path either.

---

## macOS — system audio capture (`--source mic+system`)

Capturing the meeting counterparty's voice via Core Audio Taps requires the binary to be wrapped in a `.app` bundle and code-signed. A bare CLI at `~/.cargo/bin/scrybe` cannot receive Audio Capture consent: TCC refuses to surface the permission prompt without an `Info.plist` declaring `NSAudioCaptureUsageDescription`, and the underlying tap silently zero-fills its IO callback when no consent record can be created. `scrybe doctor --check-tap` reports `frames>0, peak=0.0000` in this state — the diagnostic that distinguishes "tap delivered silence" from "IOProc never fired".

### Self-signed certificate (free, no Apple Developer membership)

A self-signed Code Signing certificate created in Keychain Access produces a stable designated requirement (`anchor leaf [Subject.CN] = "..."`) that survives `cargo install --force` rebuilds — TCC keeps the Audio Capture grant attached to the cert's identity rather than the binary's hash.

```text
Applications → Utilities → Keychain Access
  Menu: Keychain Access → Certificate Assistant → Create a Certificate…
    Name: scrybe-local-signing
    Identity Type: Self Signed Root
    Certificate Type: Code Signing
    ✓ Let me override defaults  → Continue
    Validity: 3650 days  → Continue through remaining defaults  → Done
```

Verify the cert exists, then build the bundle:

```sh
security find-identity -v -p codesigning | grep scrybe-local-signing

cargo install --path scrybe-cli --force --locked \
    --features cli-shell,hook-git,mic-capture,system-capture-mac,whisper-local,encoder-opus,llm-openai-compat
packaging/macos-app/build-app.sh \
    --binary "$HOME/.cargo/bin/scrybe" \
    --output ./scrybe.app \
    --sign-self scrybe-local-signing
```

Remove any stale TCC entry, launch the bundle, and accept the prompt:

```text
System Settings → Privacy & Security → Audio Recording
  click scrybe (if listed)  → click `-`  → quit Settings
```

```sh
open ./scrybe.app --args doctor --check-tap
# click Allow when the dialog appears
# expected: tap probe: frames=N peak=0.X → OK
```

For day-to-day recording, invoke the binary inside the bundle (which is the one TCC has granted):

```sh
./scrybe.app/Contents/MacOS/scrybe record --source mic+system --title my-meeting --yes
```

Or symlink it onto `PATH`:

```sh
ln -sf "$PWD/scrybe.app/Contents/MacOS/scrybe" "$HOME/.local/bin/scrybe"
```

### Iteration loop

Each `cargo install --force` rewrites `~/.cargo/bin/scrybe`. Re-run `build-app.sh` to refresh the bundle's binary; the cert identity is stable so the existing TCC grant carries over without a second prompt:

```sh
alias scrybe-rebundle='packaging/macos-app/build-app.sh \
    --binary "$HOME/.cargo/bin/scrybe" \
    --output ./scrybe.app \
    --sign-self scrybe-local-signing'
```

### Developer ID (paid, required for distribution)

For builds intended to ship through the GitHub Releases tarball, swap `--sign-self` for `--sign` and pass the Developer ID Application identity:

```sh
packaging/macos-app/build-app.sh \
    --binary "$HOME/.cargo/bin/scrybe" \
    --output ./scrybe.app \
    --sign "Developer ID Application: Your Name (TEAMID)"
```

Notarization is currently out of scope for the v1 line; see `README.md:151`.

### Why this is needed

The full rationale, including the entitlements specifically required by Core Audio Taps under the hardened runtime, lives in `packaging/macos-app/README.md`.

---

## Record from a real microphone with local Whisper transcription

The default `scrybe record` runs a synthetic 440 Hz sine through the pipeline so CI smoke tests stay hermetic. To record from your actual mic and transcribe with whisper.cpp, build with the `mic-capture` and `whisper-local` features and supply a model path at runtime:

```sh
# Build with all opt-in features (mic + system audio + Whisper + Opus
# + OpenAI-compat LLM for real notes summaries)
cargo install --path scrybe-cli \
  --features cli-shell,hook-git,mic-capture,system-capture-mac,whisper-local,encoder-opus,llm-openai-compat

# Download a whisper.cpp model into scrybe's platform data directory
# (one-time; pick a size that fits your RAM).
mkdir -p ~/Library/Application\ Support/dev.scrybe.scrybe/models
curl -L -o ~/Library/Application\ Support/dev.scrybe.scrybe/models/ggml-base.en.bin \
  https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-base.en.bin

# Write the one-time local Mac profile. This lands at
# ~/Library/Application Support/dev.scrybe.scrybe/config.toml unless
# SCRYBE_CONFIG or --path overrides it.
scrybe init --force

# Capture your voice plus the meeting counterparty's audio. transcript.md
# attributes utterances as `Me:` (mic) and `Them:` (system) via the
# binary-channel diarizer. No per-run source/model flags are required
# once the profile has been written.
scrybe record
# Press Ctrl-C to stop.

scrybe list                       # shows the new session
scrybe show <session-id>          # renders transcript + notes

# audio.opus is now real Ogg/Opus — playable in any standard audio
# tool. Without --features encoder-opus the file is raw PCM bytes
# (the v0.1 NullEncoder fallback) and ffmpeg/vlc reject it.
ffprobe ~/scrybe/<session>/audio.opus
```

The first run of `--source mic+system` triggers two macOS permission
prompts:

- **Microphone** — for the cpal default-input adapter.
- **Audio Capture** — for the Core Audio Taps adapter that captures
  system playback. Requires macOS 14.4+; older macOS versions return
  `CaptureError::PermissionDenied` even after grant because the
  underlying API is unavailable.

Grant both via System Settings → Privacy & Security and re-run.

`--source mic+system` writes a single mono `audio.opus` interleaving
mic and system frames by arrival time. Stereo encoding (mic on L,
system on R) is a v1.0.x → v1.1 deliverable; the transcript channel-
split via `FrameSource` is unaffected.

What runs:

- `scrybe-capture-mic::MicCapture` opens the host's default input device via cpal on a dedicated capture thread. The first run prompts for Microphone permission via macOS System Settings → Privacy & Security → Microphone; grant it and re-run.
- The pipeline chunks the audio at the existing 30 s / 5 s-silence-after-5 s-speech boundaries (`docs/system-design.md` §5).
- Each chunk is resampled to 16 kHz and handed to `WhisperLocalProvider`, which transcribes via whisper.cpp against your model file.
- The notes step uses the backend in `[record].llm`. The `mac-local`
  profile sets it to `openai-compat`; the default profile keeps the
  stub LLM for hermetic smoke tests.

Whisper model sizes (English-only, `.en` suffix; multilingual variants are larger):

| Model | File size | RAM use | Speed on M1 Pro | Use when |
|---|---|---|---|---|
| `ggml-tiny.en.bin` | ~75 MB | ~390 MB | ~30× realtime | Quick smoke test only |
| `ggml-base.en.bin` | ~150 MB | ~500 MB | ~16× realtime | Reasonable default |
| `ggml-small.en.bin` | ~470 MB | ~1.0 GB | ~6× realtime | Better accuracy |
| `ggml-large-v3-turbo.bin` | ~1.5 GB | ~3.0 GB | ~2× realtime | Production quality |

The `--whisper-model` flag rejects `*.partial` paths so an interrupted download cannot silently produce a corrupt transcript.

`meta.toml` records the actual loaded model in `[providers].stt`
(e.g. `whisper-local:ggml-base.en` for the `ggml-base.en.bin`
example above). The `scrybe retranscribe` flow planned for v1.x
uses this string as the canonical previous-attempt identifier.

System audio capture (the other end of a Zoom/Teams/Meet call) on macOS goes through `scrybe-capture-mac` (Core Audio Taps); wiring it into `scrybe record` alongside the mic adapter is a v1.x deliverable.

---

## Real notes summaries via Ollama / OpenAI-compat

`notes.md` is generated by the LLM provider at `SessionEnd`. When no
title is supplied, scrybe first asks the same LLM for a short factual
session title, rewrites the transcript/notes/meta headers with that
title, and renames the folder to `YYYY-MM-DD-HHMM-title-ULID`.

The default backend is `stub`, which writes a fixed templated body so
CI smoke tests stay hermetic. To produce real summaries by default,
build with `--features llm-openai-compat` and write the local profile:

```sh
# Build with the LLM-OpenAI-compat feature
cargo install --path scrybe-cli \
  --features cli-shell,hook-git,mic-capture,whisper-local,encoder-opus,llm-openai-compat

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

Without `--features llm-openai-compat`, `--llm openai-compat` errors at start time rather than silently falling back to the stub — same hard-error pattern as `--whisper-model` without `--features whisper-local` (v1.0.1).

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

Each GitHub Release ships a cosign-signed `SHA256SUMS.txt` covering every artifact and a separately-signed `scrybe-cli-sbom.cdx.json` (CycloneDX SBOM). Verifying the manifest's signature transitively covers every asset whose hash appears in the file — there is no need to verify each tarball individually.

Install cosign once (any 2.x release works):

```sh
brew install cosign            # macOS
# or download from https://github.com/sigstore/cosign/releases
```

Download the manifest, its signature, and its certificate from the release page:

```sh
TAG=v1.0.0   # the release you are verifying
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
  --certificate scrybe-cli-sbom.cdx.json.pem \
  --signature scrybe-cli-sbom.cdx.json.sig \
  --certificate-identity-regexp "^https://github.com/Mathews-Tom/scrybe/.github/workflows/release.yml@refs/tags/${TAG}$" \
  --certificate-oidc-issuer https://token.actions.githubusercontent.com \
  scrybe-cli-sbom.cdx.json
```

cosign is artifact-level CI provenance, not OS-level code signing. Gatekeeper's "Apple cannot verify" prompt and Windows SmartScreen are unaffected by a cosign-verified tarball — the install paths above remain the way to handle each.

---

## Verify reproducibility

`.github/workflows/reproducibility.yml` builds each release tarball twice on a fresh macOS-14 runner from divergent workspace paths and compares SHA256 across legs. The release workflow pins `SOURCE_DATE_EPOCH=1714464000`, sets `RUSTFLAGS=--remap-path-prefix=$workspace=/build -C link-args=-Wl,-no_uuid`, and locks the toolchain to `1.95.0` via `rust-toolchain.toml`.

The lane runs in **advisory mode** at v1.0.0 — see `CHANGELOG.md` "Known limitations" and `MAINTENANCE.md` §5 for the rationale. The four inputs above are not yet sufficient to make cargo-dist tarballs bit-identical on `macos-14`; tracking down the residual non-determinism is a v1.0.x → v1.1 follow-up. Both legs' artifacts upload on every run so an investigator can pull them down and run `diffoscope leg-a/scrybe leg-b/scrybe` to localise the divergence.

Local reproduction recipe (matches the CI inputs):

```sh
git clone --branch v1.0.0 https://github.com/Mathews-Tom/scrybe.git scrybe
cd scrybe
SOURCE_DATE_EPOCH=1714464000 \
  RUSTFLAGS="--remap-path-prefix=$(pwd)=/build -C link-args=-Wl,-no_uuid" \
  cargo dist build --artifacts=local --target=aarch64-apple-darwin
shasum -a 256 target/distrib/scrybe-cli-aarch64-apple-darwin.tar.xz
```

Comparison against a published release tag's `SHA256SUMS.txt` is informative but not yet authoritative — until the v1.0.x reproducibility-hardening lands, divergences here are expected. File an issue with `xcodebuild -showsdks` and `rustc -vV` if you investigate; the diffoscope output is the load-bearing artifact.

---

## Linux

Linux distribution surfaces — `cargo deb`, AUR `scrybe-bin`, Flathub — land in the v1.0.x stream as templates in `packaging/` are submitted to each downstream registry. The audit-friendly path on Linux today is `cargo install --git https://github.com/Mathews-Tom/scrybe scrybe-cli --tag v1.0.0 --features cli-shell,hook-git`, which builds locally against the pinned toolchain and never crosses a vendor's trust path.

## Windows

Windows distribution surfaces — `cargo wix` MSI, Scoop bucket — land in the v1.0.x stream alongside the `windows-latest` cargo-dist target. The current path is `cargo install --git https://github.com/Mathews-Tom/scrybe scrybe-cli --tag v1.0.0 --features cli-shell,hook-git`. Direct-download tarballs (when available) trigger SmartScreen's "Windows protected your PC" dialog because the binary is unsigned per `.docs/development-plan.md` §13.1. Click `More info → Run anyway` once; subsequent launches do not prompt.
