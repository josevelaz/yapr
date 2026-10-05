#!/usr/bin/env bash
set -euo pipefail

NAME="Yapr Local Signing"
if security find-identity -p codesigning 2>/dev/null | grep -q "\"$NAME\""; then
  echo "\"$NAME\" already exists"
  exit 0
fi

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

openssl req -x509 -newkey rsa:2048 -nodes -days 3650 \
  -keyout "$WORK/key.pem" -out "$WORK/cert.pem" -config "$WORK/cert.cnf" 2>/dev/null
PASS="$(openssl rand -hex 16)"
openssl pkcs12 -export -inkey "$WORK/key.pem" -in "$WORK/cert.pem" -name "$NAME" \
  -out "$WORK/identity.p12" -passout "pass:$PASS"
security import "$WORK/identity.p12" -k "$HOME/Library/Keychains/login.keychain-db" \
  -P "$PASS"
echo "created \"$NAME\""
