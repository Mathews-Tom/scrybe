# Release runbook

Step-by-step procedure for publishing a scrybe release. Each step lists its exact command, its verification, and its rollback (where one exists). Read the entire document before starting.

The runbook assumes Option B from `CHANGELOG.md` for v0.1.0: only the `scrybe` crate publishes to crates.io; `scrybe-core`, `scrybe-capture-mac`, and `scrybe-cli` stay workspace-private (`publish = false`). Binary distribution is via cargo-dist tarballs published as GitHub Release assets, triggered automatically by the version tag push.

## v0.1.0 — first non-placeholder release

### Pre-flight (do these first; cheap and reversible)

#### 1. Confirm working tree is clean and on `main`

```bash
cd /Users/druk/WorkSpace/AetherForge/scrybe
git checkout main
git pull --ff-only origin main
git status                              # must be clean
git log --oneline -5                    # confirm latest commits match main on origin
```

If output shows pending changes, stop. Investigate before proceeding.

#### 2. Confirm CI on main is green

```bash
gh run list --branch main --limit 5 --json status,conclusion,workflowName
```

Every recent `ci` run must show `conclusion: success`. The last `nightly-e2e` run (if any) must also be `success` — if it failed, triage Tier-3 before publishing.

#### 3. Confirm crates.io credentials are configured

```bash
ls ~/.cargo/credentials.toml            # must exist
```

If missing, generate a new token at <https://crates.io/me> (scope: publish-update) and run `cargo login <token>`. The token will be persisted to `~/.cargo/credentials.toml` with `0600` permissions.

#### 4. Verify the version bump landed in main

```bash
grep -rn 'version = "0.1.0"' scrybe/Cargo.toml scrybe-core/Cargo.toml \
    scrybe-capture-mac/Cargo.toml scrybe-cli/Cargo.toml | head
```

Must show `0.1.0` in all four. If any still show `0.1.0-alpha.1`, the version-bump PR (which this runbook is shipped under) was not fully merged.

### Publish (the irreversible bit)

#### 5. Dry-run one more time

```bash
cargo publish --dry-run -p scrybe
```

Expected output ends with:

```text
Packaging scrybe v0.1.0 (.../scrybe)
Packaged 6 files, 5.7KiB (2.5KiB compressed)
Verifying scrybe v0.1.0 (.../scrybe)
Compiling scrybe v0.1.0 (.../target/package/scrybe-0.1.0)
Finished `dev` profile [unoptimized + debuginfo] target(s) in <N>s
Uploading scrybe v0.1.0 (.../scrybe)
warning: aborting upload due to dry run
```

If `--dry-run` errors with "version 0.1.0 already exists", someone has already published — stop and investigate before re-running. If it errors on packaging or compile, fix the underlying issue and re-run from step 1.

#### 6. Publish the `scrybe` crate to crates.io

```bash
cargo publish -p scrybe
```

This is **irreversible** at the version-number level. Once `cargo publish` returns success, the version `0.1.0` of `scrybe` is permanently consumed on crates.io. You can yank but not delete (see Rollback below).

Expected: same output as the dry-run minus the abort line. The final line is `Uploaded scrybe v0.1.0`.

#### 7. Verify the publish

```bash
sleep 30                                # crates.io index needs a moment to propagate
cargo search scrybe --limit 1
```

Expected:

```text
scrybe = "0.1.0"        # Open-source local-first meeting transcription...
```

Or check the page directly:

```bash
open https://crates.io/crates/scrybe
```

Confirm `0.1.0` is listed as the latest version.

#### 8. Tag the release

```bash
git tag -a v0.1.0 -m "v0.1.0: macOS-alpha first non-placeholder release"
git push origin v0.1.0
```

The tag push triggers `.github/workflows/release.yml` automatically, which runs `cargo-dist build` for `aarch64-apple-darwin` and `x86_64-apple-darwin`, generates the shell installer, computes `SHA256SUMS.txt`, and creates a GitHub Release at `https://github.com/Mathews-Tom/scrybe/releases/tag/v0.1.0` carrying every artifact.

#### 9. Watch the release workflow

`gh run watch` accepts a run ID positionally; resolve the latest run from the `release` workflow and pipe it in:

```bash
gh run list --workflow=release.yml --limit 1 --json databaseId --jq '.[0].databaseId' | xargs gh run watch
```

Or open the run in a browser:

```bash
gh run list --workflow=release.yml --limit 1 --json url --jq '.[0].url'
```

Wait for `conclusion: success`. Job durations: `plan` ~30s, `build` (per target) ~90s, `release` ~60s. Total wall time ~5 minutes.

#### 10. Verify the GitHub Release

```bash
gh release view v0.1.0
```

Expected assets:

- `scrybe-cli-aarch64-apple-darwin.tar.xz`
- `scrybe-cli-x86_64-apple-darwin.tar.xz`
- `scrybe-cli-installer.sh`
- `dist-manifest.json`
- `SHA256SUMS.txt`

