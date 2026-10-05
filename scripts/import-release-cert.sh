#!/usr/bin/env bash
set -euo pipefail

if [[ $# != 1 ]]; then
  echo "usage: $0 KEYCHAIN_PATH  (reads RELEASE_SIGNING_P12_BASE64 and RELEASE_SIGNING_P12_PASSWORD)" >&2
  exit 2
fi
KEYCHAIN="$1"
NAME="Yapr Release Signing"
: "${RELEASE_SIGNING_P12_BASE64:?RELEASE_SIGNING_P12_BASE64 is required}"
: "${RELEASE_SIGNING_P12_PASSWORD:?RELEASE_SIGNING_P12_PASSWORD is required}"

WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT
printf '%s' "$RELEASE_SIGNING_P12_BASE64" | base64 --decode >"$WORK/identity.p12"
KEYCHAIN_PASSWORD="$(openssl rand -hex 24)"

security create-keychain -p "$KEYCHAIN_PASSWORD" "$KEYCHAIN"
security set-keychain-settings -lut 3600 "$KEYCHAIN"
security unlock-keychain -p "$KEYCHAIN_PASSWORD" "$KEYCHAIN"
security import "$WORK/identity.p12" -k "$KEYCHAIN" -P "$RELEASE_SIGNING_P12_PASSWORD" -T /usr/bin/codesign
security set-key-partition-list -S apple-tool:,apple: -s -k "$KEYCHAIN_PASSWORD" "$KEYCHAIN" >/dev/null
read -r -a SEARCH <<<"$(security list-keychains -d user | tr -d '"' | tr '\n' ' ')"
security list-keychains -d user -s "$KEYCHAIN" "${SEARCH[@]}"

if ! security find-identity -p codesigning "$KEYCHAIN" | grep -qF "\"$NAME\""; then
  echo "no \"$NAME\" identity in the imported certificate" >&2
  exit 1
fi
echo "imported \"$NAME\" into $KEYCHAIN"
