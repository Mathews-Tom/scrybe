# Maintenance commitments

> Public commitments the scrybe maintainer makes to downstream users at v1.0.0.
> The document is intentionally narrow — only items the maintainer plans to honour are listed here.

scrybe shipped v1.0.0 on 2026-05-02. This document records the maintenance posture for the v1.0 series so downstream users — distribution maintainers, integrators, audit teams, contributors — have a single place to read what they can rely on.

The authoritative architectural contract is `docs/system-design.md` §12 (versioning and stability). This file translates §12 into operational commitments. Where the two appear to disagree, the design doc wins and this document is the bug.

---

## 1. Six-month scope freeze

From the v1.0.0 release date (2026-05-02), for at least six months (through 2026-11-02), the project commits to **no scope expansion** beyond the surface enumerated in `docs/system-design.md` §12.

What that means in practice:

- No new platform adapters. The four `AudioCapture` implementations (`scrybe-capture-{mac,linux,win,android}`) are the v1.0 set; iOS, BSD, ChromeOS, embedded targets are out of scope for the freeze window.
- No new extension seams. The five traits (`AudioCapture`, `ContextProvider`, `SttProvider`, `LlmProvider`, `Hook`, `Diarizer`) are the v1.0 set; a sixth seam needs a tracked issue with two real downstream call sites and a minor-version cycle to land.
- No new top-level CLI subcommands. The Tier-2 set in `system-design.md` §12.2 (`init`, `record`, `list`, `show`, `doctor`, `bench`) is what v1.0 maintains.
- No new optional feature flags on `scrybe-core`. The set committed at v1.0 (`hook-git`, `whisper-local`, `parakeet-local`, `openai-compat`, `context-ics`, `hook-webhook`, `hook-tantivy`, `diarize-pyannote`) is what v1.0 supports.

What is **in** scope during the freeze:

- Bug fixes. Every bug report is triaged; severe bugs cut a patch release.
- Security advisories. CVE-bearing dependencies get patched out under the SLA in §3 below.
- Live-binding work behind already-shipped feature flags. The `core-audio-tap`, `media-projection`, `wasapi-loopback`, `parakeet-local`, and `diarize-pyannote` features all carry follow-up work to land their real native bindings; that work continues without re-opening the scope question.
- Documentation, examples, README polish.
- Reproducibility hardening (`reproducibility.yml`) and supply-chain hardening (`cargo-vet`) — both shipped advisory at v0.9.0-rc1 and remain advisory at v1.0.0; promoting either to a blocking gate is a v1.0.x → v1.1 deliverable.
- Downstream package-manager submissions (Homebrew tap, Scoop bucket, AUR, Flathub, F-Droid). The in-tree templates at `packaging/` are ready; the submissions themselves are maintainer actions and may land at any v1.0.x patch release.

If a feature request arrives during the freeze, the response is "thank you, deferred to v1.1 — feel free to comment on the issue with a use case." Holding the scope is a feature.

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
- Feature requests: triaged but not necessarily acted on during the freeze. The expected response is "thanks, queued for v1.1 consideration" unless the request maps to an existing follow-up tracked in `docs/system-design.md` open questions or `.docs/development-plan.md` §17.

The maintainer is a single person on evenings; the SLA is best-effort, not contractual. If the SLA slips, the only recourse is to fork — Apache-2.0 makes that an honest option, not a threat.

---

## 4. Release cadence

Per `docs/system-design.md` §12.5, scrybe targets a **time-boxed minor release every 6 weeks**. Predictability beats feature completeness for an OSS project — releases go out on schedule with whatever shipped, not "when ready". Patch releases (`1.0.x`) cut on demand for bug fixes and security advisories.

The next planned milestones:

- `v1.0.x` patch stream — bug fixes and security advisories as needed.
- `v1.1.0` — first minor after the freeze; targeted for ~2026-11-02 if the cadence holds. Scope decided in the v1.0 retrospective, not this document.

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
- Tier-1 changes are non-starters during the v1.0 series — see §1 and §2. A PR that touches a Tier-1 type closes with a pointer to this document.
- Contributors retain copyright on their contributions. The DCO sign-off on every commit (`Signed-off-by: Name <email>`) is the licensing record. There is no CLA.

---

## 7. The "if I disappear" plan

scrybe is a solo-maintainer project. The bus factor is one. Two structural mitigations:

1. **Apache-2.0 license.** §3 of the license grants a perpetual patent licence; §4 enforces attribution and modification notices on derivative works; §6 protects the project name. A fork can keep the project alive without the maintainer's continued involvement.
2. **Self-contained architecture.** The four traits + filesystem-as-database design means a fork can replace the maintainer's chosen providers (whisper-rs, ollama, sherpa-rs) with their own without touching `scrybe-core`. The architecture is the artifact; the maintainer is replaceable.

If the maintainer goes silent for >90 days without a public note, downstream users should expect to fork. That's the design.
