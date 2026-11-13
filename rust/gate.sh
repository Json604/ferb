#!/bin/sh
# The gate's Rust checks (kb/gate.toml). Usage: rust/gate.sh fmt | lint | test | portable
set -eu
[ -f "$HOME/.cargo/env" ] && . "$HOME/.cargo/env"
cd "$(dirname "$0")"
case "${1:-}" in
  fmt)  exec cargo fmt --all -- --check ;;
  lint) exec cargo clippy --workspace --all-targets --locked -- -D warnings ;;
  test) exec cargo test --workspace --locked ;;
  # The shared foundation builds for the phones: the engine crates, without the Mac platform crate or the store.
  portable)
    cargo check -p ferb-py -p ferb-core -p ferb-sanitize -p ferb-rpc -p ferb-identity -p ferb-context --target aarch64-apple-ios
    exec cargo check -p ferb-py -p ferb-core -p ferb-sanitize -p ferb-rpc -p ferb-identity -p ferb-context --target aarch64-linux-android ;;
  *) echo "usage: rust/gate.sh fmt|lint|test|portable" >&2; exit 2 ;;
esac
