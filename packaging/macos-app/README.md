# macOS `.app` bundle

Source-checkout tooling for wrapping the `scrybe` CLI into a macOS application bundle. The bundle is required only for the Core Audio Tap recovery backend on macOS 14.4 and later: TCC (Transparency, Consent, and Control) refuses to surface an Audio Capture consent prompt against a bare CLI binary, so Tap recordings made from `~/.cargo/bin/scrybe` directly receive zero-filled buffers. The default ScreenCaptureKit backend runs from the invoking terminal and requires no bundle or signing identity.

`build-app.sh` consumes the canonical templates embedded in the publishable `scrybe` package under `scrybe-cli/assets/macos/`. Normal users should run `scrybe doctor`; the script remains the source-checkout packaging surface.

## Files

| File | Purpose |
|---|---|
| `../../scrybe-cli/assets/macos/Info.plist.template` | Bundle metadata. Carries `NSAudioCaptureUsageDescription` and `NSMicrophoneUsageDescription`, the strings TCC reads when surfacing consent. `{{VERSION}}` is replaced at build time with the version reported by `scrybe --version`. |
| `../../scrybe-cli/assets/macos/entitlements.plist` | Code-signing entitlements. Declares `com.apple.security.device.audio-input` for tap delivery under the hardened runtime, plus the JIT/library-validation relaxations whisper-rs needs at inference time. |
| `build-app.sh` | Renders the template, copies the binary into `Contents/MacOS/`, and code-signs. Exactly one signing mode is required — `--sign` for a Developer ID, `--sign-self` for local development, `--unsigned` for a deliberately unsigned local bundle — and there is no default. A `--sign` build is then put through `../../scripts/check-signed-artifact.py`, which reads the signing authority by name. |
| `../../scripts/check-signed-artifact.py` | The signature, Gatekeeper, and notarization assertions. Reads `Authority` and `TeamIdentifier` out of `codesign -dvvv`, parses the verdict word out of `spctl`, and reads Apple's own submission status and a stapled ticket. Never `codesign --verify`, and never an exit status. |

## Why Core Audio Tap requires a bundle

A bare Mach-O at `~/.cargo/bin/scrybe` cannot receive Audio Capture consent. The OS-level chain is:

1. Process calls `AudioHardwareCreateProcessTap` and starts the IOProc.
2. Apple's audio framework checks the calling binary's TCC record for `kTCCServiceAudioCapture`.
3. Without a bundle and `Info.plist`, no consent record can be created — the `NSAudioCaptureUsageDescription` string is what populates the system prompt.
4. With no record, the framework defaults to "deny" but does not return an error. It substitutes zero-filled buffers and keeps the IOProc running on schedule.

The result: `scrybe doctor --check-tap` reports `frames=141 peak=0.0000` — frames flow at the expected ~94 Hz cadence, but every sample is exactly `0.0`. That signature is unambiguous: the OS is stripping the audio at the entitlement boundary, not before it reaches us.

## Self-signed certificate workflow (free, no Apple Developer membership)

A self-signed certificate created in Keychain Access satisfies TCC's csreq check without paying Apple. The certificate has no chain of trust and Gatekeeper will warn on first launch, but it gives rebuilt local bundles a stable designated requirement.

```text
Keychain Access → Certificate Assistant → Create a Certificate
  Name: scrybe-local-signing
  Identity Type: Self Signed Root
  Certificate Type: Code Signing
  Let me override defaults → Continue
  Validity period: 3650 days → Continue through remaining defaults
```

Verify the certificate is in your keychain:

```sh
security find-identity -v -p codesigning
```

For a crates.io installation, let Doctor build, sign, verify, and launch the temporary bundle:

```sh
cargo install scrybe
scrybe doctor --check-tap --fix --sign-self scrybe-local-signing
```

Doctor accepts only the named identity and never auto-selects an unrelated Developer ID certificate. It validates a temporary sibling bundle before replacement and preserves an existing valid bundle when candidate construction or validation fails.

For development against a source checkout, install the current binary and invoke the same application-owned lifecycle:

```sh
cargo install --path scrybe-cli --force --locked \
    --features cli-shell,hook-git,mic-capture,system-capture-mac,whisper-local,encoder-opus,llm-openai-compat
scrybe install-macos-bundle --sign-self scrybe-local-signing --output ./scrybe.app
scrybe doctor --check-tap
```

A macOS dialog asks permission to capture system audio. Click **Allow**. Re-run the probe if the first grant flow does not yet return `peak > 0.01`.

## Optional Developer ID workflow

If the project later enables Developer ID distribution, the source-checkout script accepts the full Developer ID Application identity:

```sh
packaging/macos-app/build-app.sh \
    --binary "$HOME/.cargo/bin/scrybe" \
    --output ./scrybe.app \
    --sign "Developer ID Application: Your Name (TEAMID)"
```

The Developer ID requires a paid Apple Developer membership ($99/year).

`--sign` is the only mode that produces a shippable bundle, and the script asserts that afterwards rather than trusting that signing happened:

```sh
python3 scripts/check-signed-artifact.py --bundle ./scrybe.app
```

Three assertions, none of which can pass on a machine with no distribution credential:

- **Signature.** A Developer ID Application authority read by name out of `codesign -dvvv`, with a ten-character team identifier that the authority line and the `TeamIdentifier` field agree on.
- **Gatekeeper.** The verdict word parsed out of `spctl -a -vv --type execute`, required to be `accepted` from a notarized source.
- **Notarization.** The status Apple returns for a submission, required to be exactly `Accepted`, plus a ticket Apple issued stapled to the bundle.

`codesign --verify` is deliberately absent from all three. Measured on this repository's own output: an ad-hoc-signed bundle returns exit 0 from `codesign --verify --deep --strict` while `spctl` reports `rejected`. A gate built on it certifies an artifact Gatekeeper will not run. `spctl`'s own exit status is no better — the same `rejected` verdict was measured at exit 3 here and at exit 0 earlier — so the verdict is read from the text.

### Why no signing mode is the default any more

The script used to make signing optional: with no flags it printed a warning, built the bundle anyway, and finished with `codesign --verify`. So the ordinary way to run it produced an unsigned bundle that nothing downstream refused, and the last line of output was a verification that could not have caught it. Choosing a mode is now required, and the two development modes state on stdout that their output is not shippable.

Notarization is a separate step from signing and needs its own credential; the assertion for it exists and fails naming what is missing.

## Iteration loop

Each `cargo install --path scrybe-cli --force` rewrites the binary at `~/.cargo/bin/scrybe`. Refresh and validate the development bundle through the application-owned command:

```sh
scrybe install-macos-bundle \
    --sign-self scrybe-local-signing \
    --output ./scrybe.app
```

Because the Subject Common Name in the certificate is stable, the rebuilt bundle's designated requirement matches the existing TCC grant and avoids an unnecessary second permission prompt.

## Verifying

After signing:

```sh
codesign -dv --verbose=4 ./scrybe.app 2>&1 | grep -E 'Authority|Identifier|Signature|Sealed'
codesign --verify --deep --strict --verbose=2 ./scrybe.app
spctl --assess --type execute --verbose ./scrybe.app   # Gatekeeper view
```

For self-signed builds `spctl` reports "rejected (the code is valid but does not seem to be an app that has been signed by a certificate trusted to sign applications)" — that is expected and does not block local launches.
