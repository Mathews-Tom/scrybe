# Audit log

Append-only record of execution-discipline deviations from the maintainer's per-session execution prompt. Each entry names the deviation, the affected artifact on `main`, the reason in-place correction was rejected, and the remediation accepted by the maintainer.

---

## 2026-04-29 — `docs/resolve-foundation-deltas` (PR #4)

**Deviation.** Three commit-message bodies merged via PR #4 contain the word that the maintainer's execution prompt forbids in commit-message bodies, used as a reference to a numbered section of `.docs/development-plan.md`. Subject lines, the branch name (`docs/resolve-foundation-deltas`), the PR title, and the merged document content of `docs/system-design.md` and `docs/dependency-decisions.md` are all clean — the violation is confined to commit-body prose.

**Affected commits on `main`:**

- `3df0dc5` `docs(design): sketch error type hierarchy in system-design §4.6` — body references the v0.1 contract using the forbidden word.
- `f7a0e91` `docs(deps): pin dependency choices for v1.0` — body references the v0.1 contract using the forbidden word.
- `03d3b58` `docs(design): correct error-format strings, atomic-write recipe, platform APIs` — body references the v0.1 contract using the forbidden word.

**Why not corrected in place.** Rewriting commit-message bodies on `main` requires a force-push. The execution prompt classifies force-push to a protected branch as forbidden absent explicit maintainer authorization. The maintainer was offered three options at session end and selected the audit-log marker.

**Remediation accepted.** This file. The original commits stand on `main`; the audit trail is explicit on `main` going forward.

**Future-proofing.** When invoking the execution prompt against a numbered section of `.docs/development-plan.md`, prefer §-anchored references in commit-message bodies (e.g. "§6.2 deliverable item 4", "v0.1 macOS-alpha contract") over deliverable-named references that include the forbidden word. The development plan's section numbering is durable; the deliverable-numbering vocabulary is the violation surface.

---

## 2026-05-01 — `release.yml` triage trail (PRs #24, #25, #26)

**Context.** Not an execution-discipline deviation; preserved here because the v0.1.0 release pipeline hit three distinct latent defects between the first `git push origin v0.1.0` and the final green `release.yml` run, and the triage steps are worth keeping for future maintainers who see one of the same symptoms.

**Tag re-points (each one a delete + re-create at a new SHA):**

