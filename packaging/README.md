# Packaging templates

Source-of-truth templates for the package-manager surfaces named in `.docs/development-plan.md` §13.1. Each template is a starting point that the maintainer renders against a published release tag, then commits to the appropriate downstream repository.

## Why templates instead of generated artifacts

Submitting a new version to Homebrew, Scoop, AUR, Flatpak, or F-Droid is a stop condition (per the runbook prompt for this branch). Each submission has real-world cost — pushes to a tap repo, an AUR account-bound `git push`, an F-Droid metadata pull request that is human-reviewed by the F-Droid team. Holding the templates in-tree lets the maintainer re-render and submit when the release is otherwise green, without the build pipeline doing it implicitly.

## Render workflow

For any tag `vX.Y.Z`:

1. Compute the SHA256 of the per-target tarballs and the source archive from the GitHub Release page.
2. Render the relevant template, replacing the `{{ ... }}` placeholders with the released values.
3. Commit the rendered file to the downstream repository under the maintainer's account.
4. Watch the package-manager-specific CI (Homebrew's `brew test-bot`, AUR's manual review, Flathub's GitHub Actions, F-Droid's metadata-CI).

## Templates by manager

| Manager | Template path | Submission target |
|---|---|---|
| Homebrew | `homebrew/scrybe.rb` | `Mathews-Tom/homebrew-scrybe` (tap repo) |
| Scoop | `scoop/scrybe.json` | `Mathews-Tom/scoop-scrybe` (bucket repo) |
| AUR | `aur/PKGBUILD` | `aur:scrybe-bin` (AUR account) |
| Flatpak | `flatpak/dev.scrybe.scrybe.yaml` | Flathub PR |
| F-Droid | `fdroid/dev.scrybe.scrybe.yml` | F-Droid `fdroiddata` PR |

## Verification before submission

Each rendered manifest should be verified against the corresponding cosign-signed `SHA256SUMS.txt` from the GitHub Release before it is pushed downstream. The `cosign verify-blob` recipe lives in `INSTALL.md`; running it once locally catches a mistyped SHA256 before downstream review.

## Known template gaps (rendering checklist)

Each placeholder template carries gaps that need closing at submission time, beyond `{{ ... }}` substitution:

- **`fdroid/dev.scrybe.scrybe.yml`** — `subdir: scrybe-android` points at the Cargo crate of the same name; F-Droid builds a Gradle project, which (per `docs/system-design.md` §3) lives at `android/`. Update `subdir` to `android` once the Gradle project lands at v0.5.x.
- **`scoop/scrybe.json`** — references `scrybe-cli-x86_64-pc-windows-msvc.zip`, which the cargo-dist matrix does not yet emit. Wait until the v0.9.x cargo-dist target expansion adds Windows artifacts before submitting to the Scoop bucket.
- **`aur/PKGBUILD`** — references `scrybe-cli-{x86_64,aarch64}-unknown-linux-gnu.tar.xz`, which the cargo-dist matrix does not yet emit. Same gating as Scoop.
- **`flatpak/dev.scrybe.scrybe.yaml`** — `--share=network` is wider than the default-feature graph requires (the egress-audit lane proves no networking crates link in). Optional users opt in to cloud STT/LLM features at install time via `flatpak override --user --share=network dev.scrybe.scrybe`; consider dropping `--share=network` from the manifest before submission.
