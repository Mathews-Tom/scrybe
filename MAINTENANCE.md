# Maintenance commitments

> Public commitments the scrybe maintainer makes to downstream users at v1.0.0.
> The document is intentionally narrow — only items the maintainer plans to honour are listed here.

scrybe shipped v1.0.0 on 2026-05-02. This document records the maintenance posture for the v1.0 series so downstream users — distribution maintainers, integrators, audit teams, contributors — have a single place to read what they can rely on.

The authoritative architectural contract is `docs/system-design.md` §12 (versioning and stability). This file translates §12 into operational commitments. Where the two appear to disagree, the design doc wins and this document is the bug.

---

## 1. Post-v1.0 scope policy

At v1.0.0 (2026-05-02) the project announced a six-month scope freeze running through 2026-11-02: no new platform adapters, extension seams, top-level CLI subcommands, or `scrybe-core` feature flags until a "v1.0 retrospective" decided v1.1's scope (see §4, historical text below).

That retrospective happened early. On 2026-09-03 the maintainer authored `.docs/DEVELOPMENT_PLAN.md`, a concrete deliverable-by-deliverable post-v1.0 roadmap (nine releases, `v1.1.0-rc1` through `v1.4.1`) that requires new CLI subcommands and feature flags well before the original November date — the first of those deliverables already shipped `scrybe rec` as `v1.1.0-rc1`.

**The six-month freeze is retired, effective 2026-09-03.** `.docs/DEVELOPMENT_PLAN.md` — not a fixed calendar date — is the live scope authority for what ships next. Each deliverable's own pre-implementation design gate is the scope-review mechanism: it revalidates that deliverable against current code, prior merges, and this document before any product code is written. The Tier-1 architectural freeze in §2 below (and `docs/system-design.md` §12.1) is unrelated and remains fully in force — it protects trait/schema shapes until a major-version bump, not a calendar window, and nothing here changes it.

### Historical: the original six-month freeze text (2026-05-02 – 2026-09-03, retired)

What the freeze meant in practice while it held:

- No new platform adapters. The four `AudioCapture` implementations (`scrybe-capture-{mac,linux,win,android}`) were the v1.0 set; iOS, BSD, ChromeOS, embedded targets were out of scope for the freeze window.
- No new extension seams. The five traits (`AudioCapture`, `ContextProvider`, `SttProvider`, `LlmProvider`, `Hook`, `Diarizer`) were the v1.0 set; a sixth seam needed a tracked issue with two real downstream call sites and a minor-version cycle to land.
- No new top-level CLI subcommands. The Tier-2 set in `system-design.md` §12.2 (`init`, `record`, `list`, `show`, `doctor`, `bench`) was what v1.0 maintained.
- No new optional feature flags on `scrybe-core`. The set committed at v1.0 (`hook-git`, `whisper-local`, `parakeet-local`, `openai-compat`, `context-ics`, `hook-webhook`, `hook-tantivy`, `diarize-pyannote`, `encoder-opus`) was what v1.0 supported.

What was **in** scope during the freeze — this stays true now, unchanged:

- Bug fixes. Every bug report is triaged; severe bugs cut a patch release.
- Security advisories. CVE-bearing dependencies get patched out under the SLA in §3 below.
- Live-binding work behind already-shipped feature flags — this class of work continues without re-opening the scope question, same as before.
- Documentation, examples, README polish.
- Reproducibility hardening (`reproducibility.yml`) and supply-chain hardening (`cargo-vet`).
- Downstream package-manager submissions (Homebrew tap, Scoop bucket, AUR, Flathub, F-Droid).

---

## 2. Stability tiers

Per `docs/system-design.md` §12, the v1.0 surface splits into three tiers with different stability promises.

### Tier 1 — frozen at v1.0

These items do not change without a v2.0 major bump and a six-month deprecation window where the deprecated shape coexists with the new one and a `LifecycleEvent::SchemaDeprecated` warning emits on every load:

- `AudioCapture` trait + `AudioFrame` + `FrameSource` + `Capabilities` + `PermissionModel`.
- `MeetingContext` field set (additive new fields with `#[serde(default)]` are non-breaking).
- `LifecycleEvent` variant set.
- `ConsentAttestation` schema and the `[consent]` table key set in `meta.toml`.
- `meta.toml` on-disk schema v1.
- Storage-layout invariants: ULID-suffixed folder name, per-session `pid.lock`, append-only `transcript.md` + `audio.opus`, atomic-replace `meta.toml` + `notes.md`.
- Apache-2.0 license. Re-licensing requires a major-version event and unanimous contributor consent.

### Tier 2 — stable, may evolve in minor releases

Breaking changes here are permitted in minor releases and **must** appear in `CHANGELOG.md` under a `### Breaking` heading. Affected releases bump the second SemVer component (`1.0.0` → `1.1.0`). The Tier-2 surface is enumerated in `docs/system-design.md` §12.2 — provider traits, the `Diarizer` trait, the `Hook` trait, CLI subcommands, the `config.toml` block schema, the `notes.md` template variables, the bench snapshot format, and the multilingual manifest schema.

### Tier 3 — internal, no commitment

Anything not listed in §12.1 / §12.2 is implicitly Tier 3 and changes between releases without a CHANGELOG entry. Promotion to Tier 2 follows the procedure in `docs/system-design.md` §12.4: tracking issue with two real downstream call sites, freeze the shape behind the existing surface in a minor release, add the row to §12.2.

