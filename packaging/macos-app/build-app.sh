#!/usr/bin/env bash
# build-app.sh — wrap an installed scrybe CLI into a `.app` bundle and
# optionally code-sign it.
#
# This bundle is what TCC (Transparency, Consent, and Control) needs to
# attach an Audio Capture grant against. A bare CLI binary at
# `~/.cargo/bin/scrybe` cannot receive Core Audio Tap audio because TCC
# refuses to surface the consent prompt without a bundle + Info.plist
# usage description — observed live on macOS 26 (Darwin 25.4) where the
# IO callback fired but every buffer arrived zero-filled.
#
# Usage:
#   packaging/macos-app/build-app.sh \
#       --binary ~/.cargo/bin/scrybe \
#       --output ./scrybe.app \
#       [--sign "Developer ID Application: NAME (TEAMID)"] \
#       [--sign-self "scrybe-local-signing"]
#
# Without --sign or --sign-self the bundle is left ad-hoc signed, which
# is sufficient for the bundle structure but NOT for TCC consent on
# recent macOS — pass one of the two for a working dev workflow.
#
# Per "no-bullshit-code" — every error path exits with a clear message
# rather than continuing with a partial bundle.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
TEMPLATE_PLIST="${SCRIPT_DIR}/Info.plist.template"
ENTITLEMENTS="${SCRIPT_DIR}/entitlements.plist"

usage() {
    cat <<EOF
Usage: $0 --binary <path> --output <path.app> [signing flags]

Required:
    --binary <path>           Path to the built scrybe binary (typically ~/.cargo/bin/scrybe)
    --output <path.app>       Where to write the .app bundle (e.g. ./scrybe.app)

Signing (mutually exclusive, optional):
    --sign <identity>         Real Developer ID, e.g. "Developer ID Application: Tom (ABC123XYZ)"
    --sign-self <name>        Self-signed Keychain identity, e.g. "scrybe-local-signing"

Other:
    --version <X.Y.Z>         Override version string (default: read from binary --version)
    -h, --help                Show this help

Example (self-signed dev workflow):
    $0 --binary ~/.cargo/bin/scrybe \\
       --output ./scrybe.app \\
       --sign-self scrybe-local-signing
EOF
}

BINARY=""
OUTPUT=""
SIGN_IDENTITY=""
SIGN_SELF=""
VERSION_OVERRIDE=""

while [[ $# -gt 0 ]]; do
    case "$1" in
        --binary) BINARY="$2"; shift 2 ;;
        --output) OUTPUT="$2"; shift 2 ;;
        --sign) SIGN_IDENTITY="$2"; shift 2 ;;
        --sign-self) SIGN_SELF="$2"; shift 2 ;;
        --version) VERSION_OVERRIDE="$2"; shift 2 ;;
        -h|--help) usage; exit 0 ;;
        *) echo "ERROR: unknown argument: $1" >&2; usage >&2; exit 2 ;;
    esac
done

if [[ -z "$BINARY" || -z "$OUTPUT" ]]; then
    echo "ERROR: --binary and --output are required" >&2
    usage >&2
    exit 2
fi
if [[ ! -x "$BINARY" ]]; then
    echo "ERROR: binary not found or not executable: $BINARY" >&2
    exit 2
fi
if [[ ! -f "$TEMPLATE_PLIST" ]]; then
    echo "ERROR: missing template: $TEMPLATE_PLIST" >&2
    exit 2
fi
if [[ -n "$SIGN_IDENTITY" && -n "$SIGN_SELF" ]]; then
    echo "ERROR: --sign and --sign-self are mutually exclusive" >&2
    exit 2
fi

# Derive version from the binary itself unless overridden. This keeps the
# bundle metadata in lockstep with whatever was built.
if [[ -n "$VERSION_OVERRIDE" ]]; then
    VERSION="$VERSION_OVERRIDE"
else
    VERSION="$("$BINARY" --version 2>/dev/null | awk '{print $2}')"
    if [[ -z "$VERSION" ]]; then
        echo "ERROR: failed to read version from $BINARY --version" >&2
        exit 1
    fi
fi
echo "==> binary version: $VERSION"

# Resolve absolute output path so codesign and validation paths work
# regardless of caller's CWD.
OUTPUT="$(cd "$(dirname "$OUTPUT")" 2>/dev/null && pwd)/$(basename "$OUTPUT")"

# Clean any prior bundle at this path so we never end up with a half-
# updated structure (stale Info.plist + new binary, etc).
if [[ -e "$OUTPUT" ]]; then
    echo "==> removing existing $OUTPUT"
    rm -rf "$OUTPUT"
fi

echo "==> creating bundle skeleton at $OUTPUT"
mkdir -p "$OUTPUT/Contents/MacOS"
mkdir -p "$OUTPUT/Contents/Resources"

echo "==> copying binary into bundle"
cp "$BINARY" "$OUTPUT/Contents/MacOS/scrybe"
chmod 0755 "$OUTPUT/Contents/MacOS/scrybe"

echo "==> rendering Info.plist with version=$VERSION"
sed "s/{{VERSION}}/$VERSION/g" "$TEMPLATE_PLIST" > "$OUTPUT/Contents/Info.plist"

# Validate the rendered plist before codesign — catches a malformed
# template render before TCC has a chance to silently reject the bundle.
plutil -lint "$OUTPUT/Contents/Info.plist" >/dev/null

if [[ -n "$SIGN_IDENTITY" ]]; then
    echo "==> signing with Developer ID: $SIGN_IDENTITY"
    codesign --force --options runtime \
        --sign "$SIGN_IDENTITY" \
        --entitlements "$ENTITLEMENTS" \
        "$OUTPUT"
elif [[ -n "$SIGN_SELF" ]]; then
    echo "==> signing with self-signed Keychain identity: $SIGN_SELF"
    # Verify the cert exists before invoking codesign so the error
    # message names the missing identity rather than the cryptic
    # "no identity found" codesign emits.
    if ! security find-identity -v -p codesigning | grep -Fq "\"$SIGN_SELF\""; then
        echo "ERROR: code-signing identity '$SIGN_SELF' not found in any keychain" >&2
        echo "       create one via Keychain Access → Certificate Assistant → Create a Certificate" >&2
        echo "       (Identity Type: Self Signed Root, Certificate Type: Code Signing)" >&2
        exit 1
    fi
    codesign --force --options runtime \
        --sign "$SIGN_SELF" \
        --entitlements "$ENTITLEMENTS" \
        "$OUTPUT"
else
    echo "==> WARNING: no signing identity supplied; bundle is unsigned"
    echo "    TCC will silently zero-fill Core Audio Tap buffers without a"
    echo "    stable code-signing identity. Re-run with --sign or --sign-self."
fi

echo "==> verifying bundle"
codesign --verify --deep --strict --verbose=2 "$OUTPUT" 2>&1 | sed 's/^/    /'

echo
echo "==> bundle ready: $OUTPUT"
echo "    next steps:"
echo "      1. Remove any stale TCC entry for /Users/...../scrybe via"
echo "         System Settings → Privacy & Security → Audio Recording"
echo "      2. Launch:  open $OUTPUT --args doctor --check-tap"
echo "      3. Click Allow on the Audio Capture prompt that appears"
echo "      4. Re-run the probe; expect peak > 0.01"
