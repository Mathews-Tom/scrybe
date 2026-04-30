# Self-hosted CI runner — Tier-3 nightly E2E (macOS)

Tier-3 of the testing strategy in `docs/system-design.md` §11 runs nightly end-to-end capture tests on real Apple Silicon hardware with TCC grants pre-configured. GitHub-hosted `macos-14` runners cannot grant Screen Recording or Microphone permission, so the live Core Audio Taps path (`scrybe-capture-mac` `core-audio-tap` feature) is unverifiable on hosted runners — Tier-3 closes that gap.

This document is the operational procedure for setting up the runner on any Apple Silicon Mac. The dev plan §6.2 deliverable #11 originally cited a dedicated Mac mini M2 (~$599) "if budget exists"; in practice, any M-series Mac with macOS 14.4+ and ≥ 16 GB RAM serves the same role. A developer's existing M1/M2/M3 Pro MacBook Pro is sufficient as long as it stays on overnight during scheduled runs.

## Hardware requirements

| Spec | Minimum | Notes |
|---|---|---|
| CPU | Apple Silicon (M1 or newer) | `core-audio-tap` requires `arm64` darwin |
| OS | macOS 14.4+ (Sonoma) | Core Audio Taps API was introduced in 14.4 |
| RAM | 16 GB | `whisper-large-v3-turbo` resident is ~800 MB; `large-v3` is ~3 GB |
| Disk free | 50 GB | runner working tree + `target/` + cargo cache + Whisper weights |
| Network | persistent IPv4 (wired preferred; Wi-Fi fine if reliable) | required for the long-poll connection to `api.github.com` |

A laptop works. The lid must stay open or `pmset -a sleep 0` must be set during the runner's active window — see "Keep awake" below.

## One-time setup

### 1. Create a dedicated user account

Do not run the runner under the developer's primary user. Tier-3 jobs install Rust toolchains, run `cargo test`, and execute the `scrybe` binary against capture APIs; isolating that into its own account contains the blast radius of a compromised workflow.

```bash
sudo dscl . -create /Users/scrybe-runner
sudo dscl . -create /Users/scrybe-runner UserShell /bin/zsh
sudo dscl . -create /Users/scrybe-runner RealName "scrybe runner"
sudo dscl . -create /Users/scrybe-runner UniqueID "510"
sudo dscl . -create /Users/scrybe-runner PrimaryGroupID 20
sudo dscl . -create /Users/scrybe-runner NFSHomeDirectory /Users/scrybe-runner
sudo createhomedir -c -u scrybe-runner
sudo dscl . -passwd /Users/scrybe-runner
```

Grant the account the right to read its own home and run `caffeinate`. Do not grant administrator rights.

### 2. Install the GitHub Actions runner

Log in as `scrybe-runner` (over `ssh` or via fast user switching). Follow the repository's runner registration page (`https://github.com/Mathews-Tom/scrybe/settings/actions/runners/new`) — pick "macOS" / "ARM64". The page generates a one-time registration token; the canonical install pattern is:

```bash
mkdir -p ~/actions-runner && cd ~/actions-runner
curl -L -o actions-runner.tar.gz \
    https://github.com/actions/runner/releases/download/v2.321.0/actions-runner-osx-arm64-2.321.0.tar.gz
tar xzf actions-runner.tar.gz

./config.sh \
    --url https://github.com/Mathews-Tom/scrybe \
    --token <REGISTRATION_TOKEN_FROM_REPO_SETTINGS> \
    --name "$(hostname -s)-scrybe" \
    --labels self-hosted,macos,arm64 \
    --work _work \
    --unattended
```

The `--labels self-hosted,macos,arm64` triple is what `.github/workflows/nightly-e2e.yml` matches against. `self-hosted` is required by GH; `macos` and `arm64` are the discriminators against any future Linux or Intel runner.

### 3. Grant Screen Recording + Microphone TCC

The runner spawns `cargo test` which spawns the test binary. The test binary in turn calls `AudioHardwareCreateProcessTap` (Core Audio Taps) and `AVCaptureDevice` (microphone). Both are TCC-gated. macOS resolves grants by *the calling binary's code-signature identity, or its absolute path if unsigned*. The grant target therefore depends on what's calling the API:

- The `Runner.Listener` binary (`~/actions-runner/bin/Runner.Listener`) is the long-running process that owns the workflow shell.
- It spawns `Runner.Worker`, which spawns `bash`, which spawns `cargo`, which spawns the test binary in `target/debug/deps/`.
- TCC inheritance: a child process inherits its parent's *responsible process* attribution. For unsigned children of an unsigned parent, the responsible process is the most recently launched `LaunchAgent` or login item — typically `Runner.Listener` itself.

Grant Screen Recording **and** Microphone to:

