---
name: Bug report
about: Report unexpected behavior in the scrybe binary, library, or build
title: "bug: <one-line summary>"
labels: ["bug", "triage"]
assignees: []
---

## What happened

A precise description of the observed behavior. Quote exact error
messages, log lines, or panic backtraces. Avoid paraphrasing.

## What you expected to happen

The behavior the documentation, system-design, or your prior session
led you to expect.

## Reproduction

Minimum steps that trigger the bug. Include input audio
characteristics if relevant (sample rate, channel count, source).

```text
1. ...
2. ...
3. ...
```

## Environment

- scrybe version (`scrybe --version` once available, otherwise the git
  SHA or release tag): 
- OS and version (e.g. macOS 14.6.1, Ubuntu 24.04, Windows 11 24H2): 
- Architecture (arm64, x86_64): 
- Install method (cargo install, Homebrew, AUR, MSI, F-Droid, source): 
- STT provider (`whisper-local`, `openai-compat <host>`, `parakeet-local`): 
- LLM provider (`ollama`, `openai-compat <host>`, none): 
- Capture adapter (`mac-coreaudio-tap`, `mac-screencapturekit`,
  `linux-pipewire`, `linux-pulse`, `win-wasapi`, `android`): 

## Logs

Attach `~/scrybe/<session>/meta.toml` if the bug is session-scoped.
Attach `RUST_LOG=debug scrybe ...` output for capture or pipeline
issues. Redact any audio content you don't want to share.

## Did the bug change behavior between versions

If you saw it work in a prior version, name that version. Bisect-able
reports get fixed faster.

## Anything else

Sleep/wake events, device hot-swaps (AirPods, USB mics), concurrent
scrybe sessions, low disk space, or other environmental factors that
might be load-bearing.
