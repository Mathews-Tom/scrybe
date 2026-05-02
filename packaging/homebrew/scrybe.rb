# Homebrew formula template for scrybe.
#
# Render against a published GitHub Release tag and commit the result
# to `Mathews-Tom/homebrew-scrybe` so users can `brew install
# Mathews-Tom/scrybe/scrybe`.
#
# Placeholders:
#   {{ version }}            — release version without the leading `v`
#                              (e.g. `0.9.0-rc1`).
#   {{ sha256_aarch64 }}     — SHA256 of `scrybe-cli-aarch64-apple-darwin.tar.xz`
#                              from the release's `SHA256SUMS.txt`.
#   {{ sha256_x86_64 }}      — SHA256 of `scrybe-cli-x86_64-apple-darwin.tar.xz`
#                              from the release's `SHA256SUMS.txt`.
#
# `brew install` from a tap is the recommended convenience-first install
# path on macOS per `INSTALL.md`. Tap-installed binaries inherit the
# tap's trust posture; no Gatekeeper "Apple cannot verify" prompt fires.

class Scrybe < Formula
  desc "Local-first meeting transcription — capture, transcribe, summarize on device"
  homepage "https://github.com/Mathews-Tom/scrybe"
  version "{{ version }}"
  license "Apache-2.0"

  on_macos do
    on_arm do
      url "https://github.com/Mathews-Tom/scrybe/releases/download/v#{version}/scrybe-cli-aarch64-apple-darwin.tar.xz"
      sha256 "{{ sha256_aarch64 }}"
    end
    on_intel do
      url "https://github.com/Mathews-Tom/scrybe/releases/download/v#{version}/scrybe-cli-x86_64-apple-darwin.tar.xz"
      sha256 "{{ sha256_x86_64 }}"
    end
  end

  def install
    bin.install "scrybe"
    doc.install "INSTALL.md", "README.md", "LICENSE"
  end

  test do
    assert_match version.to_s, shell_output("#{bin}/scrybe --version")
  end
end
