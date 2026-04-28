# scrybe — System Overview

> **Meeting notes that never leave your laptop.**
> Open-source meeting transcription and notes. Captures audio without bots, runs on your laptop, your data on disk.

---

## 1. What scrybe is

scrybe is a single-binary, open-source meeting transcription and notetaking tool. It records meetings on your machine, transcribes them locally or via any OpenAI-compatible cloud provider, and produces structured AI summaries — without joining the call as a bot, without uploading audio anywhere by default, and without an account.

| Attribute | Value |
|---|---|
| **License** | Apache-2.0 |
| **Language** | Rust (core), Kotlin (Android shell), small platform shims (Swift/objc2) |
| **Platforms** | macOS 13+, Windows 10 build 2004+, Linux (PipeWire), Android 10+ |
| **Storage** | Plain markdown on disk. No SQLite, no proprietary format |
| **STT** | `whisper.cpp` local by default. Any OpenAI-compatible API as alternative |
| **LLM** | Ollama / LM Studio local. Any OpenAI-compatible API as alternative |
| **Network egress** | Zero by default. Only present if the user configures a cloud provider |
| **Audio capture** | Native OS APIs only. No virtual audio driver required. macOS uses Core Audio Taps (14.4+) primary, ScreenCaptureKit fallback (13.0–14.3) |
| **Account system** | None. There is nothing to sign up for |
| **Recording consent** | Mandatory pre-recording step (configurable, not removable). Recording laws are jurisdiction-dependent — see `LEGAL.md` |
| **Project posture** | Open-source software. The author publishes a binary; users run it on their own hardware. There is no managed service, no hosted endpoint, no data flow back to the author |

## 2. Why scrybe exists

The meeting notetaker market has split into three camps. None of them serves users with privacy constraints, regulatory requirements, or aesthetic preferences for local-first software.

### The three existing camps

| Camp | Examples | Mechanism | Failure mode for the target user |
|---|---|---|---|
| Bot-based SaaS | Otter, Fireflies, Read.ai, Fathom, Avoma | A virtual participant joins the call and records | Awkward and political — visible to all attendees, often blocked by IT |
| Audio-capture SaaS | Granola, tldv (some modes) | Local audio capture, then upload to vendor cloud | Cloud upload is the dealbreaker for regulated industries; account required |
| OSS notetakers | Meetily, char/anarlog (now dormant), Amical | Local capture + local STT + cloud or local LLM | Architectural sprawl. Single-platform reality despite cross-platform claims. Mid-migration debt visible in the repos |

### The user who falls through every gap

Five concrete personas, none well-served by anything currently shipping:

| Persona | Constraint | What they currently do |
|---|---|---|
| Healthcare/legal/finance professional | Cannot upload meeting audio to vendor clouds for compliance reasons (HIPAA, attorney-client privilege, MAR/MNPI) | Manual notes, or skips notes entirely |
| Open-source maintainer | Won't use SaaS notetakers on principle | Manual notes, or `obsidian-recorder` + manual transcription |
| Independent consultant | Can't have a bot join client calls without explaining what it is and why | Manual notes |
| Solo developer / SRE | BYO local model, filesystem storage, minimal dependencies, no telemetry | Half-built local Whisper scripts, abandoned monthly |
| Multi-language professional outside the US/EU | English-centric SaaS tools, expensive cloud STT for their language, latency to nearest region | Manual notes in native language |

The bot-based and SaaS audio-capture tools are not technical failures for these users. They are policy failures. No amount of feature improvement closes the gap; the architecture is the problem.

### The aesthetic case, separate from the policy case

A growing developer cohort actively prefers local-first software for reasons that have nothing to do with compliance: reproducibility, longevity beyond a vendor's funding runway, the ability to grep meeting notes the same way they grep code, the absence of telemetry. This cohort is small in absolute terms but disproportionately influential — they write the blog posts, file the issues, and contribute the patches that compound an OSS project's surface area.

## 3. How it works

Architecturally, scrybe is one Rust core with cargo-feature-gated capture adapters per platform and four small extension seams.

### The capture problem (the only genuinely hard problem)

