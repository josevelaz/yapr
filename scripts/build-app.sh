#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
APP_NAME="Yapr"
BUNDLE_ID="dev.yapr"
DEST="${INSTALL_DIR:-$HOME/Applications}"
LOCAL_IDENTITY="Yapr Local Signing"
if [[ -n "${SIGN_IDENTITY:-}" ]]; then
  IDENTITY="$SIGN_IDENTITY"
elif security find-identity -p codesigning 2>/dev/null | grep -q "\"$LOCAL_IDENTITY\""; then
  IDENTITY="$LOCAL_IDENTITY"
else
  IDENTITY="-"
fi
VERSION="$(sed -n 's/^version = "\(.*\)"/\1/p' "$ROOT/app/Cargo.toml" | head -1)"

BUILD_ARGS=(--release --no-default-features --manifest-path "$ROOT/app/Cargo.toml")
if [[ "${1:-}" == "--e2e" && $# == 1 ]] || [[ $# == 0 && "${E2E:-0}" == 1 ]]; then
  BUILD_ARGS+=(--features e2e)
elif [[ $# != 0 ]]; then
  echo "usage: $0 [--e2e]" >&2
  exit 2
fi

echo "building $APP_NAME $VERSION"
cargo build "${BUILD_ARGS[@]}"

STAGE="$(mktemp -d)"
trap 'rm -rf "$STAGE"' EXIT
APP="$STAGE/$APP_NAME.app"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp "$ROOT/app/target/release/yapr" "$APP/Contents/MacOS/$APP_NAME"

ICONSET="$STAGE/AppIcon.iconset"
mkdir -p "$ICONSET"
for size in 16 32 128 256 512; do
  sips -z "$size" "$size" "$ROOT/assets/icon.png" --out "$ICONSET/icon_${size}x${size}.png" >/dev/null
  double=$((size * 2))
  sips -z "$double" "$double" "$ROOT/assets/icon.png" --out "$ICONSET/icon_${size}x${size}@2x.png" >/dev/null
done
iconutil -c icns "$ICONSET" -o "$APP/Contents/Resources/AppIcon.icns"

cat >"$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key><string>$APP_NAME</string>
  <key>CFBundleDisplayName</key><string>$APP_NAME</string>
  <key>CFBundleIdentifier</key><string>$BUNDLE_ID</string>
  <key>CFBundleExecutable</key><string>$APP_NAME</string>
  <key>CFBundleIconFile</key><string>AppIcon</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>$VERSION</string>
  <key>CFBundleVersion</key><string>$VERSION</string>
  <key>LSMinimumSystemVersion</key><string>14.0</string>
  <key>LSUIElement</key><true/>
  <key>NSMicrophoneUsageDescription</key>
  <string>Yapr listens while you dictate and turns your speech into text.</string>
  <key>NSScreenCaptureUsageDescription</key>
  <string>Yapr records the sound your Mac plays so it can remove it from your dictation.</string>
</dict>
</plist>
PLIST

TIMESTAMP="--timestamp=none"
[[ "$IDENTITY" == "Developer ID Application"* ]] && TIMESTAMP="--timestamp"
codesign --force --sign "$IDENTITY" --identifier "$BUNDLE_ID" --options runtime "$TIMESTAMP" \
  --entitlements "$ROOT/app/Yapr.entitlements" "$APP"
codesign --verify --strict "$APP"

pkill -x "$APP_NAME" 2>/dev/null && sleep 0.5 || true
mkdir -p "$DEST"
rm -rf "$DEST/$APP_NAME.app"
ditto "$APP" "$DEST/$APP_NAME.app"
echo "installed $DEST/$APP_NAME.app (signed: ${IDENTITY/#-/ad-hoc})"
