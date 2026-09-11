# crates.io Installation Design

**Status:** Approved  
**Target release:** v1.3.2  
**Primary user contract:** `cargo install scrybe --locked`

## Problem

The functional Scrybe application is not installable from crates.io. The `scrybe` crate is a v0.1.0 placeholder with no binary. The workspace package that builds the application is named `scrybe-cli`, has `publish = false`, and depends on other unpublished workspace packages. An unrelated project owns the `scrybe-cli` and `scrybe-core` package names on crates.io.

GitHub Release installation works, but users who expect Cargo-native installation can either install the placeholder accidentally or install the unrelated `scrybe-cli` project. v1.3.2 must make `cargo install scrybe --locked` install the real meeting-recording application without cloning this repository.

## Decision

Use the reserved `scrybe` crates.io package for the application. Convert the existing CLI package into the `scrybe` package and remove the obsolete placeholder package. Publish only the internal packages required by the application, under unique package names that do not conflict with the unrelated Scrybe project.

Package mapping:

| Workspace directory | v1.3.2 package name | Rust crate name | Published |
| --- | --- | --- | --- |
| `scrybe-cli/` | `scrybe` | binary `scrybe` | yes |
| `scrybe-core/` | `scrybe-meeting-core` | `scrybe_core` | yes |
| `scrybe-capture-mac/` | `scrybe-meeting-capture-mac` | `scrybe_capture_mac` | yes |
| `scrybe-capture-mic/` | `scrybe-meeting-capture-mic` | `scrybe_capture_mic` | yes |
| `scrybe/` placeholder | removed | removed | no |
| Linux, Windows, Android adapters | unchanged | unchanged | no |

Cargo dependency aliases preserve existing Rust imports. For example, the dependency key remains `scrybe-core`, but its package is `scrybe-meeting-core`. Explicit `[lib]` names preserve `scrybe_core`, `scrybe_capture_mac`, and `scrybe_capture_mic` in source code.

All four published packages use version `1.3.2`. Their internal path dependencies use an exact `=1.3.2` registry requirement so `cargo package` replaces local paths with the intended crates.io version during publication.

## Installation Behavior

On supported macOS hosts:

```sh
cargo install scrybe --locked
scrybe --version
```

The bare install command enables the same production capabilities as the v1.3.1 GitHub binary:

- microphone capture;
- ScreenCaptureKit system-audio capture;
- local Whisper transcription;
- Ogg Opus encoding;
- OpenAI-compatible local or hosted notes;
- tray/global-hotkey shell support;
- Git hook support.

The application remains local-first at runtime: network providers are inactive until selected in configuration. The previous claim that the application package's default dependency graph contains no network-capable crates must be narrowed to the explicit hermetic build:

```sh
cargo build -p scrybe --no-default-features
```

The egress audit must measure that command rather than silently continuing to report an obsolete package/default contract.

The GitHub installer remains supported as the faster prebuilt path. Cargo installation builds native dependencies from source and therefore requires the Rust 1.95 toolchain and Xcode Command Line Tools. The package verification must reproduce a clean macOS build without relying on repository-only Cargo configuration or undeclared Homebrew libraries.

## Package and Source Changes

1. Remove the placeholder `scrybe/` workspace member and its placeholder-only constants/tests.
2. Rename the `scrybe-cli` package to `scrybe`, retain its `scrybe` binary target, and remove its dependency on the placeholder.
3. Rename the publishable internal package identities to the unique `scrybe-meeting-*` names while preserving existing Rust crate names with `[lib]` declarations.
4. Change only the required four packages from private to publishable. Linux, Windows, Android, benchmark-only, and unreleased provider packages remain private.
5. Set the application package's default features to the production macOS feature set. Preserve `--no-default-features` as the hermetic build contract.
6. Resolve any source-package build defects exposed by `cargo package`, including native Opus/CMake behavior, without requiring undocumented environment variables.
7. Update package-name consumers in CI, scripts, release automation, documentation, SBOM generation, and local commands.
8. Update cargo-dist artifacts and installation documentation to the new application package identity. Exact-version v1.3.1 assets remain immutable. The v1.3.2 documentation must reference the actual filenames generated for the new package.

