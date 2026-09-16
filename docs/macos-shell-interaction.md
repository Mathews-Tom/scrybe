# macOS Recording Shell Interaction Qualification

This runbook qualifies the installed native recording shell independently of unit tests. It exercises the real AppKit panel, macOS status item, global hotkey, shared Saving transition, and ordinary session finalization path.

## Scope

The qualification covers three representative configurations and four graceful stop inputs:

1. default hybrid, with `[shell]` omitted, stopped from the floating pill;
2. menu-bar-only, with `menu-bar-waveform` and `menu-bar-label`, stopped from the tray;
3. floating-only, with `floating-window`, stopped by the global hotkey;
4. floating-only again, stopped by the first `SIGINT`.

Automated configuration tests cover the other four valid non-empty subsets and reject empty, duplicate, or unknown values. The signal bridge test confirms that a queued signal enters the same Saving transition as the native controls.

The smoke uses synthetic capture and the stub notes provider. It proves native interaction, shell lifecycle, and complete artifact finalization without requesting capture permissions, reading meeting data, or making network calls. It does not replace microphone, system-audio, Whisper, or real-provider qualification.

## Candidate

Qualification performed on 2026-09-16:

- source commit: `538f215` (`feat(shell): add native recording indicators`);
- host: Darwin 25.6.0, arm64 Apple Silicon;
- build command: `rustup run 1.95.0 cargo build --release -p scrybe --no-default-features --features cli-shell`;
- installed candidate: `/tmp/scrybe-shell-qualification-20260916/bin/scrybe-final`;
- SHA-256: `4a14e1a64ec67628377f0ab89c0a28a83b11ab02eeb81ad4136795e5c51b0d17`.

The candidate was copied out of `target/release` before interaction testing so every configuration exercised the same binary.

## Disposable configuration

Default hybrid:

```toml
schema_version = 1
```

Menu-bar-only:

```toml
schema_version = 1

[shell]
indicators = ["menu-bar-waveform", "menu-bar-label"]
```

Floating-only:

```toml
schema_version = 1

[shell]
indicators = ["floating-window"]
```

Each run used an isolated config and session root:

```sh
SCRYBE_CONFIG=/tmp/scrybe-shell-qualification-20260916/config-default.toml \
SCRYBE_TEST_SYNTHETIC_FRAME_DELAY_MS=20 \
RUST_LOG=debug \
/tmp/scrybe-shell-qualification-20260916/bin/scrybe-final rec \
  --title reviewed-default-shell \
  --root /tmp/scrybe-shell-qualification-20260916/reviewed-default \
  --yes --source synthetic --synthetic-secs 600 --llm stub --shell
```

Replace the config, title, and root with the menu, floating, or signal variants for the other runs.

## Interaction procedures

### Default hybrid: floating Stop button

1. Start with the default config, which omits `[shell]`.
2. Confirm the five-bar status waveform and Scrybe image mark are visible.
3. Confirm one 240×44 floating pill appears near the active screen's top center.
4. Confirm another application remains frontmost.
5. Press the pill's Stop button through macOS Accessibility.
6. Within 50 ms, confirm the state reads `Saving…`, elapsed time is frozen, and the Stop button is disabled.
7. Wait for process exit and inspect the session directory.

Automation used for step 5:

```sh
osascript \
  -e 'tell application "System Events"' \
  -e 'tell process "scrybe"' \
  -e 'tell group 1 of window 1' \
  -e 'perform action "AXPress" of button 1' \
  -e 'end tell' \
  -e 'end tell' \
  -e 'end tell'
```

### Menu-bar-only: tray menu

1. Start with the menu-bar-only config.
2. Confirm the Scrybe process has no windows.
3. Confirm the status item contains the five-bar animated waveform and Scrybe image mark.
4. Open the native status menu through Accessibility.
5. Confirm the first row reads `Recording` with elapsed time and the second row is `Stop & save` with `CmdOrCtrl+Shift+R`.
6. Press `Stop & save`, wait for exit, and inspect the session directory.

Automation used for steps 4–6:

```sh
osascript \
  -e 'tell application "System Events"' \
  -e 'tell process "scrybe"' \
  -e 'perform action "AXPress" of menu bar item 1 of menu bar 1' \
  -e 'delay 0.1' \
  -e 'perform action "AXPress" of menu item 2 of menu 1 of menu bar item 1 of menu bar 1' \
  -e 'end tell' \
  -e 'end tell'
```

### Floating-only: global hotkey

1. Start with the floating-only config.
2. Confirm the Scrybe process has one 240×44 window and no menu bar.
3. Confirm another application remains frontmost.
4. Send Command-Shift-R through macOS System Events.
5. Confirm debug output records `recording stop accepted source="hotkey"`.
6. Wait for exit and inspect the session directory.

Automation used for step 4:

```sh
osascript -e 'tell application "System Events" to key code 15 using {command down, shift down}'
```

### First termination signal

1. Start with any valid shell configuration; the qualification reused the floating-only configuration.
2. Send one `SIGINT` to the Scrybe process.
3. Confirm debug output records `recording stop accepted source="signal"`.
4. Confirm the process exits 0 after ordinary finalization and produces the complete session artifact set.

The signal monitor queues the first signal into the same main-thread stop coordinator as the native controls. A second signal remains the explicit immediate-abort path and is not part of this artifact-producing smoke.

```sh
kill -INT <scrybe-pid>
```

Accessibility permission is required only for the automation above. Ordinary Scrybe shell use does not require it.

## Qualification receipt

| Configuration | Native surface evidence | Stop source | Result |
| --- | --- | --- | --- |
| Default hybrid | Five-bar waveform plus Scrybe mark visible; one 240×44 pill; Accessibility exposed the state, elapsed time, and Stop button | Floating Stop button | Debug receipt: `source="floating-window" elapsed="00:08"`; exit 0 |
| Menu-bar-only | Zero windows; one status menu exposed `Recording  00:09` and `Stop  save    CmdOrCtrl+Shift+R` | Tray `Stop & save` | Debug receipt: `source="tray" elapsed="00:17"`; exit 0 |
| Floating-only | One 240×44 pill at `(1800, 54)`; zero menu bars; ghostty remained frontmost | Command-Shift-R | Debug receipt: `source="hotkey" elapsed="00:13"`; exit 0 |
| Floating-only signal check | Same floating-only surface contract | First `SIGINT` | Debug receipt: `source="signal" elapsed="00:07"`; exit 0 |

Every run produced exactly one session folder containing:

```text
.stignore
audio.opus
meta.toml
notes.md
transcript.md
```

Session receipts:

- default hybrid: `01M2NE6REQX16QZ5BTG1SRJSAX`;
- menu-bar-only: `01M2NE7HYRF2RW1WEBMGRYFCSJ`;
- floating-only: `01M2NE8K9JSCRM9368SCP1BRVD`;
- first-signal check: `01M2NE9EY4X7R3YQ79VNEA2PYT`.

No run changed capture permissions, application signing state, Keychain contents, the user's Scrybe config, or the user's session store.
