# FERB.app's parts per architecture (L34), sourced by build-app-rust.sh (zsh). Nothing here builds, signs or downloads.

# _ferb_bytes PATH…: the total size in bytes of the regular files under each PATH (0 when there are none)
_ferb_bytes() {
  local p
  for p in "$@"; do
    if [[ -e $p ]]; then find "$p" -type f -print0; fi
  done | xargs -0 stat -f %z | awk '{ s += $1 } END { printf "%.0f\n", s }'
}

# ferb_arch_names ARCH: the names one architecture goes by (ARCH: arm64 or x86_64; anything else returns 1).
# FERB_NPM_CPU, FERB_CODEX_NPM: Codex's npm platform package.
# FERB_CODEX_TRIPLE: its binary's folder. FERB_CODEX_BIN: that binary in FERB.app, relative to Contents
# (rust/crates/ferb-runtimes/src/codex/mod.rs native_bin names the same file).
ferb_arch_names() {
  case ${1:-} in
    arm64)  FERB_NPM_CPU=arm64; FERB_CODEX_TRIPLE=aarch64-apple-darwin ;;
    x86_64) FERB_NPM_CPU=x64;   FERB_CODEX_TRIPLE=x86_64-apple-darwin ;;
    *) return 1 ;;
  esac
  FERB_CODEX_NPM="codex-darwin-$FERB_NPM_CPU"
  FERB_CODEX_BIN="Resources/node/node_modules/@openai/$FERB_CODEX_NPM/vendor/$FERB_CODEX_TRIPLE/bin/codex"
}

# ferb_check_arch APP ARCH: every Mach-O file in FERB.app is ARCH code only; otherwise names each one that isn't
ferb_check_arch() {
  local app=${1:A} arch=${2:?} f got
  local -a files bad
  files=("${(@f)$(find "$app/Contents" -type f \( -name '*.so' -o -name '*.dylib' -o -perm -u+x \) -print)}")
  for f in $files; do
    [[ -n $f && "$(file -b "$f")" == Mach-O* ]] || continue
    got=$(lipo -archs "$f" 2>/dev/null || true)
    [[ $got == $arch ]] || bad+=("${f#$app/}: ${got:-unreadable}")
  done
  (( ${#bad} == 0 )) && return 0
  print -u2 "Not $arch-only:"
  print -u2 -l -- "  "${^bad}
  return 1
}