## Publication Flow

Publish from the reviewed v1.3.2 release commit in dependency order:

1. `scrybe-meeting-core`;
2. `scrybe-meeting-capture-mac`;
3. `scrybe-meeting-capture-mic`;
4. `scrybe`.

After each internal package publish, wait until crates.io resolves that exact version before publishing its dependent. A failure leaves already-published internal packages immutable but harmless; fix forward at the same application release only when the failed package version was never accepted. If crates.io accepted defective bytes, increment the patch version because published artifacts cannot be replaced.

After the application package is live, create the matching `v1.3.2` Git tag. The tag-triggered GitHub workflow publishes the prebuilt macOS assets. The crates.io package and GitHub release must derive from the same commit and report the same version.

## Failure Handling

- Abort before publication if any proposed package name becomes occupied by another owner.
- Abort if `cargo package --locked` includes repository secrets, local evidence, generated sessions, or files outside the intended source/license/readme surface.
- Abort a dependent package upload unless every internal dependency at version 1.3.2 is already resolvable from crates.io.
- Abort if the generated package builds only because of workspace `.cargo/config.toml`, cached native libraries, or local patch configuration.
- Never publish the application before all three internal dependencies are resolvable from crates.io.
- Never reuse or move the existing v1.3.1 tag.
- Never claim Linux or Windows recording support; M8 remains parked.

## Verification

Before publication:

1. Run `cargo package --locked --allow-dirty --no-verify` for all four publishable packages so every archive can be inspected before any irreversible upload.
2. Inspect each generated `.crate` file list and normalized `Cargo.toml`.
3. Install the application from its workspace path into isolated `CARGO_HOME`, install root, and target directories to exercise the packaged feature set without cached build artifacts or Homebrew libraries.
4. Run `cargo publish --dry-run` in publication order. The core dry-run runs before any upload. Each dependent dry-run runs only after its exact internal dependencies are visible in the crates.io index, because Cargo verifies the normalized registry dependency graph rather than the local path graph.
5. Run the installed binary:

```sh
scrybe --version
scrybe doctor
scrybe record --help
```

6. Run the repository's Rust formatting, Clippy, check, test, egress, LoC, cargo-dist plan, and release-equivalent macOS build gates.
7. Run the tag workflow and verify every GitHub asset checksum.

After publication:

```sh
INSTALL_ROOT="$(mktemp -d)"
cargo install scrybe --version 1.3.2 --locked --root "$INSTALL_ROOT"
"$INSTALL_ROOT/bin/scrybe" --version
"$INSTALL_ROOT/bin/scrybe" doctor
"$INSTALL_ROOT/bin/scrybe" record --help
```

Acceptance requires the install to resolve only crates.io sources, produce the real application binary, report `scrybe 1.3.2`, expose recording commands, and require no repository clone.

## Documentation

Update:

- `README.md` and `INSTALL.md` with Cargo and GitHub installation paths;
- `CHANGELOG.md` with v1.3.2 package/distribution changes and corrected publish posture;
- `MAINTENANCE.md` and `docs/RELEASE.md` with the four-package publication order and failure recovery;
- release workflow comments, artifact checks, SBOM paths, and cargo-dist examples;
- package-manager templates only where the generated v1.3.2 asset names change;
- local development-plan release tables/history to record the new v1.3.2 distribution repair without resuming M8.

## Rejected Alternatives

### Keep the placeholder and add a launcher wrapper

This requires turning the current CLI into a public library, maintaining an extra launcher package, and preserving duplicate binary surfaces. It adds an abstraction solely to avoid the clean package cutover.

### Publish under `scrybe-app`

This leaves `cargo install scrybe` misleading and wastes the package name already reserved for this project.

### Publish a downloader/bootstrapper

A bootstrap binary would perform hidden network access after `cargo install`, duplicate GitHub installer logic, complicate checksum and self-replacement behavior, and fail the expectation that Cargo built the installed application.