Capturing **system audio** (the other end of a Zoom/Teams/Meet call) without installing a virtual audio driver is the single technical challenge worth talking about. Modern OS APIs solve it differently on each platform:

| Platform | API | Driver needed? | Min OS |
|---|---|---|---|
| macOS | ScreenCaptureKit | No | macOS 13.0 |
| Windows | WASAPI loopback (system) + `AUDIOCLIENT_PROCESS_LOOPBACK_PARAMS` (per-app) | No | Win 10 build 2004 (May 2020) |
| Linux | PipeWire monitor sources, PulseAudio fallback | No | Recent distros |
| Android | `AudioPlaybackCaptureConfiguration` + MediaProjection | No | API 29 (Android 10) |
| **iOS** | **Not possible from arbitrary apps. Excluded by Apple's sandbox model. Out of scope.** | — | — |

scrybe captures the **microphone on one channel and system audio on the other**, transcribes each independently, and merges by timestamp. This produces binary speaker attribution (`Me:` / `Them:`) for **one-on-one remote calls** without diarization models — a deliberate engineering choice to delete a problem category for the modal 1:1 use case.

The honest scope: binary `Me`/`Them` is correct for 1:1 remote calls and degrades for in-room meetings (one mic, multiple physical speakers) and ≥3-party remote calls (one system channel, multiple voices labeled `Them:`). The `Diarizer` trait (system-design §4.4) accepts a neural fallback (`pyannote-onnx`, v0.5+) for those cases. v0.1 ships the channel-split path; users with multi-party meetings get correct transcript content with a single `Them:` label per non-self utterance until v0.5.

### Pipeline

```
mic + system → channel split → 30s VAD-aware chunker → STT provider → markdown transcript
                                                                     → buffered until end-of-session
                                                                                                ↓
                                                        complete transcript + meeting context → LLM provider → notes.md
```

Two STT calls per chunk (one per channel), one LLM call at end-of-session. The pipeline is intentionally **batch-windowed, not streaming**. Streaming buys ~10% better UX at ~5x the engineering cost; deferred to v2 if anyone asks.

### Storage

```
~/scrybe/2026-04-29-1430-standup/
  audio.opus           # final compressed recording
  transcript.md        # appended during call
  notes.md             # generated post-call
  meta.toml            # title, duration, providers used, model versions
```

That is the entire schema. The folder is the database. `grep` is the search engine for v1. Sync is whatever the user already uses (iCloud, Syncthing, Obsidian vault, git). When indexed search becomes necessary, a regenerable `tantivy` index sits alongside, never as source of truth.

### The four extension seams

Everything that could plausibly need to vary between users or future versions fits through one of four traits. No fifth seam is added until a fifth real need exists.

| Trait | Purpose | v1 implementations |
|---|---|---|
| `AudioCapture` | Platform-specific audio source | `mac` (Core Audio Taps + ScreenCaptureKit fallback), `win` (WASAPI loopback), `linux` (PipeWire + Pulse fallback), `android` (MediaProjection) |
| `ContextProvider` | Pre-call context: title, attendees, language hint, prior notes | `cli`, `ics-file` |
| `SttProvider` / `LlmProvider` | Transcription and summarization backends | `whisper-local`, `openai-compat`; optional `parakeet-local` via `sherpa-rs` |
| `Diarizer` | Speaker attribution strategy | `binary-channel` (default, v0.1), `pyannote-onnx` (v0.5, multi-party) |
| `Hook` | Post-event subscribers: async, errors surfaced via `LifecycleEvent::HookFailed` | webhook, git, tantivy indexer (each behind cargo features) |

Calendar integration is not a system; it is an `IcsFileProvider` returning a `MeetingContext`. Multi-language is not an i18n framework; it is one field on `MeetingContext` plus the LLM prompt. Future integrations slot in as `context-*` or `hook-*` crates without touching the core.

## 4. Market research

### Direct comparison — five most relevant tools

Numbers below are best public estimates as of April 2026; OSS metrics are exact, SaaS metrics are pieced together from vendor blog posts and independent reporting.

