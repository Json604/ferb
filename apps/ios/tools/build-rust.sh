#!/bin/zsh
# Builds the phone library (engine/crates/ferb-mobile-ffi) for the iPhone app (stage 4, kb PHN-446, PHN-523):
#   1. engine/patches/apply.sh (the patched iroh, netwatch and n0-dns-resolver the library builds against);
#   2. the library for aarch64-apple-ios (phones) and aarch64-apple-ios-sim (the simulator on Apple silicon), with the
#      crate's release profile (opt-level s, LTO, stripped), IPHONEOS_DEPLOYMENT_TARGET = the app's minimum;
#   3. the host library and the crate's own UniFFI generator (no Python), which write the Swift bindings
#      (FerbMobile.swift, FerbMobileFFI.h, the module map) from the host library's UniFFI metadata;
#   4. FerbMobileFFI.xcframework (the two static archives with the header and module map) and the generated Swift,
#      both into apps/ios/Packages/FerbMobile (git-ignored), where the FerbMobile Swift package links them;
#   5. engine/crates/ferb-link/tools/ios_api_check.py on the phone's own dylib: only APIs the iOS SDK offers (PHN-478).
# It does nothing when its inputs (every engine crate's sources and manifests, the lock files, the patches, the
# toolchain file and this script) are what the last successful run built from. All cargo output is in
# apps/ios/build/rust. Usage: apps/ios/tools/build-rust.sh [--force]
set -eu
HERE="${0:A:h}"
IOS="${HERE:h}"
ROOT="${IOS:h:h}"
ENGINE="$ROOT/engine"
CRATE="$ENGINE/crates/ferb-mobile-ffi"
OUT="$IOS/build/rust"
PKG="$IOS/Packages/FerbMobile"
GEN="$PKG/Sources/FerbMobile/Generated"
XCF="$PKG/FerbMobileFFI.xcframework"
# The app's minimum iOS (kb PHN-523); the Swift packages and the Xcode project say the same.
export IPHONEOS_DEPLOYMENT_TARGET=17.0
STAMP="$OUT/inputs.sha256"

[[ -f "$HOME/.cargo/env" ]] && . "$HOME/.cargo/env"
mkdir -p "$OUT"

inputs() {
  cd "$ROOT"
  { find engine/crates -type f \( -name '*.rs' -o -name 'Cargo.toml' -o -name 'Cargo.lock' -o -name 'uniffi.toml' \) \
      -not -path '*/target/*' -print; ls engine/patches/*; echo engine/rust-toolchain.toml; echo apps/ios/tools/build-rust.sh; } \
    | LC_ALL=C sort | while read -r f; do shasum -a 256 "$f"; done | shasum -a 256 | cut -d' ' -f1
}
WANT="$(inputs)-$IPHONEOS_DEPLOYMENT_TARGET"
if [[ "${1:-}" != "--force" && -f "$STAMP" && "$(cat "$STAMP")" == "$WANT" && -d "$XCF" && -f "$GEN/FerbMobile.swift" ]]; then
  echo "phone library: up to date ($XCF)"
  exit 0
fi

cd "$ENGINE"   # the pinned toolchain (engine/rust-toolchain.toml)
for t in aarch64-apple-ios aarch64-apple-ios-sim; do
  rustup target list --installed | grep -qx "$t" || { echo "Rust target $t is missing: run 'rustup target add $t' in engine/" >&2; exit 1; }
done

"$ENGINE/patches/apply.sh"
cd "$CRATE"   # cargo metadata finds uniffi.toml from here
export CARGO_TARGET_DIR="$OUT"
LOG="$OUT/cargo.log"
: > "$LOG"
cargo_step() {  # cargo's own output goes to the log (iroh's patched crates warn); shown when a step fails
  echo "phone library: $1"
  shift
  cargo "$@" >> "$LOG" 2>&1 || { tail -40 "$LOG" >&2; echo "phone library: cargo failed; the whole output is in $LOG" >&2; exit 1; }
}
for t in aarch64-apple-ios aarch64-apple-ios-sim; do
  cargo_step "building for $t (release)" build --release --lib --locked --target "$t"
done
cargo_step "the host library (its UniFFI metadata)" build --lib --locked
cargo_step "the bindings generator" build --locked --features bindgen --bin uniffi-bindgen

BIND="$OUT/swift-bindings"
rm -rf "$BIND"; mkdir -p "$BIND"
"$OUT/debug/uniffi-bindgen" generate --library "$OUT/debug/libferb_mobile_ffi.dylib" --language swift \
  --out-dir "$BIND" --no-format

echo "phone library: checking the iOS imports (PHN-478)"
python3 -I "$ENGINE/crates/ferb-link/tools/ios_api_check.py" "$OUT/aarch64-apple-ios/release/libferb_mobile_ffi.dylib" \
  --min-ios "$IPHONEOS_DEPLOYMENT_TARGET" > "$OUT/ios_api_check.txt" || { cat "$OUT/ios_api_check.txt" >&2; exit 1; }

HDR="$OUT/headers"
rm -rf "$HDR"; mkdir -p "$HDR"
cp "$BIND/FerbMobileFFI.h" "$HDR/"
cp "$BIND/FerbMobileFFI.modulemap" "$HDR/module.modulemap"
rm -rf "$XCF.tmp" "$XCF"
xcodebuild -create-xcframework \
  -library "$OUT/aarch64-apple-ios/release/libferb_mobile_ffi.a" -headers "$HDR" \
  -library "$OUT/aarch64-apple-ios-sim/release/libferb_mobile_ffi.a" -headers "$HDR" \
  -output "$XCF" >/dev/null
mkdir -p "$GEN"
cp "$BIND/FerbMobile.swift" "$GEN/FerbMobile.swift"
echo "$WANT" > "$STAMP"
echo "phone library: $XCF ($(du -sh "$XCF" | cut -f1)); bindings in $GEN"
