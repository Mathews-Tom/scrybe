# Recording from the desktop application

How a recording starts, what stops it, what it leaves behind, and what happens when each of those fails. The engineering contract for the state model itself is `docs/system-design.md`; this describes the surfaces built on it.

## The one state model

Every surface — the main window, the tray, the global accelerator, a terminating signal, and the act of quitting — reads and drives one `RecordingController`, obtained from the composition root. There is no second instance, no second clock, and no second stop coordinator. Elapsed time comes from a single monotonic instant the controller owns; a surface that wants a ticking display re-reads the controller rather than counting forward, because a local counter would drift from it and would keep ticking through a stop it had not heard about.

The states are `Idle → Preparing → Recording → Saving → Completed → Idle`, with `Failed` reachable from `Preparing`, `Recording` and `Saving` and settling back to `Idle` the same way `Completed` does.

## Before anything is written

A recording is resolved and then checked before a byte is written.

**Resolution** turns the configuration file plus whatever the surface overrode into a plan: capture source, system-audio adapter, input device, transcription model, notes backend, consent mode, storage root. A value the release does not define — a misspelled `[record].source`, say — is refused rather than replaced by a default, because a recording that silently captured the in-process tone would hand a reader a session they believe is their meeting.

**Preflight** then answers seven things separately:

| Check | What it establishes |
| --- | --- |
| Configuration | The plan resolved at all. |
| Permissions | **Nothing.** See below. |
| Device | The named Core Audio input device is one the platform offers. |
| Provider | The notes provider the plan names is one this build carries. |
| Model | The transcription model the plan names exists and is loadable. |
| Storage | The session folder can be created under the plan's root. |
| Capture | This build carries the capture the plan names. |

Preflight **writes nothing**. That is what makes the guarantee checkable rather than asserted: after a refusal there is no session folder, and where the storage root did not exist beforehand there is no storage root either.

### What the permission check does not do

It does not check the macOS capture permission. Nothing in this release measures a TCC grant — the diagnostics layer reports capture as unverified for the same reason — so the check reports `Unverified`, never blocks, and says in its own summary which prompt the platform will raise instead. A surface rendering it must present it as something that was not checked. The desktop window lists the unverified findings separately from the blocking ones, because presenting "not checked" beside "failed" reads as though both had been measured.

## What the desktop build can open

`recording::support()` reports what the binary actually linked, written under the same conditions as the capture construction, so preflight cannot clear a source the host then refuses:

- **Capture**: the default input device through `cpal`, with the `mic-capture` feature (on by default). Without it, only the in-process synthetic source.
- **Transcription**: no model runtime. A configured whisper or Sherpa model is **refused** rather than silently replaced by the stub, because a reader who configured a model and got the stub's fixed line would have a transcript that is not of their meeting and no sign of it.
- **Notes**: the configured OpenAI-compatible provider, with the `notes-generation` feature (on by default).

macOS system audio (`mic+system`) is not available from the desktop host.

## The six stop sources

A stop can come from the window's `Stop & save`, the tray, the floating pill, the global accelerator, a terminating signal, or a quit. Every one of them calls the same `Stop::request`, which asks the controller and tears capture down **only if the controller accepted**.

The primitive is the controller's mutex. The invariant is that `stop_requested` moves from false to true inside the same lock acquisition that reads it, so exactly one caller ever observes an acceptance — whatever order the surfaces arrive in, and however simultaneously. Any combination therefore produces exactly one `Recording → Saving` transition and exactly one finalization.

The floating pill is extracted and tested but is not wired into the desktop host in this release; it is driven by the command-line shell.

## Saving

Finalization is four ordered steps, published as they run: finalising the transcript, encoding the audio, generating the notes, writing the session details. The payload carries a step, a position and a total, and nothing that was said — the projection deliberately refuses to derive a step from the one pipeline event that holds a transcribed line, so no indicator, event, or log can come to carry meeting content.

A new recording is refused while one is saving.

## Quitting

| State | What quitting does |
| --- | --- |
| Idle, Completed, Failed | Exits immediately. |
| Preparing, Recording, Saving | Requests a stop, then exits once the recording reaches a terminal state. |

It does not refuse. A reader who asks a recorder to quit is asking it to finish, not to argue. It also does not exit straight away: at that moment the session is *recoverable* but not *complete*, and a process that left would hand the reader a folder to repair rather than a session to read.

Nothing on the quit path deletes, truncates, or renames. The only action it takes on a recording is the same stop request `Stop & save` makes.

**The emergency route.** The first terminating signal is treated as a quit request. The second is deliberately not intercepted, and `SIGKILL` cannot be. Both leave the session folder with its journal, which is exactly what `scrybe repair` reconstructs a session from — so an unavoidable kill costs a repair, not the recording.

## How a recording fails

| Boundary | What is on disk | What a surface is told |
| --- | --- | --- |
| Preflight | Nothing. Not even the storage root, if it did not already exist. | `preflight_failed`, naming every check that blocked. |
| Capture | Not settled here. The pipeline creates the session folder before it reports the progress that moves the controller into `Recording`, so a journal should exist by the time this label is reachable — but the only test that produces this label fails the controller directly without running a pipeline, so it proves the label and nothing about the bytes. | `Capture` — the failure kind whose documented meaning is that a recoverable journal may exist. |
| Finalization | The session folder, and whatever the pipeline finished writing. | `Finalization` — audio exists and the session is repairable. |

The kind is derived from the state the controller was in when the failure arrived, never from where in a call stack it was raised. That is what keeps the label honest about ordering: a failure cannot be called `Capture` once finalisation has begun. It is not the same as proving the disk matches — only the `Finalization` row is settled that way, by a test that induces a real mid-stream capture failure and then enumerates the session folder.

**A capture source that fails part-way does not abandon the recording.** The frames already captured are finalised in full — transcript, notes, encoded audio and `meta.toml` all written — and the capture error is surfaced afterwards as the returned error's source. A reader whose microphone is unplugged mid-meeting keeps everything that was said before it was. Because the pipeline has reached finalisation by then, that failure is labelled `Finalization` rather than `Capture`, which is accurate: the audio does exist.

The failure summary a surface renders is a fixed line. The detailed error, which carries paths, device identities and provider names through its source chain, goes to a log and to the caller, never into an event a window or a tray renders.

## Qualifying it

```sh
python3 scripts/qualify-desktop-app.py --hermetic --scenario recording
```

Drives the built bundle against a disposable storage root: starts a recording from the tray, refuses a second, converges two stops on one finalization, asserts the artifacts the pipeline wrote, and quits. It uses the synthetic source and the stub providers, so it needs no device, no permission grant, and no network.
