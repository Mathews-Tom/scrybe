---
name: Feature request
about: Propose a change to scrybe's behavior, configuration, or extension seams
title: "feat: <one-line summary>"
labels: ["enhancement", "triage"]
assignees: []
---

## The problem

Describe the user-visible problem the feature solves. Concrete usage
scenarios beat abstract capability claims. If the problem is "scrybe
should support X," explain what X enables that the user cannot do
today.

## The proposed change

The smallest change that solves the problem. If the change touches one
of the four extension seams (`AudioCapture`, `ContextProvider`,
`SttProvider`/`LlmProvider`, `Diarizer`, `Hook`), describe how it fits
the existing trait shape rather than adding a new one. New seams
require a higher bar; see `docs/system-design.md` §1 non-goals.

## Alternatives considered

What you tried, what is already possible via configuration or hooks,
and why those approaches do not solve the problem.

## Scope check

- [ ] The change keeps scrybe single-binary, filesystem-as-database,
      zero-egress-by-default. Features that introduce a server, an
      account system, a plugin runtime, or telemetry are explicitly
      out of scope (`docs/pitch.md` §"What I'm explicitly not
      building").
- [ ] The change does not require a new platform target or undermine
      one of the four supported targets (macOS, Windows, Linux,
      Android).
- [ ] The change can ship under Apache-2.0 without pulling in a
      copyleft dependency.

## Anything else

Prior art from competitors (Meetily, anarlog, Granola), upstream
issues from `whisper-rs` / `sherpa-rs`, or relevant OS-API
documentation that informs the design.
