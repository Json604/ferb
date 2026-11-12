#!/bin/sh
# The gate's Rust checks (kb/gate.toml). Usage: rust/gate.sh fmt | lint | test
set -eu
[ -f "$HOME/.cargo/env" ] && . "$HOME/.cargo/env"
cd "$(dirname "$0")"
case "${1:-}" in
  fmt)  exec cargo fmt --all -- --check ;;
  lint) exec cargo clippy --workspace --all-targets --locked -- -D warnings ;;
  test) exec cargo test --workspace --locked ;;
  *) echo "usage: rust/gate.sh fmt|lint|test" >&2; exit 2 ;;
esac
