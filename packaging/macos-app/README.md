# macOS `.app` bundle

Source-of-truth artifacts for wrapping the `scrybe` CLI into a macOS application bundle. The bundle is required — not optional — for system-audio capture on macOS 14.4 and later: TCC (Transparency, Consent, and Control) refuses to surface an Audio Capture consent prompt against a bare CLI binary, so `--source mic+system` recordings made from `~/.cargo/bin/scrybe` directly receive zero-filled buffers from the Core Audio Tap.

This directory holds the pieces; `build-app.sh` glues them together.

## Files

| File | Purpose |
|---|---|
| `Info.plist.template` | Bundle metadata. Carries `NSAudioCaptureUsageDescription` and `NSMicrophoneUsageDescription`, the strings TCC reads when surfacing consent. `{{VERSION}}` is replaced at build time with the version reported by `scrybe --version`. |
| `entitlements.plist` | Code-signing entitlements. Declares `com.apple.security.device.audio-input` for tap delivery under the hardened runtime, plus the JIT/library-validation relaxations whisper-rs needs at inference time. |
| `build-app.sh` | Renders the template, copies the binary into `Contents/MacOS/`, optionally code-signs against either a real Developer ID identity or a self-signed Keychain identity, and runs `codesign --verify`. |

## Why a bundle is required

A bare Mach-O at `~/.cargo/bin/scrybe` cannot receive Audio Capture consent. The OS-level chain is:

1. Process calls `AudioHardwareCreateProcessTap` and starts the IOProc.
2. Apple's audio framework checks the calling binary's TCC record for `kTCCServiceAudioCapture`.
3. Without a bundle and `Info.plist`, no consent record can be created — the `NSAudioCaptureUsageDescription` string is what populates the system prompt.
4. With no record, the framework defaults to "deny" but does not return an error. It substitutes zero-filled buffers and keeps the IOProc running on schedule.

The result: `scrybe doctor --check-tap` reports `frames=141 peak=0.0000` — frames flow at the expected ~94 Hz cadence, but every sample is exactly `0.0`. That signature is unambiguous: the OS is stripping the audio at the entitlement boundary, not before it reaches us.

## Self-signed cert workflow (free, no Apple Developer membership)

A self-signed certificate created in Keychain Access satisfies TCC's csreq check without paying Apple. The certificate has no chain of trust and Gatekeeper will warn on first launch, but for local development that is acceptable.

```text
Keychain Access → Certificate Assistant → Create a Certificate
  Name: scrybe-local-signing
  Identity Type: Self Signed Root
  Certificate Type: Code Signing
  ✓ Let me override defaults    → Continue
  Validity period: 3650 days     → Continue through remaining defaults
```

Verify the cert is in your keychain:

```sh
security find-identity -v -p codesigning | grep scrybe-local-signing
```

Then build and sign the bundle:

```sh
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
```

A macOS dialog appears asking permission to capture system audio. Click **Allow**. Re-run the probe — `peak > 0.01` should appear in the verdict.

## Developer ID workflow (for distribution)

For builds intended to ship to other machines via the GitHub Releases tarball, replace `--sign-self` with `--sign` and pass the full Developer ID Application identity:

```sh
packaging/macos-app/build-app.sh \
    --binary "$HOME/.cargo/bin/scrybe" \
    --output ./scrybe.app \
    --sign "Developer ID Application: Your Name (TEAMID)"
```

The Developer ID requires a paid Apple Developer membership ($99/year). Notarization is a separate step and is currently out of scope for the v1 release line per `README.md:151`.

## Iteration loop

Each `cargo install --path scrybe-cli --force` rewrites the binary at `~/.cargo/bin/scrybe`. Re-run `build-app.sh` after every install to refresh the bundle:

```sh
alias scrybe-rebundle='packaging/macos-app/build-app.sh \
    --binary "$HOME/.cargo/bin/scrybe" \
    --output ./scrybe.app \
    --sign-self scrybe-local-signing'
```

Because the Subject Common Name in the cert is stable, the rebuilt bundle's designated requirement matches the existing TCC grant — no second permission prompt.

## Verifying

After signing:

```sh
codesign -dv --verbose=4 ./scrybe.app 2>&1 | grep -E 'Authority|Identifier|Signature|Sealed'
codesign --verify --deep --strict --verbose=2 ./scrybe.app
spctl --assess --type execute --verbose ./scrybe.app   # Gatekeeper view
```

For self-signed builds `spctl` reports "rejected (the code is valid but does not seem to be an app that has been signed by a certificate trusted to sign applications)" — that is expected and does not block local launches.