Each tarball must contain `scrybe`, `INSTALL.md`, `LICENSE`, and `README.md` — verify by extracting one:

```bash
mkdir -p /tmp/scrybe-release-check && cd /tmp/scrybe-release-check
gh release download v0.1.0 --pattern '*aarch64*'
tar xf scrybe-cli-aarch64-apple-darwin.tar.xz
ls scrybe-cli-aarch64-apple-darwin/      # scrybe, INSTALL.md, LICENSE, README.md
./scrybe-cli-aarch64-apple-darwin/scrybe --version    # 0.1.0
```

#### 11. Smoke-test the shell installer

On a clean shell:

```bash
curl --proto '=https' --tlsv1.2 -sSf https://github.com/Mathews-Tom/scrybe/releases/download/v0.1.0/scrybe-cli-installer.sh | sh
```

Expected: installer fetches the matching tarball, verifies SHA256, extracts to `~/.local/bin/` (or `$CARGO_HOME/bin` if cargo-dist's default placement applies), and prints a success message.

```bash
which scrybe                            # path to the just-installed binary
scrybe --version                        # 0.1.0
xattr -dr com.apple.quarantine "$(which scrybe)"   # required on macOS for the unsigned binary
```

#### 12. Update the dev-plan checkboxes

`.docs/development-plan.md` §1.1 status snapshot, §4 phase overview, and §7.4 exit criteria all reference v0.1.0 as a target. The maintainer manually updates each to reflect the released state. This is a doc-only commit on a follow-up PR; not part of the release runbook itself.

### Rollback

#### Yank a published version

If a critical bug is discovered post-publish:

```bash
cargo yank --version 0.1.0 -p scrybe --reason "describe the bug here"
```

`yank` does **not** delete the version. Existing dependents that have `Cargo.lock` pinned to `0.1.0` continue to download it. New resolutions that don't already pin will skip the yanked version. You cannot re-use the version number.

To unyank (e.g., after confirming the bug is downstream-only):

```bash
cargo yank --version 0.1.0 --undo -p scrybe
```

#### Delete the GitHub Release

```bash
gh release delete v0.1.0 --yes --cleanup-tag
```

`--cleanup-tag` removes both the GitHub Release entry and the underlying `v0.1.0` tag. Note: only the release UI and tag are affected. The crates.io publish is not undone by this command.

#### Recover from a failed `release.yml` run

If `release.yml` fails (e.g., one of the macOS targets fails to build mid-run), the GitHub Release will not exist or will be partial. Diagnose via `gh run view <run-id> --log-failed`. Fix the underlying issue, push the fix to main, then either:

- Re-run the failed jobs: `gh run rerun <run-id> --failed`
- Or delete the tag and re-push: `git push --delete origin v0.1.0 && git tag -d v0.1.0 && git tag v0.1.0 <new-sha> && git push origin v0.1.0`

The `cargo publish` to crates.io is independent of the release workflow — that step is already irreversible regardless of what happens to the GitHub Release.

## Future releases (v0.2.0 and beyond)

When the dev plan reaches §8 (Phase 2) and beyond, the publish set may grow:

- v0.2.0 introduces `OpenAiCompatSttProvider` and `OpenAiCompatLlmProvider`. These are useful as a building block for downstream consumers, so flipping `publish = true` on `scrybe-core` becomes a real proposal at that point.
- v0.3.0 introduces the Linux capture adapter (`scrybe-capture-linux`). Same publish question.
- v1.0.0 freezes Tier-1 traits per `docs/system-design.md` §12. After v1.0, every workspace crate is a candidate for crates.io.

When the publish set changes, this runbook gains additional steps. Order them topologically — every dependent's publish must wait for its dependencies to be live on the crates.io index, because publish-time verification resolves `path + version` deps against the index, not against the local workspace.

Workspace dependency graph:

- `scrybe-core` — no scrybe-* deps
- `scrybe` — no scrybe-* deps (placeholder)
- `scrybe-capture-mac` — depends on `scrybe-core`
- `scrybe-cli` — depends on `scrybe`, `scrybe-core`, `scrybe-capture-mac`

A valid publish order:

```bash
# multi-crate publish in dependency order with index-propagation sleeps
cargo publish -p scrybe-core
sleep 60                                # let scrybe-core land in the index
cargo publish -p scrybe                 # independent of scrybe-core, but must precede scrybe-cli
sleep 60
cargo publish -p scrybe-capture-mac     # needs scrybe-core in the index
sleep 60
cargo publish -p scrybe-cli             # needs all three above in the index
```

The 60-second sleep matters: dependents publish-time-resolve their `path + version` deps against the live crates.io index, not against the local workspace, so the index has to have the new version visible before the dependent's publish job verifies. Publishing `scrybe-cli` before `scrybe` (or before `scrybe-capture-mac`) would fail at the verify step because the dep's new version wouldn't be on crates.io yet.