1. `~/actions-runner/bin/Runner.Listener` (drag it into System Settings → Privacy & Security → Screen Recording, then Microphone).
2. `/bin/zsh` (the runner's default shell — needed when steps invoke shell commands directly).
3. The first time a workflow run fails with `kAudioHardwareUnauthorized` (status `0x1768415F`), check `Console.app` filtered on `subsystem:com.apple.TCC` to see which binary was actually denied; add it to the grant list.

After granting, `killall Runner.Listener` and let `launchctl` (next step) restart it so the new TCC state is picked up.

### 4. Install as a Login Item

```bash
cd ~/actions-runner
./svc.sh install scrybe-runner
./svc.sh start
./svc.sh status
```

`svc.sh` writes a `LaunchAgent` plist to `~/Library/LaunchAgents/actions.runner.Mathews-Tom-scrybe.<hostname>.plist` and registers it with `launchctl`. The runner will start at user login; no manual restart after reboot.

Verify the runner is online at `https://github.com/Mathews-Tom/scrybe/settings/actions/runners` — it should show as "Idle" with the labels `self-hosted, macos, arm64`.

### 5. Keep awake during scheduled runs

The default cron in `.github/workflows/nightly-e2e.yml` fires at 09:00 UTC (≈ 02:00 PT, 04:00 ET). On a laptop with the lid open, the runner is reachable but the system may have suspended `cargo` mid-build if `pmset` defaults are stricter than the run's runtime budget. Two options:

**Option A — keep the laptop awake while plugged in (recommended for laptop runners):**

```bash
sudo pmset -c sleep 0 displaysleep 30 disksleep 0
```

This sets system sleep to "never" while on AC power, but lets the display sleep after 30 minutes. Battery-only behaviour is unchanged.

**Option B — caffeinate per workflow run:**

Add a step to `nightly-e2e.yml` that runs `caffeinate -dimsu &` at job start and kills it at job end. This is simpler but only works while the runner already has a workflow assigned — it does not prevent suspension while idle waiting for a job. Stick with Option A for laptop runners.

For a dedicated Mac mini, the same `pmset -c sleep 0` setting applies. Mac minis don't have a lid; ensure the user account is configured to auto-login at boot.

### 6. Enable the workflow lane

The nightly workflow is gated by a repository variable so it stays inert until the maintainer registers a runner and is ready to switch the lane on:

```bash
gh variable set NIGHTLY_E2E_ENABLED --body 'true'
```

To pause the lane (for maintenance, dependency upgrades, or hardware downtime):

```bash
gh variable set NIGHTLY_E2E_ENABLED --body 'false'
```

The workflow `if:` evaluates the variable at scheduling time — flipping it to `false` causes future runs to no-op without queueing on the runner.

## Operational notes

### Disk hygiene

`cargo`'s build cache grows without bound. Schedule a weekly `cron` under the `scrybe-runner` account to prune:

```cron
0 4 * * 0 cd ~/actions-runner/_work/scrybe/scrybe && cargo clean --target-dir target -p scrybe-capture-mac && cargo clean --target-dir target -p scrybe-cli
```

Or use `cargo-cache --autoclean` if installed. The full `target/` for this workspace at `core-audio-tap` features sits around 3–4 GB; weekly pruning is plenty.

### Whisper model cache

`whisper-rs` downloads `ggml-large-v3-turbo.bin` on first use to `~/Library/Application Support/dev.scrybe.scrybe/models/`. The runner pulls it once (~800 MB) and reuses it across runs. If a model change lands and the runner has the old weights cached, `scrybe doctor` (or the test fixture) re-downloads with a new checksum match — see `system-design.md` §8.3 atomic-write recipe for the `*.partial → fsync → checksum → rename` model-download path.

### Triage when a job fails

1. Open the failed run in GH Actions UI.
2. Inspect "print runner identity" step output (first step in `nightly-e2e.yml`) — confirms macOS version, rustc, working tree.
3. If the failure is `kAudioHardwareUnauthorized` (TCC denial), re-check Step 3 above and grant the binary in question.
4. If the failure is "no runners online", check the runner's `_diag/Runner_*.log` for the disconnect reason. A common cause is laptop lid closed without `pmset -c sleep 0`.
5. If the failure is a real test regression, the artifact upload step preserves logs at `nightly-e2e-logs` retention 14 days.

### Security

- The runner has read/write access to its own working tree and the cargo cache. It does **not** need administrator rights.
- `Runner.Listener` connects outbound to `pipelinesghubeus*.actions.githubusercontent.com` for job assignment; no inbound port is opened.
- The runner's GitHub PAT (encrypted in `.runner` and `.credentials`) authorizes job assignment only; it does not have repository write access. PR-triggered workflows are off-limits to self-hosted runners by default per `.github/workflows/nightly-e2e.yml` (no `pull_request` trigger).
- A `pull_request` trigger on a self-hosted runner is the textbook supply-chain attack vector — a fork can submit a PR that runs arbitrary code on your hardware. We deliberately use only `schedule` and `workflow_dispatch`. Do not add `pull_request` without rethinking the trust model.

### Pausing for travel / hardware swap

- Set `gh variable set NIGHTLY_E2E_ENABLED --body 'false'` before disconnecting the runner.
- Run `~/actions-runner/svc.sh stop` to suspend the LaunchAgent (the runner stays registered but goes offline).
- To fully retire: `~/actions-runner/config.sh remove --token <REMOVAL_TOKEN>` then delete `~/actions-runner/`.

## Cross-reference

- `docs/system-design.md` §11 Tier 3 — what tests run here.
- `.docs/development-plan.md` §6.2 deliverable #11, §7.6.2 — context on why this lane exists and what it unblocks.
- `scrybe-capture-mac/src/coreaudio_tap.rs:792` — the single in-tree Tier-3 test the runner currently executes; additional E-1 through E-6 from `.docs/development-plan.md` §7.3.3 land in subsequent slices.
