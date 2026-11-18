#!/usr/bin/env bash
# Ensure a stable, self-signed code-signing identity for local builds and print
# the keychain path that holds it. Idempotent.
#
# Why: macOS ties Accessibility / Microphone / System Audio grants to the app's
# signature. Ad-hoc signatures change on every build, so each rebuild silently
# loses its grants. A self-signed certificate that never changes keeps them.
#
# The identity lives in its own keychain (~/Library/Keychains/zwisper-dev.keychain-db).
# It isn't added to the keychain search list and nothing is marked trusted;
# codesign is pointed at it with --keychain. Delete that file to start over.
set -euo pipefail

NAME="Zwisper Local Dev"
KC="$HOME/Library/Keychains/zwisper-dev.keychain-db"
# Not a secret: this keychain only holds a self-signed cert for local builds.
KC_PASS="zwisper-dev"

if [ ! -f "$KC" ]; then
  security create-keychain -p "$KC_PASS" "$KC" >&2
  security set-keychain-settings "$KC" >&2   # no auto-lock timeout
fi
security unlock-keychain -p "$KC_PASS" "$KC" >&2

if ! security find-certificate -c "$NAME" "$KC" >/dev/null 2>&1; then
  echo "Creating signing identity \"$NAME\" in $KC" >&2
  tmp=$(mktemp -d)
  trap 'rm -rf "$tmp"' EXIT
  cat > "$tmp/cert.cnf" <<EOF
[req]
distinguished_name = dn
prompt = no
[dn]
CN = $NAME
[ext]
basicConstraints = critical, CA:false
keyUsage = critical, digitalSignature
extendedKeyUsage = critical, codeSigning
EOF
  # /usr/bin/openssl is LibreSSL; its PKCS#12 output is what `security import` reads.
  /usr/bin/openssl req -x509 -newkey rsa:2048 -nodes -days 3650 \
    -config "$tmp/cert.cnf" -extensions ext \
    -keyout "$tmp/key.pem" -out "$tmp/cert.pem" 2>/dev/null
  /usr/bin/openssl pkcs12 -export -name "$NAME" -inkey "$tmp/key.pem" -in "$tmp/cert.pem" \
    -out "$tmp/id.p12" -passout pass:zwisper
  security import "$tmp/id.p12" -k "$KC" -P zwisper -T /usr/bin/codesign >&2
  # Let codesign use the key without a GUI prompt.
  security set-key-partition-list -S apple-tool:,apple: -s -k "$KC_PASS" "$KC" >/dev/null
fi

echo "$KC"
