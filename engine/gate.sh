#!/bin/sh
# The gate's Rust checks (kb/gate.toml). Usage: engine/gate.sh fmt | lint | test | portable
set -eu
[ -f "$HOME/.cargo/env" ] && . "$HOME/.cargo/env"
cd "$(dirname "$0")"
# The phone crates sit outside the workspace (PHN-486: their patched iroh would bind the whole workspace's lock), so
# each is checked by its own manifest, after patches/apply.sh has made the pinned, patched copies.
PHONE="ferb-link ferb-mobile ferb-mobile-ffi"
# One build folder for the three, so iroh is compiled once.
PT="${CARGO_TARGET_DIR:-$PWD/target}/phone"
ndk() {
  NDK=$(ls -d "${ANDROID_HOME:-$HOME/Library/Android/sdk}"/ndk/* | sort | tail -1)/toolchains/llvm/prebuilt/darwin-x86_64/bin
  export CC_aarch64_linux_android="$NDK/aarch64-linux-android30-clang" AR_aarch64_linux_android="$NDK/llvm-ar" \
    RANLIB_aarch64_linux_android="$NDK/llvm-ranlib"
}
case "${1:-}" in
  fmt)
    for c in $PHONE; do cargo fmt --manifest-path "crates/$c/Cargo.toml" -- --check; done
    exec cargo fmt --all -- --check ;;
  lint)
    ./patches/apply.sh >/dev/null
    cargo clippy --manifest-path crates/ferb-link/Cargo.toml --target-dir "$PT" --all-targets --features devrelay --locked -- -D warnings -D clippy::significant_drop_in_scrutinee
    cargo clippy --manifest-path crates/ferb-mobile/Cargo.toml --target-dir "$PT" --all-targets --locked -- -D warnings -D clippy::significant_drop_in_scrutinee
    cargo clippy --manifest-path crates/ferb-mobile-ffi/Cargo.toml --target-dir "$PT" --all-targets --locked -- -D warnings -D clippy::significant_drop_in_scrutinee
    exec cargo clippy --workspace --all-targets --locked -- -D warnings -D clippy::significant_drop_in_scrutinee ;;
  # TEST-504 in Rust (T-1109): every test binary runs through ferb-test-guard (cargo's target runner), which runs it
  # in a sandbox that ends it on a connection beyond loopback, the Keychain, or a read or write in ~/.ferb, the Trash,
  # a model program's home or a browser's folders, and ends and names a test past FERB_TEST_TIMEOUT_S (300) or a
  # binary past FERB_TEST_MAX_RSS_MB (4096) (TEST-511). Doc tests don't go through a runner.
  test)
    cargo build -p ferb-test-guard --locked
    guard="$(cd "${CARGO_TARGET_DIR:-target}" && pwd)/debug/ferb-test-guard"
    host="$(rustc -vV | sed -n 's/^host: //p' | tr 'a-z-' 'A-Z_')"
    export "CARGO_TARGET_${host}_RUNNER=$guard"
    ./patches/apply.sh >/dev/null
    cargo test --manifest-path crates/ferb-link/Cargo.toml --target-dir "$PT" --locked -- --test-threads 3
    cargo test --manifest-path crates/ferb-mobile/Cargo.toml --target-dir "$PT" --locked
    cargo test --manifest-path crates/ferb-mobile-ffi/Cargo.toml --target-dir "$PT" --locked
    exec cargo test --workspace --locked ;;
  # The shared foundation builds for the phones: the engine crates, without the Mac platform crate or the store.
  # The phone library also may import nothing the iOS SDK marks unavailable (PHN-478: ios_api_check.py).
  portable)
    ./patches/apply.sh >/dev/null
    ndk
    for c in $PHONE; do
      cargo check --manifest-path "crates/$c/Cargo.toml" --target-dir "$PT" --locked --lib --target aarch64-apple-ios
      cargo check --manifest-path "crates/$c/Cargo.toml" --target-dir "$PT" --locked --lib --target aarch64-linux-android
    done
    # The app's minimum iOS (PHN-478: two Network calls need 13); without it the C code's link fails.
    IPHONEOS_DEPLOYMENT_TARGET=13.0 cargo build --manifest-path crates/ferb-mobile-ffi/Cargo.toml --target-dir "$PT" --locked --lib --target aarch64-apple-ios
    python3 -I crates/ferb-link/tools/ios_api_check.py \
      "$PT/aarch64-apple-ios/debug/libferb_mobile_ffi.dylib" --min-ios 13.0
    cargo check -p ferb-py -p ferb-core -p ferb-sanitize -p ferb-rpc -p ferb-identity -p ferb-context --target aarch64-apple-ios
    exec cargo check -p ferb-py -p ferb-core -p ferb-sanitize -p ferb-rpc -p ferb-identity -p ferb-context --target aarch64-linux-android ;;
  *) echo "usage: engine/gate.sh fmt|lint|test|portable" >&2; exit 2 ;;
esac