- `135ab48` (post `8a21ce2`) — first attempt; failed at `dist build` with `error: profile dist is not defined`.
- `a202bd2` (post `b74440a`, after PR #24 merge) — second attempt; failed at `dist build` with `rustc 1.85.0 is not supported by the following packages: icu_collections@2.2.0 requires rustc 1.86 ...`.
- `7cb3ba1` (post `3c5cf94`, after PR #25 merge) — third attempt; matrix builds passed but `release` job failed at `gh release create dist-staging/*` with `read dist-staging/scrybe-cli-aarch64-apple-darwin: is a directory`.
- `92acd6c` (post `130b88b`, after PR #26 merge) — fourth attempt; **green end-to-end**.

The crates.io publish (`scrybe v0.1.0`) was unaffected by every release.yml failure — it succeeded before the first dispatch and stayed live throughout.

**Round 1 — `[profile.dist]` missing (PR #24).**

`cargo-dist` 0.25.x runs `cargo build --profile=dist` for tarball production. Without an explicit `[profile.dist]` block in workspace `Cargo.toml`, the build aborts. Fix: 2-line addition (`inherits = "release"`).

Why this leaked through PR-time CI: `ci.yml`'s `dist-plan` job only runs `dist plan`, which validates `[workspace.metadata.dist]` config but never compiles. `dist build` only runs in `release.yml` on tag push.

**Round 2 — rustc 1.85 vs `icu_*` 2.2 MSRV (PR #25).**

Dependency chain: `scrybe-core` (with `hook-git` feature) → `git2 0.19` → `url 2.5.8` → `idna 1.x` → `idna_adapter 1.2.2` → `icu_normalizer 2.2.0` → `icu_collections 2.2.0`. Each `icu_* 2.2.0` declares `rust-version = "1.86"`; the workspace pins 1.85.0 in `rust-toolchain.toml`.

Fix: `cargo update -p url --precise 2.5.0` rolls back to the last `url` release before the `idna 1.x` migration (which was the entry point for the entire `icu_*` cluster). Reintroduces `RUSTSEC-2024-0421` (idna 0.5 Punycode validation); ignored in both `audit.toml` and `deny.toml` with documented rationale (Hook::Git's URL surface — local-repo paths and conventional git remotes — does not exercise the affected Punycode path).

Why this leaked through PR-time CI: `ci.yml`'s `build` jobs run `cargo check`, which is more permissive about MSRV mismatches in transitive deps than `cargo build`. `dist build` compiles every transitive crate, hitting the `rust-version` declaration head-on.

**Round 3 — `dist-staging/*` matched a directory (PR #26).**

`cargo-dist build` leaves the unpacked `scrybe-cli-<target>/` working directory next to the produced `.tar.xz` in `target/distrib/`. The build job's `actions/upload-artifact` `path: target/distrib/*` uploads everything; `actions/download-artifact --merge-multiple` flattens it into the release job's `dist-staging/`; the script's `gh release create dist-staging/*` then expanded to include the directory; `gh release create` rejects directory args with "is a directory".

Fix: `mapfile -t ASSETS < <(find dist-staging -maxdepth 1 -type f)` filters to regular files before passing to `gh release create`. Logged via `printf` so the next failure surfaces the file list considered.

A partial GitHub Release was created at this point (release ID 316214887, notes generated, no assets attached) before the script errored. Cleaned up via `gh release delete v0.1.0 --yes` (kept the tag).

**Future hardening (not done in this triage; tracked here).**

Add a `dist-build-host` lane to `ci.yml` that runs `cargo build --profile=dist -p scrybe-cli --features cli-shell,hook-git --no-default-features` on `macos-14`. That single command catches all three failure modes (missing profile, MSRV-via-transitive-dep, build success) at PR time, before a tag is pushed and the cargo-dist matrix is invoked. The runtime cost is one additional macos-14 build per PR (~1 minute incremental); the value is "no more multi-round release-pipeline hotfix sequences after a release tag is in flight". Out of scope for v0.1.0; lands as a `chore/ci-dist-build-host` slice in v0.2 prep.

---

## 2026-05-01 — "Claude Code" substring in commit body (PR #22, commit `37410fc`)

**Deviation.** Self-review at session end caught the substring `Claude Code` in the body of `37410fc fix(docs): soften TCC inheritance claim to match observed evidence`. The full sentence reads: "if macOS displayed a prompt and they accepted it, the dispatch would still pass and Claude Code wouldn't have observed the click."

**Letter vs. spirit of the policy.** The maintainer's `~/.claude/CLAUDE.md` lists specific forbidden phrases for AI attribution: `Generated with Claude`, `Co-Authored-By: Claude`, `🤖`, `AI-assisted`, `via Claude Code`, "any coauthor tag pointing at an AI". None of those literal phrases appear in the commit body — the usage is **narrative** ("Claude Code wouldn't have observed") referring to the executor as a third-party observer in the prose, not **attributive** ("Generated with Claude Code"). Letter-of-the-rule: clean. Spirit-of-the-rule: borderline — the project's sweep grep (`grep -iE "phase[ -]?[0-9]|🤖|claude|ai-assisted|co-authored.*ai"`) is intentionally broader than the literal forbidden-phrase list and surfaces the substring.

**Affected commit on `main`:**

- `37410fc` `fix(docs): soften TCC inheritance claim to match observed evidence` — body uses "Claude Code" once as the subject of a hypothetical clause about prompt observation.

**Why not corrected in place.** Rewriting commit-message bodies on `main` requires a force-push. The execution prompt classifies force-push to a protected branch as forbidden absent explicit maintainer authorization. Same precedent as the 2026-04-29 entry above; the maintainer chose the audit-log marker over force-pushing the body.

**Remediation accepted.** This entry. The original commit stands on `main`.

**Future-proofing.** When narrating session history in commit-message bodies, prefer impersonal phrasing ("the executor", "the build", "the workflow") over named references to the runtime. The narrative information is preserved; the substring sweep stays clean.
