# scrybe — Pitch

> **Meeting notes that never leave your laptop.**
> An open-source meeting notetaker. Capture your own meetings, get a transcript and AI-generated notes — all on your own machine. No cloud, no account.

---

## The thing I'm building

A single Rust binary you install on your laptop. Press a hotkey when a meeting starts. It captures the meeting audio and your microphone, transcribes locally with whisper.cpp (or via any OpenAI-compatible API you point it at), and produces a structured markdown summary at the end. Files land in `~/scrybe/<date>-<title>/`. Everything stays on your machine.

macOS, Windows, Linux, and Android. iOS is excluded because Apple's sandbox makes it impossible.

## Why I think this is worth building

The meeting-notetaker market splits into three camps:

| Camp | Examples | Problem |
|---|---|---|
| Bot-based SaaS | Otter, Fireflies, Read.ai, Fathom | A bot visibly joins the call. Awkward, often blocked by client IT. Vendor cloud by definition. |
| Audio-capture SaaS | Granola, tldv | Local capture but uploads to a vendor cloud. Requires account. Mac-only mostly. Granola does post a courtesy notice into the meeting chat — the only competitor that does. |
| Open-source notetakers | Meetily, fastrepl/anarlog (formerly Hyprnote) | Local-first in claim, architecturally sprawling in reality. Meetily (~11.4k stars) ships SQLite + Tauri + Next.js frontend with ~44 MB of workspace; anarlog (~8.3k stars, active 2026-04, mac-only) is ~632 MB of workspace. Neither covers Linux or Android. Neither offers a courtesy-notification step. |

The user who falls through every gap:

- **Privacy-conscious professionals.** Doctors taking dictation post-consult, lawyers reviewing their own meetings, finance and product folks whose meetings shouldn't leave their machine.
- **Independent consultants.** Don't want a visible bot joining client calls.
- **OSS maintainers and developers who prefer local-first software.** Want filesystem storage so they can `grep`, sync via Syncthing, and audit the binary themselves.
- **Multi-language professionals outside the US/EU.** Underserved by English-centric SaaS; hit latency to the nearest cloud region.
- **Anyone whose org's procurement process makes a SaaS notetaker a non-starter.**

These users are all currently taking notes by hand, or not at all. Every one of them I've described to has reacted with some version of "yes, I'd use that."

## Why now specifically

Three things that weren't true two years ago:

1. **Native OS APIs solve system-audio capture without a virtual driver on every platform we care about.** macOS ScreenCaptureKit (since macOS 13), Windows WASAPI per-process loopback (since Win10 build 2004), PipeWire on Linux, Android's `AudioPlaybackCapture` (since Android 10). The "you need to install BlackHole or VB-Cable" era is over.
2. **Local Whisper is genuinely good now.** Whisper-large-v3 on M-series Apple Silicon hits ~5x realtime with Metal. For most languages, it's usable for production note-taking. Parakeet is even faster.
3. **The OSS shelf is crowded but skewed.** Two Rust meeting-notetakers matter: Meetily (Tauri + SQLite, mac+win) and `fastrepl/anarlog` (formerly Hyprnote, mac-only, recently active). Both ship local-first capture, but neither runs on Linux or Android, and neither offers a courtesy-notification step that posts into the meeting chat at start. The unoccupied lane is **cross-platform (mac+win+linux+android) + filesystem-only + courtesy-notification by default**, not "the only OSS option."

## What makes this different from the existing OSS options

I cloned char/anarlog and Meetily before designing this. The honest comparison:

| Dimension | fastrepl/anarlog | Meetily | scrybe (target) |
|---|---|---|---|
| Workspace size | ~632 MB | ~44 MB | ≤ 30 MB |
| Realistic Rust LoC at v1.0 | 100k+ | 40k+ (mixed Rust/TS) | 15–20k |
| Leanness metric | n/a | n/a | **`scrybe-core` ≤ 4k LoC; core/(sum of capture adapters) ratio ≤ 0.6** |
| `*_v2` / `*_old` cruft | Yes (heavy) | Yes (`audio` + `audio_v2`, `lib_old_complex.rs`) | None — replace, don't accrete |
| Storage | TinyBase + libsql + markdown | SQLite + audio files | Pure filesystem |
| Platforms | macOS only | macOS, Windows | macOS, Windows, Linux, Android |
| In-meeting courtesy notification | None | None | **Mandatory pre-start step, configurable but not removable** |
| Maintenance status | Active mac-only (last push 2026-04-27) | Active, monetizing | Will be active or fail visibly |

**This is not a "build a better OSS notetaker" pitch. It's a "the OSS notetaker shelf has the wrong stuff on it" pitch.** Cross-platform Linux+Android + filesystem-only + lean architecture is a niche nobody currently occupies.

## The architecture, in three bullets

- **One Rust core**, four cargo-feature-gated platform capture adapters behind a single `AudioCapture` trait. The core has zero `cfg(target_os)` outside that trait.
- **Filesystem as the only data model.** No SQLite, no embedded DB, no schema migrations. Markdown on disk. Sync via whatever you already use (Syncthing, iCloud, Obsidian vault, git).
- **Four extension seams**: capture, context (calendar/attendees), STT/LLM provider, lifecycle hooks. Adding calendar integration is implementing one trait. Adding a new cloud provider is config, not code.

Detailed system design lives in `system-design.md`.

## What I'm explicitly not building

To stay honest about scope, and to make the "no, that's out" answers easier later:

