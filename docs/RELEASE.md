# Release Runbook

This runbook publishes the Scrybe application to crates.io and GitHub from one reviewed commit. crates.io versions are immutable. Complete every reversible check before the first `cargo publish`.

## Publish Graph

Publish v1.6.0 packages in this order:

1. `scrybe-meeting-core`
2. `scrybe-meeting-application`
3. `scrybe-meeting-capture-mac`
4. `scrybe-meeting-capture-mic`
5. `scrybe`

`scrybe-meeting-application` depends on core and must follow it. The two capture packages are independent after core is visible. The application must remain last, and it pins `scrybe-meeting-application = "=1.6.0"`, so `cargo publish -p scrybe --locked` cannot resolve until that package is on the registry.

## Preflight

Work from synchronized `main` with a clean tree:

```sh
git switch main
git pull --ff-only origin main
git status --short
```

Confirm main CI is green and the release commit is the expected v1.6.0 preparation commit:

```sh
gh run list --branch main --limit 5 --json status,conclusion,workflowName,headSha
git log --oneline -5
```

Confirm the crates.io credential file exists without printing its contents:

```sh
test -f ~/.cargo/credentials.toml
cargo owner --list scrybe
cargo owner --list scrybe-meeting-core
cargo owner --list scrybe-meeting-application
cargo owner --list scrybe-meeting-capture-mac
cargo owner --list scrybe-meeting-capture-mic
```

Every owner listing must include `Mathews-Tom`. Never print or paste the registry token.

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
cargo info scrybe-meeting-core@1.6.0 --registry crates-io
cargo info scrybe-meeting-application@1.6.0 --registry crates-io
cargo info scrybe-meeting-capture-mac@1.6.0 --registry crates-io
cargo info scrybe-meeting-capture-mic@1.6.0 --registry crates-io
cargo info scrybe@1.6.0 --registry crates-io
```

The expected result for each exact version is “could not find”. Stop if any immutable `1.6.0` package already exists.

## Package Inspection

Assemble the five packages together so Cargo can resolve their unpublished workspace dependencies:

```sh
cargo package \
  -p scrybe-meeting-core \
  -p scrybe-meeting-application \
  -p scrybe-meeting-capture-mac \
  -p scrybe-meeting-capture-mic \
  -p scrybe \
  --locked \
  --allow-dirty \
  --no-verify
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

## Publish to crates.io

Dry-run and publish core:

```sh
cargo publish -p scrybe-meeting-core --dry-run --locked
cargo publish -p scrybe-meeting-core --locked
```

Wait until the exact version resolves:

```sh
cargo info scrybe-meeting-core@1.6.0 --registry crates-io
```

Then dry-run and publish the shared application services, which depend on core:

```sh
cargo publish -p scrybe-meeting-application --dry-run --locked
cargo publish -p scrybe-meeting-application --locked
```

Wait until the exact version resolves:

```sh
cargo info scrybe-meeting-application@1.6.0 --registry crates-io
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
cargo info scrybe-meeting-capture-mac@1.6.0 --registry crates-io
cargo info scrybe-meeting-capture-mic@1.6.0 --registry crates-io
```

Dry-run and publish the application last:

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

Both version commands must report `scrybe 1.6.0`. `doctor` and `record --help` must execute without a repository checkout. Run Doctor from a terminal and decline the optional live permission probe during this registry-only acceptance; the release's hardware qualification covers the real probes separately.

## Publish the GitHub Release

Create an annotated tag on the same commit used for crates.io:

```sh
git tag -a v1.6.0 -m "v1.6.0"
git push origin v1.6.0
```

The tag triggers `.github/workflows/release.yml`. Wait for its plan, Apple Silicon build, Intel build, and release jobs:

```sh
gh run list --workflow release.yml --limit 1
gh release view v1.6.0
```

Download every asset into an empty directory and verify `SHA256SUMS.txt`. Execute the installed published binary, confirm its Mach-O UUID, and run the self-signed bundle smoke from `INSTALL.md`.

Expected v1.6.0 asset names include:

- `scrybe-aarch64-apple-darwin.tar.xz`
- `scrybe-x86_64-apple-darwin.tar.xz`
- `scrybe-installer.sh`
- `dist-manifest.json`
- `scrybe-sbom.cdx.json`
- `SHA256SUMS.txt`

## Recovery

If a crates.io upload succeeds, that package version cannot be replaced. Yank a defective version only to prevent new resolution:

```sh
cargo yank --vers 1.6.0 scrybe
```

Fix forward with a new patch version when accepted bytes are defective. Never move or recreate an existing release tag after users can install its crates.io package.

If the GitHub workflow fails before publishing a usable release, fix the workflow on main and cut a new patch version. Do not retag v1.6.0 after crates.io publication because the registry package and source tag must remain permanently aligned.
