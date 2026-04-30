# Installing scrybe

scrybe ships unsigned binaries through v1.0. macOS Gatekeeper and the equivalent SmartScreen prompt on Windows are addressed at install time rather than at build time — Apple Developer ID enrollment and Windows code-signing certificates are deliberately out of scope (`.docs/development-plan.md` §13.1).

This document covers macOS today. Linux and Windows installation paths land in their respective releases.

---

## macOS — install from the GitHub Release

### 1. Pick the right tarball

Each release publishes two macOS archives at `https://github.com/Mathews-Tom/scrybe/releases`:

| Archive | When to download |
|---|---|
| `scrybe-cli-aarch64-apple-darwin.tar.xz` | Apple Silicon Macs (M1, M2, M3, M4) |
| `scrybe-cli-x86_64-apple-darwin.tar.xz` | Intel Macs |

`uname -m` answers which one you need: `arm64` → aarch64, `x86_64` → x86_64.

### 2. Verify the archive

Each archive ships a SHA256 sum on the release page. Confirm the download:

```sh
shasum -a 256 scrybe-cli-aarch64-apple-darwin.tar.xz
# Compare against the SHA256 line on the GitHub Release page.
```

### 3. Extract and place the binary

```sh
tar -xf scrybe-cli-aarch64-apple-darwin.tar.xz
mkdir -p ~/.local/bin
mv scrybe-cli-aarch64-apple-darwin/scrybe ~/.local/bin/
```

If `~/.local/bin` is not on `$PATH`, add it (`export PATH="$HOME/.local/bin:$PATH"` in `~/.zshrc`).

### 4. Remove the Gatekeeper quarantine attribute

The first time macOS sees an unsigned binary downloaded from a browser, the kernel attaches `com.apple.quarantine` to it. Launching produces a dialog reading "Apple cannot verify this app is free of malware." Remove the attribute once, on the binary itself:

```sh
xattr -dr com.apple.quarantine ~/.local/bin/scrybe
```

`-d` deletes the attribute; `-r` recurses if you ever extract into a `.app` bundle (scrybe-cli ships a single binary, so the recurse flag is harmless). The first launch after this no longer prompts.

If you re-download the archive in a new browser session, repeat step 4. Quarantine is per-download, not per-binary.

### 5. Verify it runs

```sh
scrybe --version
scrybe doctor
```

`scrybe doctor` checks permission state, disk space, and reports any missing prerequisites.

---

## macOS — build from source (audit-friendly path)

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

---

## macOS — quick reference

```sh
# Apple Silicon
curl -LO https://github.com/Mathews-Tom/scrybe/releases/latest/download/scrybe-cli-aarch64-apple-darwin.tar.xz
tar -xf scrybe-cli-aarch64-apple-darwin.tar.xz
mkdir -p ~/.local/bin && mv scrybe-cli-aarch64-apple-darwin/scrybe ~/.local/bin/
xattr -dr com.apple.quarantine ~/.local/bin/scrybe
scrybe doctor
```

```sh
# Intel Mac
curl -LO https://github.com/Mathews-Tom/scrybe/releases/latest/download/scrybe-cli-x86_64-apple-darwin.tar.xz
tar -xf scrybe-cli-x86_64-apple-darwin.tar.xz
mkdir -p ~/.local/bin && mv scrybe-cli-x86_64-apple-darwin/scrybe ~/.local/bin/
xattr -dr com.apple.quarantine ~/.local/bin/scrybe
scrybe doctor
```

---

## Why no notarization?

macOS notarization requires an Apple Developer ID enrollment ($99/year) and ties the project's release pipeline to a vendor account. `.docs/development-plan.md` §13.1 documents the trade: until scrybe has demonstrated longevity, vendor-tied trust dependencies stay deferred. The `xattr -dr com.apple.quarantine` step is the documented user-side workaround. The audit-friendly persona is steered to `cargo install --path scrybe-cli` from a verified clone, which does not interact with Gatekeeper at all.

This posture is reviewed post-v1.0 if first-run friction is shown to materially block adoption.
