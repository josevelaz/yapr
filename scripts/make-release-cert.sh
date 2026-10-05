#!/usr/bin/env bash
set -euo pipefail

NAME="Yapr Release Signing"
REPO="${REPO:-josevelaz/yapr}"
STORE="${YAPR_RELEASE_DIR:-$HOME/.config/yapr-release}"
P12="$STORE/release-signing.p12"
PASSWORD_FILE="$STORE/release-signing.password"
OPENSSL=/usr/bin/openssl

mkdir -p "$STORE"
chmod 700 "$STORE"

if [[ -f "$P12" ]]; then
  echo "reusing $P12"
else
  WORK="$(mktemp -d)"
  trap 'rm -rf "$WORK"' EXIT
  cat >"$WORK/cert.cnf" <<EOF
[req]
distinguished_name = dn
x509_extensions = ext
prompt = no
[dn]
CN = $NAME
[ext]
basicConstraints = critical,CA:false
keyUsage = critical,digitalSignature
extendedKeyUsage = critical,codeSigning
EOF
  "$OPENSSL" req -x509 -newkey rsa:2048 -nodes -days 7300 \
    -keyout "$WORK/key.pem" -out "$WORK/cert.pem" -config "$WORK/cert.cnf" 2>/dev/null
  "$OPENSSL" rand -hex 24 >"$PASSWORD_FILE"
  chmod 600 "$PASSWORD_FILE"
  "$OPENSSL" pkcs12 -export -inkey "$WORK/key.pem" -in "$WORK/cert.pem" -name "$NAME" \
    -out "$P12" -passout "file:$PASSWORD_FILE"
  chmod 600 "$P12"
  echo "created $P12"
fi

base64 -i "$P12" | gh secret set RELEASE_SIGNING_P12_BASE64 --repo "$REPO" --env release
gh secret set RELEASE_SIGNING_P12_PASSWORD --repo "$REPO" --env release --body "$(head -1 "$PASSWORD_FILE")"
echo "stored the certificate in the release environment of $REPO"
echo "back up $STORE: losing it means users must grant permissions again after the next update"
