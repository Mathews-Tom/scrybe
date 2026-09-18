# Release Runbook

This runbook publishes the Scrybe application to crates.io and GitHub from one reviewed commit. crates.io versions are immutable. Complete every reversible check before the first `cargo publish`.

## Publish Graph

The set and its order live in [`publish-order.md`](publish-order.md), generated from the workspace by `scripts/publish-order.py`. Read it before authorizing anything; it is the one place in this runbook that is derived rather than transcribed.

```sh
python3 scripts/publish-order.py         # the order
cat docs/publish-order.md                # the artifact, with registry state
```

This section used to hold the order as a typed list, and that list omitted `scrybe-widgets` — which `scrybe` names with an exact version, and which therefore has to be on the registry before `cargo publish -p scrybe --locked` can resolve. The identical omission in `.github/workflows/ci.yml` is what broke `cargo package -p scrybe`. A list nobody derives is a list that goes stale silently, so there is no longer one here.

Two packages in the set **have never been published**: `scrybe-meeting-application` and `scrybe-widgets`. Their first publication happens whenever a release version is assigned. A first publication claims the name permanently, cannot be undone, and has no prior version to diff against or roll back to. This is the most consequential fact on this page.

## Preflight

Work from synchronized `main` with a clean tree:

```sh
git switch main
git pull --ff-only origin main
git status --short
```

Confirm main CI is green and the release commit is the expected v2.1.0 preparation commit:

```sh
gh run list --branch main --limit 5 --json status,conclusion,workflowName,headSha
git log --oneline -5
```

Confirm the crates.io credential file exists without printing its contents:

```sh
test -f ~/.cargo/credentials.toml
python3 scripts/publish-order.py --names | while read -r name; do
    echo "== $name"
    cargo owner --list "$name" || echo "   (no such package on the registry)"
done
```

Every package already on the registry must list `Mathews-Tom`. The two that have never been published have no owners to list and report that they do not exist — which is the expected answer for them and the signal to re-read [`publish-order.md`](publish-order.md) before continuing. Never print or paste the registry token.

Confirm every surface that carries a version still agrees with the one in `[workspace.package]`:

```sh
python3 scripts/check-version-agreement.py
```

The gate reads the source of truth once and holds thirty-two surfaces to it: every member manifest inherits rather than restates, `cargo metadata` resolves each member to it, the desktop host manifest and `tauri.conf.json` carry it because a separate workspace and a JSON literal cannot inherit, and all eleven intra-workspace `=` pins match it. A disagreement names the file that holds it. Nothing here is transcribed by hand — `--write` propagates, and this check is what makes a failed propagation visible.

