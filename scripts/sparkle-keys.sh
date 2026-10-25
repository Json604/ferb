#!/bin/zsh -f
# FERB's update key (L34 Phase G), made once by the owner. Sparkle's generate_keys keeps the private EdDSA key in the
# login Keychain and prints the public key, which this writes to scripts/sparkle-public-key.txt for build-app.sh (a
# public key: commit that file). The private key never enters this repository.
#
#   scripts/sparkle-keys.sh                 make the key once (or read the existing one), write the public key file
#   scripts/sparkle-keys.sh --export FILE   back the private key up to FILE, outside this checkout; keep it offline
set -euo pipefail
ROOT="${0:A:h:h}"
source "$ROOT/scripts/swiftpm-anonymous.sh"      # SwiftPM fetches anonymously: never the owner's keychain
NOTCH="$ROOT/ui/mac/FERBNotch"
die() { print -u2 "sparkle-keys.sh: $*"; exit 1 }

OUT=""
if [[ "${1:-}" == --export ]]; then
  OUT="${${2:?usage: sparkle-keys.sh --export FILE}:A}"
  [[ "$OUT" != "$ROOT" && "$OUT" != "$ROOT"/* ]] || die "keep the private key outside this checkout ($ROOT)"
fi
swift package --package-path "$NOTCH" resolve >/dev/null
TOOL=$(find "$NOTCH/.build/artifacts" -path '*/Sparkle/bin/generate_keys' -type f 2>/dev/null | head -1)
[[ -n "$TOOL" && -x "$TOOL" ]] || die "Sparkle's generate_keys isn't under $NOTCH/.build/artifacts"
if [[ -n "$OUT" ]]; then
  "$TOOL" -x "$OUT"
  chmod 600 "$OUT"
  print "Private key written to $OUT. Keep it offline: it signs every FERB update."
  exit 0
fi
# generate_keys -p prints its "No existing signing key found" error on stdout, so only a base64 key counts as one.
is_key() { [[ "$1" =~ '^[A-Za-z0-9+/]{40,}={0,2}$' ]] }
PUBLIC=$("$TOOL" -p 2>/dev/null || true)
if ! is_key "$PUBLIC"; then
  "$TOOL" >/dev/null
  PUBLIC=$("$TOOL" -p 2>/dev/null || true)
fi
is_key "$PUBLIC" || die "Sparkle's generate_keys printed no public key: $PUBLIC"
print -r -- "$PUBLIC" > "$ROOT/scripts/sparkle-public-key.txt"
print "Public key: $PUBLIC (in scripts/sparkle-public-key.txt; commit it). Back up the private key with --export."