- No accounts, no sync server, no web app. Ever.
- No plugin runtime / dynamic loading. Compiled-in Rust hooks only.
- No streaming "watch the words appear" UX. Batch-windowed (30s chunks).
- No iOS. Apple's sandbox blocks the necessary APIs.
- No Slack/Teams/Notion native integrations. A `Hook` POSTing notes to a webhook is enough.
- No "Pro tier", no hosted backend, no telemetry. The architectural discipline is the product.

If at 24 months scrybe has any of those, it has failed at its founding constraint.

## Risks I see

I'd rather hear "you're underestimating X" now than at v0.3.

| Risk | What I think today |
|---|---|
| macOS Screen Recording permission UX kills first-run conversion | Real on macOS 13.0–14.3. **Mitigation: use Core Audio Taps (macOS 14.4+) for audio-only capture — avoids the screen-recording permission and the orange dot.** ScreenCaptureKit is the fallback for older macOS. |
| Local Whisper accuracy below user expectations for non-English | Real. Default to `large-v3-turbo` (~800 MB resident) not `tiny`; document `large-v3` as opt-in for max accuracy on ≥ 32 GB systems; document cloud STT as the path for hard cases. |
| Local LLM summary quality below GPT-4 expectations | Real. Default messaging: "local works; cloud is better; here's how to BYO." Recommend cloud LLM on ≤ 16 GB systems. |
| Meetily / anarlog ship scrybe's exact feature set first | Possible. anarlog is mac-only, Meetily is mac+win — neither will ship Linux + Android system-audio capture without significant rearchitecture. Differentiation is platform breadth + consent-by-default + filesystem-only, not first-mover. |
| Solo-maintainer bus factor | Real. Apache-2.0 license — explicit patent grant under §3, attribution-preserving §4, no copyleft — so forks survive me without forcing closed-source-SaaS-only outcomes. |
| Apple/Google deprecate the underlying capture APIs | Same risk every competitor carries. Track upstream; vendor `screencapturekit-rs` and `coreaudio-tap-rs` (the latter to be written and contributed upstream). |
| Author legal posture for publishing an open-source notetaker | Materially low. See `docs/LEGAL.md` for the publisher-posture summary. Mitigations: courtesy-notification UX as default, neutral marketing, no managed service. |

## What I want from you

This document is me testing the water before writing the code. Specifically:

1. **Is the user persona real to you?** Either as you, or as someone you know within one degree. If nobody in your circle is a "yes, I'd use that," the persona is wrong.
2. **What am I missing?** What's the obvious failure mode I'm not naming? What's the existing tool I should have compared against and didn't?
3. **Is there an integration that would make this immediately useful for you?** Calendar source, hook target, output format. I'd rather hear three concrete asks than a vague "would be nice if it had X."
4. **Would you star, contribute, or sponsor?** I'm not asking for money — there's no funding round. I'm asking what level of engagement is honest. "I'd star and check back" is a real answer; "I'd run it on Friday for our team standup" is a different real answer.

A response of "interesting but not for me" is also a useful answer. The wrong answer is silence.

## What ships first

The plan is staged honestly:

| Version | Target | Estimated weeks of evening work |
|---|---|---|
| **v0.1.0-alpha** | Reserve `scrybe` on crates.io. Stub crate, README, repo set up. | < 1 |
| **Phase 0** | Resolve critical findings, freeze trait shapes, write `LEGAL.md`. Doc-only, no release. | 2 |
| **v0.1** | macOS only (Core Audio Taps primary, ScreenCaptureKit fallback). CLI: `scrybe init / record / stop / list / show / doctor`. Local whisper + Ollama. Folder-on-disk storage. Mandatory consent step. | 4 |
| **v0.2** | Add OpenAI-compatible STT/LLM. Channel-split diarization (binary `Me`/`Them`). `.ics` calendar context. Webhook hook. | 3 |
| **v0.3** | Add Linux (PipeWire + Pulse fallback). | 3 |
| **v0.4** | Add Windows (WASAPI per-process loopback). Optional Parakeet via `sherpa-rs`. | 3 |
| **v0.5** | Add Android (MediaProjection + Compose UI). Neural diarizer (`pyannote-onnx`) for in-room and multi-party. | 5 |
| **v0.6** | Hooks complete (git, webhook, indexer). Multilingual regression corpus. Bench gates. | 2 |
| **v0.9-rc** | Reproducible builds, packaging (Homebrew, Scoop, AUR, Flatpak, F-Droid). Unsigned binaries on macOS / Windows with documented Gatekeeper / SmartScreen workarounds. | 2 |
| **v1.0** | Trait freeze, docs polish, release, hold for 6 months. | 1 |

Total v1.0 surface: realistically **15–20k LoC of Rust + ~1.5k Kotlin + ~500 Swift/objc2**, not 6–8k. The leanness story holds against Meetily and anarlog at this number — see the comparison table — but the headline metric is **core/adapter ratio**, not total LoC.

Total calendar effort: ~25–26 weeks of evenings to v1.0. Detailed phase breakdown in `.docs/development-plan.md`.

If this lands and the persona is real, v1 is a project I'll maintain for years. If the persona is wrong, I'll have learned that in 6 weeks (Phase 0 + v0.1) instead of 6 months.

## Where this lives

GitHub: `github.com/<my-handle>/scrybe` (claimed shortly).
License: Apache-2.0.
Docs: `system-overview.md`, `system-design.md`, this pitch.
Status: pre-code. Architecture docs are the artifact. First commit lands within 2 weeks of getting feedback from this round.

---

*If you read this far and have thoughts, my DMs are open. Honest takes welcome — including "I think you're wrong about X."*