| Tool | Type | License | OS coverage | Model | Local STT | Local LLM | Filesystem storage | Account required |
|---|---|---|---|---|---|---|---|---|
| **Granola** | Audio-capture SaaS | Closed | macOS only | Free + paid tiers | No | No | No (cloud) | Yes |
| **Otter.ai** | Bot-based SaaS | Closed | Web/iOS/Android | Free + paid | No | No | No | Yes |
| **Fireflies** | Bot-based SaaS | Closed | Web | Paid | No | No | No | Yes |
| **Fathom** | Bot-based SaaS | Closed | Mac/Win | Free + paid | No | No | No | Yes |
| **Meetily** | OSS audio-capture | MIT | macOS, Windows | OSS + paid Pro | Yes | Yes | Partial (SQLite + audio files) | No |
| **fastrepl/anarlog** (formerly Hyprnote) | OSS audio-capture, active | MIT | macOS only | Yes | Yes | Yes | Markdown + libsql | Optional |
| **scrybe** (proposed) | OSS audio-capture | Apache-2.0 | macOS, Windows, Linux, Android | Yes | Yes | Pure markdown | No |

### What's missing from the market

1. **No cross-platform OSS option that actually ships on more than two platforms.** Meetily is mac+win. anarlog is mac-only (active 2026-04). No serious option exists on Linux or Android.
2. **No OSS option with truly minimal storage.** Every existing OSS option ships SQLite or libsql, sometimes with vector DBs on top. None of them treat the filesystem as the primary data model.
3. **No OSS option that has resisted the urge to add accounts/sync.** Meetily ships a paid Pro tier with hosted AI. The OSS-pure-forever niche is unoccupied.
4. **No option (OSS or SaaS) that ships on Android with system audio capture.** Voice memo apps exist; meeting capture apps do not.
5. **No OSS option ships an in-meeting consent UX.** SaaS tools (Granola, Read.ai, Fireflies, Fathom) have different versions of this; OSS tools don't have it at all. *Brewer v. Otter.ai* (2025) and *Cruz v. Fireflies.AI* (2025, BIPA) are putting consent UX on the regulatory map. scrybe ships consent-by-default. |

### Recent market signals

Granola raised a Series B in 2024 valuing the company at a reported ~$250M. Otter is profitable, large, and now defending a class action (*Brewer v. Otter.ai*, N.D. Cal. Aug 2025). Fireflies is defending a separate BIPA action (*Cruz v. Fireflies.AI*, Dec 2025). Meetily passed 11,400 GitHub stars in early 2026 and now ships a paid tier. The team behind char rebranded to char.com (productivity-focused), but the open-source meeting-recorder under the `fastrepl/anarlog` repo is **active** with commits in April 2026 — earlier framing of "abandoned" was wrong. The market is real and growing; the SaaS-side consent model is under legal pressure; the OSS shelf has two active Rust competitors (Meetily, anarlog) but no cross-platform-with-Linux-and-Android option, and no in-meeting consent UX.

This is the window. The successful OSS project is climbing the SaaS ladder; the technically excellent OSS project has moved on. A clean, lean, multi-platform OSS option has no current incumbent.

## 5. Risks (honest)

### Risks that could kill the project

