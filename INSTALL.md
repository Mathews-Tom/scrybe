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
TAG=v0.9.0-rc1   # the release you are verifying
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

The lane runs in **advisory mode** at v0.9.0-rc1. The four inputs above are not yet sufficient to make cargo-dist tarballs bit-identical on `macos-14`; tracking down the residual non-determinism is a v0.9.x → v1.0 follow-up. Both legs' artifacts upload on every run so an investigator can pull them down and run `diffoscope leg-a/scrybe leg-b/scrybe` to localise the divergence.

Local reproduction recipe (matches the CI inputs):

```sh
git clone --branch v0.9.0-rc1 https://github.com/Mathews-Tom/scrybe.git scrybe
cd scrybe
SOURCE_DATE_EPOCH=1714464000 \
  RUSTFLAGS="--remap-path-prefix=$(pwd)=/build -C link-args=-Wl,-no_uuid" \
  cargo dist build --artifacts=local --target=aarch64-apple-darwin
shasum -a 256 target/distrib/scrybe-cli-aarch64-apple-darwin.tar.xz
```

Comparison against a published release tag's `SHA256SUMS.txt` is informative but not yet authoritative — until the v0.9.x reproducibility-hardening lands, divergences here are expected. File an issue with `xcodebuild -showsdks` and `rustc -vV` if you investigate; the diffoscope output is the load-bearing artifact.

---

## Linux

Linux distribution surfaces — `cargo deb`, AUR `scrybe-bin`, Flathub — land in the v0.9.x stream as templates in `packaging/` are submitted to each downstream registry. The audit-friendly path on Linux today is `cargo install --git https://github.com/Mathews-Tom/scrybe scrybe-cli --tag v0.9.0-rc1 --features cli-shell,hook-git`, which builds locally against the pinned toolchain and never crosses a vendor's trust path.

## Windows

Windows distribution surfaces — `cargo wix` MSI, Scoop bucket — land in the v0.9.x stream alongside the `windows-latest` cargo-dist target. The current path is `cargo install --git https://github.com/Mathews-Tom/scrybe scrybe-cli --tag v0.9.0-rc1 --features cli-shell,hook-git`. Direct-download tarballs (when available) trigger SmartScreen's "Windows protected your PC" dialog because the binary is unsigned per `.docs/development-plan.md` §13.1. Click `More info → Run anyway` once; subsequent launches do not prompt.