---

## 3. Issue triage and security disclosure

Triage SLA at v1.0:

- Bugs (any severity): first response within 7 days of report. Reproducible bugs get a tracking issue and a target release; non-reproducible bugs are closed with a "needs more info" template.
- Security disclosures: first response within 72 hours. Use private security advisories on GitHub (`Security` tab → `Report a vulnerability`); do not post in public issues. The maintainer aims for a fix-or-mitigate within 7 days for High/Critical and 30 days for Medium.
- Feature requests: triaged against `.docs/DEVELOPMENT_PLAN.md`'s deliverable sequence. The expected response is "thanks, tracked for an upcoming release" when it maps to a planned deliverable, or "queued for a future release" otherwise.

The maintainer is a single person on evenings; the SLA is best-effort, not contractual. If the SLA slips, the only recourse is to fork — Apache-2.0 makes that an honest option, not a threat.

---

## 4. Release cadence

Per `docs/system-design.md` §12.5, scrybe targets a **time-boxed minor release every 6 weeks**. Predictability beats feature completeness for an OSS project — releases go out on schedule with whatever shipped, not "when ready". Patch releases (`1.0.x`) cut on demand for bug fixes and security advisories.

The active release train — supersedes the "first minor after the freeze" framing this section carried through 2026-09-03 — is `.docs/DEVELOPMENT_PLAN.md` §4: nine deliverables, each cutting its own minor/patch release, `v1.1.0-rc1` through `v1.4.1`. `v1.1.0-rc1` shipped 2026-09-03.

---

## 5. Distribution and trust posture

The publish posture from v0.1.0 carries forward unchanged at v1.0:

- Only the `scrybe` placeholder crate publishes to crates.io. `scrybe-core`, `scrybe-cli`, and the four capture adapters keep `publish = false`. Downstream users install the binary via the cargo-dist tarballs (Homebrew, Scoop, AUR, Flathub, F-Droid as those submissions land), via the `curl | sh` installer one-liner, or via `cargo install --git https://github.com/Mathews-Tom/scrybe scrybe-cli --tag v1.0.0 --features cli-shell,hook-git` for the audit-friendly path.
- Native code-signing on macOS (Apple Developer ID + notarization) and Windows (Authenticode certificate) remains explicitly out of scope through v1.x. Users handle Gatekeeper's "Apple cannot verify" prompt and Windows SmartScreen's "Run anyway" path manually per `INSTALL.md`. The cosign keyless OIDC signature over `SHA256SUMS.txt` is the cryptographic anchor for distribution trust — it proves the artifact came from the GHA workflow on the tagged commit, without paying the vendor-CA tax.
- Reproducible builds verified via `.github/workflows/reproducibility.yml`. The lane runs in advisory mode at v1.0.0 — the macOS-14 cargo-dist tarballs are not yet bit-identical across runner instances. Promotion to a blocking gate is a v1.0.x → v1.1 deliverable. The lane uploads both legs' artifacts on every run so a contributor can run `diffoscope` between them and localise the residual non-determinism.
- Supply-chain provenance via `cargo-vet`. The wiring lands at v0.9.0-rc1 / v1.0.0; the direct-dep audit work is incremental and the lane stays advisory until the maintainer commits the first batch of `audits.toml` entries.

---

## 6. Contributor expectations

Contributions land via pull request with the conventional-commit format documented in `.github/PULL_REQUEST_TEMPLATE.md` (when present) or in `~/.claude/rules/commit-standards.md` (the maintainer's local convention). Concrete expectations:

- Every PR must keep the workspace CI green: `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test --workspace`, `cargo audit`, `cargo deny check`, the coverage gate, the LoC-budget gate, and the egress-audit gate.
- New code carries unit tests at the 90% line-coverage threshold for `scrybe-core` and 80% for the workspace. Critical paths (capture, atomic writes, retry policy, consent attestation, config validation) target 95%.
- Tier-1 changes are non-starters — see §2. The scope freeze in §1 that used to pair with this line is retired; Tier-1 (architectural trait/schema shapes) is the remaining hard boundary. A PR that touches a Tier-1 type closes with a pointer to this document.
- Contributors retain copyright on their contributions. There is no CLA. A `CONTRIBUTING.md` documenting the licensing record (whether DCO sign-off, an explicit Apache-2.0 §5 contribution clause, or another mechanism) is a v1.0.x deliverable; until it lands, contributions are accepted under the repository's existing Apache-2.0 license per Apache-2.0 §5 ("each Contributor hereby grants ... a perpetual, worldwide, non-exclusive ... license").

---

## 7. The "if I disappear" plan

scrybe is a solo-maintainer project. The bus factor is one. Two structural mitigations:

1. **Apache-2.0 license.** §3 of the license grants a perpetual patent licence; §4 enforces attribution and modification notices on derivative works; §6 protects the project name. A fork can keep the project alive without the maintainer's continued involvement.
2. **Self-contained architecture.** The four traits + filesystem-as-database design means a fork can replace the maintainer's chosen providers (whisper-rs, ollama, sherpa-rs) with their own without touching `scrybe-core`. The architecture is the artifact; the maintainer is replaceable.

If the maintainer goes silent for >90 days without a public note, downstream users should expect to fork. That's the design.
