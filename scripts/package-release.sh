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
IDENTITY="${SIGN_IDENTITY:-Yapr Release Signing}"

STAGE="$(mktemp -d)"
trap 'rm -rf "$STAGE"' EXIT
INSTALL_DIR="$STAGE" SIGN_IDENTITY="$IDENTITY" "$ROOT/scripts/build-app.sh"
APP="$STAGE/$APP_NAME.app"

BUILT_VERSION="$(/usr/libexec/PlistBuddy -c 'Print :CFBundleShortVersionString' "$APP/Contents/Info.plist")"
if [[ "$BUILT_VERSION" != "$VERSION" ]]; then
  echo "built version $BUILT_VERSION does not match release version $VERSION" >&2
  exit 1
fi

SIGNER="$(codesign -dvv "$APP" 2>&1 | sed -n 's/^Authority=//p' | head -1)"
if [[ "$SIGNER" != "$IDENTITY" ]]; then
  echo "app is signed by '${SIGNER:-ad-hoc}', expected '$IDENTITY'" >&2
  exit 1
fi

mkdir -p "$(dirname "$OUTPUT")"
rm -f "$OUTPUT"
ditto -c -k --keepParent "$APP" "$OUTPUT"
echo "packaged $OUTPUT"