| Risk | Probability | Severity | Mitigation |
|---|---|---|---|
| macOS Screen Recording permission UX kills first-run conversion | Low (with Core Audio Taps) / High (ScreenCaptureKit fallback) | Medium | Default to Core Audio Taps on macOS 14.4+ (no screen-recording permission, no orange dot). ScreenCaptureKit is the fallback for 13.0–14.3 only |
| Whisper accuracy below user expectations for non-English | Medium | Medium | Default to `whisper-large-v3-turbo` not `tiny`; opt-in `large-v3` for max accuracy; ship Parakeet (via `sherpa-rs`) in v0.4; document cloud STT for hard languages |
| Local LLM summary quality below GPT-4 expectations | High | Medium | Default to recommending OpenAI-compat with the user's BYO key on ≤ 16 GB systems; local-only path explicitly "ok" not "great" |
| Apple deprecates Core Audio Taps / ScreenCaptureKit | Low | High | Same risk all competitors carry; vendor `screencapturekit-rs` and ship a `coreaudio-tap-rs` binding (none exists today; writing one is a useful side artifact) |
| Google tightens MediaProjection on Android 16+ | Medium | Medium | Already a moving target; track AOSP; degrade gracefully to mic-only |
| Bus factor (solo maintainer) | Default state | High | Documented architecture, small core, Apache-2.0 license — explicit patent grant under §3 plus attribution-preserving §4 — so forks survive |
| Meetily / anarlog ship scrybe's feature set first | Medium | Medium | Differentiation is Linux+Android coverage + consent-by-default + filesystem-only, not first-mover. Even if a competitor reaches Linux+Android, the OSS world is better off |
| Author legal exposure for publishing a recording-adjacent tool | Low | Medium | See `LEGAL.md`. HHS FAQ 256, GDPR Art. 4 controller test, AI Act Recital 102 FOSS carve-out, *MGM v. Grokster* inducement standard, Apache-2.0 §7–§8 enforceability all favor the OSS publisher posture. Mitigation: ship consent UX as default; neutral marketing; no managed service |
| Two-party-consent jurisdiction litigation against a downstream user | Medium | Low (for author) | User-facing risk, not author-facing. Mitigation is education + consent UX. `LEGAL.md` documents jurisdiction matrix |

### Risks I'm explicitly choosing to accept

| Decision | What I accept |
|---|---|
| Defer iOS | Loses the "I want this on my phone" persona. Real, but unavoidable per Apple's sandbox |
| Defer streaming transcription UX | Loses the "watch the words appear" demo wow-factor. Acceptable; product is a notetaker, not a karaoke app |
| Defer plugin runtime | Loses extensibility-by-strangers. Acceptable; static-compiled hooks cover the use cases that exist |
| Defer accounts and sync | Loses every persona who wants "open it on phone, see notes from laptop." Acceptable; the user already has Syncthing or iCloud |
| Apache-2.0, not MIT and not AGPL | Apache-2.0 §3 provides an explicit patent grant; §4 enforces attribution and modification notice on derivative works; §6 protects the project's name from being used to endorse derivatives. MIT lacks the patent grant and the attribution teeth; AGPL deters the corporate-OSS-contributor population that compounds project growth. Accept that someone could fork into a closed-source SaaS — the goal is impact, not capture |

## 6. Defensibility — the M-word, honestly

OSS projects don't have economic moats. They have niches and credibility. Pretending otherwise is exactly the kind of marketing-vs-reality gap that bit char/anarlog (see the "anarlog README vs. actual codebase" critique elsewhere).

### Real differentiators

1. **Architectural discipline as a measurable feature.** Meetily's frontend is 41k LoC of Rust with `audio/` and `audio_v2/` and `lib_old_complex.rs`. char/anarlog is 213k LoC. scrybe's v1 target is 6–8k LoC. This is a user-visible attribute, not vanity: install size, build time, audit surface, contributor onboarding all flow from it.
2. **True cross-platform coverage from v1.** No current OSS option ships clean macOS + Windows + Linux + Android. This is a moat in the sense that the engineering investment to reach four platforms with one core is non-trivial, and most projects pick one platform and stay there.
3. **Filesystem-as-database as a philosophical commitment.** A real subset of users wants this specifically. They are not served by SQLite-backed alternatives. A small, vocal, contribution-active user base.
4. **The author's no-magic / `archex` / `no-magic-ai` brand context.** Distribution credibility on day one, especially on technical channels (Hacker News, Lobste.rs, Rust subreddit, LinkedIn `#AIPraxisPulse` audience).
5. **Smaller-scope honesty as a positioning.** "We don't do calendar OAuth, we don't do mobile sync, we don't do Slack integration. If you need those, use Meetily or Granola." Most OSS projects can't help themselves; they bolt features on until they look like every other tool. Holding the line is a position.

### Things that are not moats

- Being open-source. Everyone in the OSS column is open-source.
- Local-first. Meetily and char are also local-first.
- Privacy. Every competitor claims privacy.
- BYO LLM. Everyone supports this now.

The honest defensibility statement: scrybe survives by being **measurably leaner and more cross-platform than any current OSS alternative, and by holding architectural discipline as a feature**. It does not survive on any single technical capability. If Meetily were to rewrite their frontend with the same discipline, we'd compete on Android coverage. If they reached Android, we'd compete on what you couldn't bolt onto their architecture.

