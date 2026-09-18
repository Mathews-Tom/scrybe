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
#       ( --sign "Developer ID Application: NAME (TEAMID)"
#       | --sign-self "scrybe-local-signing"
#       | --unsigned )
#
# One of the three is REQUIRED. It was optional, and the default was to
# print a warning and carry on, which meant the ordinary way to run this
# script produced an unsigned bundle that nothing downstream refused. An
# unsigned bundle is not a slightly worse bundle: TCC silently zero-fills
# Core Audio Tap buffers without a stable identity, and Gatekeeper
# refuses the artifact on any machine that did not build it.
#
# --sign is the only mode that produces something shippable, and it is
# verified afterwards by `scripts/check-signed-artifact.py`, which reads
# the signing authority by name. `codesign --verify` is deliberately not
# the verification: it returns exit 0 for an ad-hoc-signed bundle that
# `spctl` rejects, which was measured on this repository's own output.
#
# --sign-self and --unsigned are development modes. Both say so on
# stdout, and neither may be used to produce a release artifact.
#
# Per "no-bullshit-code" — every error path exits with a clear message
# rather than continuing with a partial bundle.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
TEMPLATE_PLIST="${SCRIPT_DIR}/../../scrybe-cli/assets/macos/Info.plist.template"
ENTITLEMENTS="${SCRIPT_DIR}/../../scrybe-cli/assets/macos/entitlements.plist"

usage() {
    cat <<EOF
Usage: $0 --binary <path> --output <path.app> [signing flags]

Required:
    --binary <path>           Path to the built scrybe binary (typically ~/.cargo/bin/scrybe)
    --output <path.app>       Where to write the .app bundle (e.g. ./scrybe.app)

Signing (mutually exclusive, exactly one REQUIRED):
    --sign <identity>         Real Developer ID, e.g. "Developer ID Application: Tom (ABC123XYZ)"
                              The only mode that produces a shippable bundle. Verified
                              afterwards by scripts/check-signed-artifact.py.
    --sign-self <name>        Self-signed Keychain identity, e.g. "scrybe-local-signing"
                              Development only. Gatekeeper rejects the result.
    --unsigned                Deliberately unsigned, for local structure checks only.
                              TCC will zero-fill Core Audio Tap buffers.

Other:
    --version <X.Y.Z>         Override version string (default: read from binary --version)
    -h, --help                Show this help

Example (self-signed dev workflow):
    $0 --binary ~/.cargo/bin/scrybe \\
       --output ./scrybe.app \\
       --sign-self scrybe-local-signing

Example (shippable):
    $0 --binary ~/.cargo/bin/scrybe \\
       --output ./scrybe.app \\
       --sign "Developer ID Application: NAME (TEAMID)"
EOF
}

BINARY=""
OUTPUT=""
SIGN_IDENTITY=""
SIGN_SELF=""
UNSIGNED=0
VERSION_OVERRIDE=""

while [[ $# -gt 0 ]]; do
    case "$1" in
        --binary) BINARY="$2"; shift 2 ;;
        --output) OUTPUT="$2"; shift 2 ;;
        --sign) SIGN_IDENTITY="$2"; shift 2 ;;
        --sign-self) SIGN_SELF="$2"; shift 2 ;;
        --unsigned) UNSIGNED=1; shift ;;
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
# Exactly one signing mode, chosen explicitly. Counting them rather than
# testing pairs, so adding a fourth mode cannot quietly reintroduce a
# combination nobody checked.
MODES=0
[[ -n "$SIGN_IDENTITY" ]] && MODES=$((MODES + 1))
[[ -n "$SIGN_SELF" ]] && MODES=$((MODES + 1))
[[ "$UNSIGNED" -eq 1 ]] && MODES=$((MODES + 1))
if [[ "$MODES" -gt 1 ]]; then
    echo "ERROR: --sign, --sign-self, and --unsigned are mutually exclusive" >&2
    exit 2