One surface the source-only run cannot reach is the artifact. After a bundle exists (see [Package Inspection](#package-inspection)), confirm what it reports about itself:

```sh
python3 scripts/check-version-agreement.py \
    --bundle scrybe-desktop/src-tauri/target/debug/bundle/macos/Scrybe.app
```

`CFBundleShortVersionString` and `CFBundleVersion` must both report the workspace version. Until this increment they reported `0.0.0` — a placeholder unrelated to anything the CLI shipped — so a bundle that still reports `0.0.0` is a bundle built before the propagation landed, not a bundle that disagrees.

The Linux, Windows, and Android adapter packages remain private; they inherit the version like every other member, which costs nothing and removes them as a place drift can hide.

Confirm that the target version is still absent from every published package immediately before publication:

```sh
VERSION="$(python3 -c 'import tomllib,pathlib; print(tomllib.loads(pathlib.Path("Cargo.toml").read_text())["workspace"]["package"]["version"])')"
python3 scripts/publish-order.py --names | while read -r name; do
    echo "== $name@$VERSION"
    cargo info "$name@$VERSION" --registry crates-io || true
done
```

The version comes from the same `[workspace.package].version` every manifest inherits, so this block cannot check a version other than the one about to be packaged. The expected result for each exact version is “could not find”. Stop if any immutable package at that version already exists.

## Package Inspection

Assemble the whole set together so Cargo can resolve the workspace dependencies that are not on the registry yet. Take the set from the derivation rather than naming packages, which is what `.github/workflows/ci.yml`'s `dist-plan` job does:

```sh
PACKAGES=()
while IFS= read -r token; do PACKAGES+=("$token"); done \
  < <(python3 scripts/publish-order.py --cargo-args)
echo "derived publish set: ${PACKAGES[*]}"
cargo package "${PACKAGES[@]}" --locked --allow-dirty --no-verify
```

Inspect every `.crate` archive and its normalized `Cargo.toml`. Confirm source, tests, README, license metadata, exact internal dependency versions, and package names. Reject credentials, local session artifacts, generated evidence, absolute paths, or undeclared files.

Run the full repository gate and a clean isolated path install before uploading:

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets --no-default-features -- -D warnings
cargo check --workspace --all-targets --no-default-features
cargo test --workspace --all-targets --no-default-features
python3 scripts/check-egress-baseline.py
python3 scripts/check-loc-budget.py
dist plan --output-format=human
```

The release gate also requires the published-default macOS Clippy, test, release build, and isolated installation checks documented in `INSTALL.md`.

Complete [Native Application Qualification](#native-application-qualification) before the first immutable crates.io upload.

## Publish to crates.io

Dry-run and publish core:

```sh
cargo publish -p scrybe-meeting-core --dry-run --locked
cargo publish -p scrybe-meeting-core --locked
```

Wait until the exact version resolves:

```sh
cargo info scrybe-meeting-core@2.1.0 --registry crates-io
```

Then dry-run and publish the shared application services, which depend on core:

```sh
cargo publish -p scrybe-meeting-application --dry-run --locked
cargo publish -p scrybe-meeting-application --locked
```

Wait until the exact version resolves:

```sh
cargo info scrybe-meeting-application@2.1.0 --registry crates-io
```

Then dry-run and publish the capture packages:

```sh
cargo publish -p scrybe-meeting-capture-mac --dry-run --locked
cargo publish -p scrybe-meeting-capture-mic --dry-run --locked
cargo publish -p scrybe-meeting-capture-mac --locked
cargo publish -p scrybe-meeting-capture-mic --locked
```

Wait until both exact versions resolve:

```sh
cargo info scrybe-meeting-capture-mac@2.1.0 --registry crates-io
cargo info scrybe-meeting-capture-mic@2.1.0 --registry crates-io
```

Then dry-run and publish the presentation surfaces:

```sh
cargo publish -p scrybe-widgets --dry-run --locked
cargo publish -p scrybe-widgets --locked
```

Wait until the exact version resolves:

```sh
cargo info scrybe-widgets@2.1.0 --registry crates-io
```

Dry-run and publish the application last. It pins every package above with an exact version, so it cannot resolve until all of them are on the registry:

```sh
cargo publish -p scrybe --dry-run --locked
cargo publish -p scrybe --locked
```

Do not proceed to the GitHub tag until crates.io installs the application twice from separate empty Cargo homes. The first command verifies the documented default resolver; the second verifies the package lockfile qualified for publication:

```sh
UNLOCKED_CARGO_HOME="$(mktemp -d)"
UNLOCKED_INSTALL_ROOT="$(mktemp -d)"
CARGO_HOME="$UNLOCKED_CARGO_HOME" rustup run 1.95.0 cargo install scrybe --root "$UNLOCKED_INSTALL_ROOT"
"$UNLOCKED_INSTALL_ROOT/bin/scrybe" --version
"$UNLOCKED_INSTALL_ROOT/bin/scrybe" doctor
"$UNLOCKED_INSTALL_ROOT/bin/scrybe" record --help

LOCKED_CARGO_HOME="$(mktemp -d)"
LOCKED_INSTALL_ROOT="$(mktemp -d)"
CARGO_HOME="$LOCKED_CARGO_HOME" rustup run 1.95.0 cargo install scrybe --locked --root "$LOCKED_INSTALL_ROOT"
"$LOCKED_INSTALL_ROOT/bin/scrybe" --version
```

Both version commands must report `scrybe 2.1.0`. `doctor` and `record --help` must execute without a repository checkout. Run Doctor from a terminal and decline the optional live permission probe during this registry-only acceptance; the release's hardware qualification covers the real probes separately.

## Publish the GitHub Release

Create an annotated tag on the same commit used for crates.io:

```sh
git tag -a v2.1.0 -m "v2.1.0"
git push origin v2.1.0
```

The tag triggers `.github/workflows/release.yml`. Wait for its plan, Apple Silicon build, Intel build, and release jobs:

```sh
gh run list --workflow release.yml --limit 1
gh release view v2.1.0
```

Download every asset into an empty directory and verify `SHA256SUMS.txt`. Execute the installed published binary, confirm its Mach-O UUID, and run the self-signed bundle smoke from `INSTALL.md`.

Expected v2.1.0 asset names include:

- `scrybe-aarch64-apple-darwin.tar.xz`
- `scrybe-x86_64-apple-darwin.tar.xz`
- `scrybe-installer.sh`
- `dist-manifest.json`
- `scrybe-sbom.cdx.json`
- `SHA256SUMS.txt`
- `Scrybe_2.1.0_aarch64.dmg`
- `Scrybe_2.1.0_x86_64.dmg`
- `Scrybe_2.1.0_aarch64.app.tar.gz`
- `Scrybe_2.1.0_aarch64.app.tar.gz.sig`
- `Scrybe_2.1.0_x86_64.app.tar.gz`
- `Scrybe_2.1.0_x86_64.app.tar.gz.sig`
- `latest.json`
- `scrybe-desktop-sbom.cdx.json`

## Native Application Qualification

Run every desktop scenario from a real macOS login session:

```sh
python3 scripts/qualify-desktop-app.py --hermetic --scenario lifecycle
python3 scripts/qualify-desktop-app.py --hermetic --scenario setup
python3 scripts/qualify-desktop-app.py --hermetic --scenario model-download-live
python3 scripts/qualify-desktop-app.py --hermetic --scenario library
python3 scripts/qualify-desktop-app.py --hermetic --scenario recording
python3 scripts/qualify-desktop-app.py --hermetic --scenario installed --trust-profile community
python3 scripts/qualify-desktop-app.py --hermetic --scenario retention
```

Every scenario must report `ok`. `model-download-live` is release-only: it downloads the checked-in production artifact from its immutable Hugging Face URL, verifies all 487,614,201 bytes and the catalog SHA-256, atomically promotes it inside a disposable model root, and deletes that root afterward. Do not run it in routine CI.

The installed scenario needs a human-granted Accessibility permission for the invoking terminal. It reads the status item's `Scrybe` name and all tray actions from the native accessibility tree. It then applies the community trust contract, which must prove all of these facts:

- the app is signed with `Scrybe Community Distribution`;
- the embedded certificate SHA-256 is `a75f69039cdc924ab3b211f7ec973e541d24895d742819bed8576d4e815f5570`;
- the bundle identifier is `dev.scrybe.desktop`;
- `codesign` reports an internally valid signature;
- Gatekeeper reports `rejected`;
- no Apple notarization ticket is stapled.

Run the artifact gate over both release surfaces:

```sh
python3 scripts/check-community-artifact.py \
  --bundle scrybe-desktop/src-tauri/target/release/bundle/macos/Scrybe.app \
  --dmg scrybe-desktop/src-tauri/target/release/bundle/dmg/Scrybe_${VERSION}_${ARCH}.dmg
```

Gatekeeper rejection is an expected limitation, not a skipped check and not an Apple-trust claim. The application is self-signed and unnotarized because the project has no paid Apple Developer Program membership. Users must verify the signed checksum manifest and follow Apple's documented Open Anyway flow in `INSTALL.md`.

## Updater Trust

Updater authenticity is independent of macOS platform trust. Tauri signs the compressed application archive with the offline updater key; the application contains only its public key. The release must publish the archive, its `.sig`, and `latest.json`. Verify the signature independently before uploading.

`scripts/check-updater-continuity.py` compares the prior and candidate applications. It must report the same bundle identifier, designated requirement, embedded certificate fingerprint, and entitlements; a sentinel under the user-data root must retain its pre-update SHA-256. The candidate version must advance. Re-signing with any other certificate must make this check fail.

The updater UI remains user-initiated and must refuse installation while a recording is preparing, active, or saving. It may install only from idle.

## Future Apple-Trusted Profile

The stronger gate remains available without weakening the community profile:

```sh
python3 scripts/qualify-desktop-app.py \
  --hermetic \
  --scenario installed \
  --trust-profile apple-trusted
python3 scripts/check-signed-artifact.py --bundle ./Scrybe.app
```

That profile requires a Developer ID Application identity, an Apple-accepted notarization submission, a stapled ticket, and a Gatekeeper `accepted` verdict. It remains deferred until paid Apple enrollment is explicitly approved. Never describe the community certificate, Sigstore provenance, or updater signature as a substitute for Apple notarization.

## Recovery

If a crates.io upload succeeds, that package version cannot be replaced. Yank a defective version only to prevent new resolution:

```sh
cargo yank --vers 2.1.0 scrybe
```

Fix forward with a new patch version when accepted bytes are defective. Never move or recreate an existing release tag after users can install its crates.io package.

If the GitHub workflow fails before publishing a usable release, fix the workflow on main and cut a new patch version. Do not retag v2.1.0 after crates.io publication because the registry package and source tag must remain permanently aligned.