## 7. Concrete usage examples

These are not hypothetical personas. They are the testable hypotheses that justify building this.

### Example 1 — Solo consultant on a client call

A consultant is on a Google Meet with a new client. They press a global hotkey. scrybe begins recording mic + system audio. The macOS screen-recording orange dot appears (this is honest — explained in onboarding). The call ends; they press the hotkey again. Within ~30 seconds, `~/scrybe/2026-04-29-1500-acme-discovery/notes.md` exists with action items, decisions, and follow-ups. Nobody on the client side ever knew a tool was involved. The notes never left the laptop.

### Example 2 — Healthcare provider dictation, post-consult

A clinician records their own dictated notes after a patient consultation (the patient is not in the recording). HIPAA prevents using Otter or Granola for any patient-recording use case; their employer prevents installing arbitrary apps. scrybe runs as a single binary in their user account. The audio file and transcript live in `~/Documents/clinical-notes/` covered by the institution's encrypted disk policy. The author of scrybe is not a HIPAA business associate (HHS FAQ 256: "the mere selling or providing of software… does not give rise to a business associate relationship"). The clinician's institutional posture is the controlling factor; scrybe's local-first architecture makes the procurement review trivial.

For *patient-present* consultations, the consent step (system-design §5) gates capture: the clinician must obtain documented patient consent before recording, and the disposition is logged into `meta.toml`. This is the user's responsibility, not scrybe's; scrybe's role is to make the consent step impossible to skip.

### Example 3 — Open-source maintainer triage call

An OSS maintainer hops on a community call. They want notes for the project's public archive. scrybe records, transcribes, summarizes locally. A `git` hook (one of the optional `Hook` implementations) commits the notes to the project's wiki repo automatically. Zero SaaS in the loop; the entire artifact chain is reproducible.

### Example 4 — Defense / classified-adjacent work

A contractor in a SCIF-adjacent environment cannot run anything that phones home. scrybe is provably air-gappable: `cargo build --no-default-features --features mac,whisper-local` produces a binary with zero outbound network capability. They can audit this themselves; it's Apache-2.0-licensed Rust without a runtime. This is the kind of user nobody markets to because they don't show up in funnel analytics.

### Example 5 — Meeting on an Android phone in a coffee shop

A founder is on a quick standup over Google Meet on their Android phone. They tap the scrybe widget. Audio capture begins via `MediaProjection`. The transcription happens on-device using a quantized Whisper model (slow, ~2x realtime on a Pixel 8); LLM summary uses their configured Ollama instance over Tailscale to their home machine. Everything in `~/storage/scrybe/` on the device. This persona is genuinely unserved today.

## 8. What scrybe is *not*, said plainly

| Not | Reason |
|---|---|
| A SaaS, even a "self-hosted" one | The architecture is single-binary on user-owned hardware. There is no server component |
| A bot that joins meetings | Architecturally impossible by design. Audio capture is local |
| A real-time live captioning tool | Batch-windowed (30s chunks). Ship streaming in v2 if a real use case appears |
| A mobile-first product | Desktop-first. Android is supported; iOS is not |
| A team product | Single-user. Sharing is whatever the user already does (git, Dropbox, email) |
| An integration platform | Hooks exist; OAuth runtimes do not |
| A startup | OSS project. No funding round, no growth team, no customer success |

## 9. Success criteria

How will I know this worked?

| Horizon | Metric | Target |
|---|---|---|
| 6 months | GitHub stars | 1,000+ |
| 6 months | Monthly active issue authors | 20+ |
| 12 months | Distinct contributors with merged PRs | 25+ |
| 12 months | Platform coverage shipped | All four (mac, win, linux, android) |
| 12 months | "scrybe" appears in 3+ comparison articles for OSS notetakers | Implicit endorsement |
| 24 months | Has not collected a single account credential, processed a single dollar, or phoned home once | Foundational claim still holds |

If at 24 months scrybe has accumulated a "Pro tier", an account system, or a hosted backend — even with the best of intentions — it has failed at its founding constraint. The architectural discipline is the product.
