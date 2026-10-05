#!/usr/bin/env bash
set -euo pipefail

if [[ $# != 2 ]]; then
  echo "usage: $0 VERSION OUTPUT.zip" >&2
  exit 2
fi
VERSION="$1"
OUTPUT="$2"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
APP_NAME="Yapr"

: "${SIGN_IDENTITY:?SIGN_IDENTITY must name a Developer ID Application identity}"
: "${NOTARY_KEY_PATH:?NOTARY_KEY_PATH must point at an App Store Connect API key (.p8)}"
: "${NOTARY_KEY_ID:?NOTARY_KEY_ID is required}"
: "${NOTARY_ISSUER_ID:?NOTARY_ISSUER_ID is required}"
if [[ "$SIGN_IDENTITY" != "Developer ID Application"* ]]; then
  echo "SIGN_IDENTITY must be a Developer ID Application identity, got: $SIGN_IDENTITY" >&2
  exit 1
fi

STAGE="$(mktemp -d)"
trap 'rm -rf "$STAGE"' EXIT
INSTALL_DIR="$STAGE" SIGN_IDENTITY="$SIGN_IDENTITY" "$ROOT/scripts/build-app.sh"
APP="$STAGE/$APP_NAME.app"

BUILT_VERSION="$(/usr/libexec/PlistBuddy -c 'Print :CFBundleShortVersionString' "$APP/Contents/Info.plist")"
if [[ "$BUILT_VERSION" != "$VERSION" ]]; then
  echo "built version $BUILT_VERSION does not match release version $VERSION" >&2
  exit 1
fi

ditto -c -k --keepParent "$APP" "$STAGE/notarize.zip"
xcrun notarytool submit "$STAGE/notarize.zip" \
  --key "$NOTARY_KEY_PATH" --key-id "$NOTARY_KEY_ID" --issuer "$NOTARY_ISSUER_ID" \
  --wait --output-format json >"$STAGE/notary.json"
cat "$STAGE/notary.json"
if [[ "$(jq -r .status "$STAGE/notary.json")" != "Accepted" ]]; then
  xcrun notarytool log "$(jq -r .id "$STAGE/notary.json")" \
    --key "$NOTARY_KEY_PATH" --key-id "$NOTARY_KEY_ID" --issuer "$NOTARY_ISSUER_ID" >&2 || true
  echo "notarization was not accepted" >&2
  exit 1
fi

xcrun stapler staple "$APP"
xcrun stapler validate "$APP"
spctl --assess --type execute --verbose=2 "$APP"

mkdir -p "$(dirname "$OUTPUT")"
rm -f "$OUTPUT"
ditto -c -k --keepParent "$APP" "$OUTPUT"
echo "packaged $OUTPUT"