fi
if [[ "$MODES" -eq 0 ]]; then
    echo "ERROR: no signing mode was chosen, and there is no longer a default." >&2
    echo "" >&2
    echo "       This script used to warn and carry on, which made an unsigned bundle" >&2
    echo "       the result of running it the ordinary way. An unsigned bundle cannot" >&2
    echo "       receive a TCC Audio Capture grant — the Core Audio Tap IO callback" >&2
    echo "       fires and every buffer arrives zero-filled — and Gatekeeper refuses" >&2
    echo "       it on any machine that did not build it." >&2
    echo "" >&2
    echo "       Choose one:" >&2
    echo "         --sign \"Developer ID Application: NAME (TEAMID)\"" >&2
    echo "                         shippable; verified afterwards by" >&2
    echo "                         scripts/check-signed-artifact.py" >&2
    echo "         --sign-self scrybe-local-signing" >&2
    echo "                         local development; Gatekeeper rejects the result" >&2
    echo "         --unsigned      local structure checks only; states plainly that" >&2
    echo "                         the result is not shippable" >&2
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
    # Check the certificate is here before invoking codesign, so an absent
    # credential says so in those words. Without this, `codesign` aborts
    # the script with "no identity found", which reads like a typo rather
    # than like "nobody has issued this machine a distribution
    # certificate" — the same reason --sign-self has had this check all
    # along.
    # Require the shape as well as the presence. A substring search alone
    # matched loosely enough that `--sign -` — ad-hoc signing, which names
    # nobody — passed this check against an unrelated identity line.
    if [[ "$SIGN_IDENTITY" != "Developer ID Application: "* ]]; then
        echo "ERROR: --sign takes a Developer ID Application identity, e.g." >&2
        echo "       --sign \"Developer ID Application: NAME (TEAMID)\"" >&2
        echo "       Got: '$SIGN_IDENTITY'" >&2
        echo "" >&2
        echo "       For local work use --sign-self or --unsigned. Ad-hoc signing" >&2
        echo "       (--sign -) is not a mode here: it names nobody, and it is the" >&2
        echo "       artifact 'codesign --verify' reports as valid while Gatekeeper" >&2
        echo "       refuses it." >&2
        exit 2
    fi
    if ! security find-identity -v -p codesigning | grep -Fq "$SIGN_IDENTITY"; then
        echo "ERROR: no code-signing identity matching '$SIGN_IDENTITY' is in any keychain." >&2
        echo "       A shippable bundle needs a Developer ID Application certificate," >&2
        echo "       which comes from an Apple Developer Program membership. This is a" >&2
        echo "       missing credential, not a defect in this script." >&2
        echo "" >&2
        echo "       Identities this machine does have:" >&2
        security find-identity -v -p codesigning | sed 's/^/       /' >&2
        echo "" >&2
        echo "       For local work, use --sign-self or --unsigned instead; neither" >&2
        echo "       produces a shippable bundle and both say so." >&2
        exit 1
    fi
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
    echo "==> NOT SIGNING: --unsigned was passed"
    echo "    This bundle is a development artifact and is not shippable."
    echo "    TCC will silently zero-fill Core Audio Tap buffers without a"
    echo "    stable code-signing identity, and Gatekeeper will refuse this"
    echo "    bundle on any machine that did not build it."
fi

# NOTE: TCC's Audio Capture grant is bound to the bundle launched via
# Launch Services (`open ./scrybe.app`), NOT to the inner binary at
# `./scrybe.app/Contents/MacOS/scrybe`. Even when both are codesigned
# with `--identifier dev.scrybe.scrybe`, direct invocation of the inner
# binary bypasses Launch Services and silently zero-fills the tap. The
# bundle MUST be launched through `open` for the grant to apply. The
# trailing instructions reflect this.

# `codesign --verify --deep --strict` used to be the verification here. It
# is not one: it returns exit 0 for an ad-hoc-signed bundle that `spctl`
# rejects, measured on this repository's own output. It answers "is this
# signature internally consistent", never "who signed this".
#
# The Developer ID mode is therefore verified by reading the signing
# authority by name. The two development modes are not put through that
# assertion, because it is designed to refuse exactly what they produce;
# they are told what they are instead.
ASSERT="${SCRIPT_DIR}/../../scripts/check-signed-artifact.py"
if [[ -n "$SIGN_IDENTITY" ]]; then
    echo "==> asserting the bundle carries a Developer ID identity"
    if [[ ! -f "$ASSERT" ]]; then
        echo "ERROR: missing $ASSERT; a shippable bundle must not be reported" >&2
        echo "       as signed by a script that could not check it" >&2
        exit 1
    fi
    if ! python3 "$ASSERT" --bundle "$OUTPUT" --assert signature --assert gatekeeper; then
        echo >&2
        echo "ERROR: this bundle was signed and did not pass the assertions above." >&2
        echo "       It is not shippable. Nothing downstream may treat it as signed." >&2
        exit 1
    fi
else
    echo "==> NOT VERIFYING: this is a development bundle, not a release artifact"
    echo "    Run scripts/check-signed-artifact.py against a --sign build to"
    echo "    assert a Developer ID identity, a parsed Gatekeeper verdict, and"
    echo "    a notarization ticket."
fi

echo
echo "==> bundle ready: $OUTPUT"
echo "    next steps:"
echo "      1. Run: SCRYBE_BUNDLE=\"$OUTPUT\" scrybe doctor --check-tap"
echo "      2. Click Allow on the Audio Capture prompt that appears"
echo "      3. Re-run the probe if the first result is not peak > 0.01"
