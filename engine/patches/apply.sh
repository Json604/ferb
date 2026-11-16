#!/bin/sh
# Makes the patched copies of published crates that ferb-link builds against (README.md here: each patch and why).
# For each line of the table below: the crate exactly as published on crates.io (its SHA-256 checked against the one
# crates.io publishes, the `checksum` cargo records in a Cargo.lock), unpacked into engine/target/patched/<name>-<ver>-ferb
# (ignored by git, like all build output), plus engine/patches/<name>-<ver>.patch applied with `patch -p1`.
# ferb-link's Cargo.toml points its `[patch.crates-io]` at those copies: run this once before any cargo command on
# ferb-link. It does nothing for a copy already made from the same crate and patch. The crate comes from cargo's
# download cache when it is there, otherwise from crates.io (the only network it uses).
# Ported from the phone lab's scripts/iroh-patch.sh (ferb-phone-link@57b3f34), which made the iroh copy alone.
set -eu
HERE="$(cd "$(dirname "$0")" && pwd)"
ENGINE="$(cd "$HERE/.." && pwd)"
OUTDIR="$ENGINE/target/patched"
mkdir -p "$OUTDIR"
while read -r NAME VERSION SHA; do
  [ -n "$NAME" ] || continue
  PATCH="$HERE/$NAME-$VERSION.patch"
  OUT="$OUTDIR/$NAME-$VERSION-ferb"
  WANT="$SHA $(shasum -a 256 "$PATCH" | cut -d' ' -f1)"
  [ -f "$OUT/.ferb-patch" ] && [ "$(cat "$OUT/.ferb-patch")" = "$WANT" ] && continue
  CRATE=$(ls "${CARGO_HOME:-$HOME/.cargo}"/registry/cache/*/"$NAME-$VERSION.crate" 2>/dev/null | head -1 || true)
  if [ -z "$CRATE" ]; then
    CRATE="$OUTDIR/$NAME-$VERSION.crate"
    [ -f "$CRATE" ] || curl -fsSL -o "$CRATE" "https://static.crates.io/crates/$NAME/$NAME-$VERSION.crate"
  fi
  [ "$(shasum -a 256 "$CRATE" | cut -d' ' -f1)" = "$SHA" ] || { echo "$NAME-$VERSION.crate is not the published one (SHA-256 differs): $CRATE" >&2; exit 1; }
  TMP="$OUT.tmp.$$"
  rm -rf "$TMP"; mkdir -p "$TMP"
  tar -xzf "$CRATE" -C "$TMP" --strip-components 1
  (cd "$TMP" && patch -p1 -s --no-backup-if-mismatch < "$PATCH") || { echo "engine/patches/$NAME-$VERSION.patch did not apply" >&2; rm -rf "$TMP"; exit 1; }
  echo "$WANT" > "$TMP/.ferb-patch"
  rm -rf "$OUT"; mv "$TMP" "$OUT"
  echo "$NAME $VERSION with engine/patches/$NAME-$VERSION.patch is in $OUT"
done <<TABLE
iroh 1.4.1 e3689cab508af9bcef7816c1201b403595ec6aa20e7d15e1f9683bb4be4a5b17
n0-dns-resolver 0.1.0 535c99c9a786bee714155d2ee4fc2477405c2c43536e1bc6a51de09a0d1448f7
netwatch 0.19.3 39da9cad5f23aa43401f09497d3b280e51b5feba115d9ffbf38ca35d6ff95e96
TABLE
