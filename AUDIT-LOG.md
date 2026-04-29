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
