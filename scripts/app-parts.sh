# FERB.app's parts per architecture (L34), sourced by build-app.sh (zsh). Nothing here builds, signs or downloads.

# _ferb_bytes PATH…: the total size in bytes of the regular files under each PATH (0 when there are none)
_ferb_bytes() {
  local p
  for p in "$@"; do
    if [[ -e $p ]]; then find "$p" -type f -print0; fi
  done | xargs -0 stat -f %z | awk '{ s += $1 } END { printf "%.0f\n", s }'
}

# ferb_size_report APP: FERB.app's size by part in MB (10^6 bytes), the largest first (L34's measurement)
ferb_size_report() {
  local app=${1:A}
  local c="$app/Contents" py="$app/Contents/Resources/python"
  local std="$py/lib/python3.12" site="$py/lib/python3.12/site-packages"
  local total codex claude site_all std_all libpy notch helpers arch
  total=$(_ferb_bytes "$app")
  codex=$(_ferb_bytes "$c/Resources/node")
  claude=$(_ferb_bytes "$site/claude_agent_sdk/_bundled")
  site_all=$(_ferb_bytes "$site")
  std_all=$(_ferb_bytes "$std")
  libpy=$(_ferb_bytes "$py/lib/libpython3.12.dylib")
  notch=$(_ferb_bytes "$c/MacOS")
  helpers=$(_ferb_bytes "$c/Helpers")
  arch=$(lipo -archs "$c/MacOS/FERBNotch" 2>/dev/null || print unknown)
  local -a rows
  rows=("$codex|Codex (Resources/node)"
        "$claude|Claude Code (claude_agent_sdk/_bundled)"
        "$(( site_all - claude ))|Python packages (site-packages without Claude Code)"
        "$(( std_all - site_all ))|Python standard library"
        "$libpy|libpython"
        "$notch|Notch app (MacOS)"
        "$helpers|Helpers (agent, voice, secrets)"
        "$(( total - codex - std_all - libpy - notch - helpers ))|Everything else")
  printf 'FERB.app (%s): %.1f MB\n' "$arch" $(( total / 1e6 ))
  local bytes name
  print -l -- $rows | sort -t'|' -k1,1 -rn | while IFS='|' read -r bytes name; do
    printf '%8.1f MB  %s\n' $(( bytes / 1e6 )) "$name"
  done
}

# ferb_arch_names ARCH: the names one architecture goes by (ARCH: arm64 or x86_64; anything else returns 1).
# FERB_UV_ARCH: uv's standalone CPython builds. FERB_NPM_CPU, FERB_CODEX_NPM: Codex's npm platform package.
# FERB_CODEX_TRIPLE: its binary's folder. FERB_CODEX_BIN: that binary in FERB.app, relative to Contents
# (ferb/runtimes/codex_exec.py native_bin names the same file).
ferb_arch_names() {
  case ${1:-} in
    arm64)  FERB_UV_ARCH=aarch64; FERB_NPM_CPU=arm64; FERB_CODEX_TRIPLE=aarch64-apple-darwin ;;
    x86_64) FERB_UV_ARCH=x86_64;  FERB_NPM_CPU=x64;   FERB_CODEX_TRIPLE=x86_64-apple-darwin ;;
    *) return 1 ;;
  esac
  FERB_CODEX_NPM="codex-darwin-$FERB_NPM_CPU"
  FERB_CODEX_BIN="Resources/node/node_modules/@openai/$FERB_CODEX_NPM/vendor/$FERB_CODEX_TRIPLE/bin/codex"
}

# ferb_trim_app APP ARCH: remove what FERB never runs, and keep one architecture in every universal Mach-O file.
# Run before signing (scripts/signing.sh signs what is left).
ferb_trim_app() {
  local app=${1:?} arch=${2:?}
  ferb_arch_names "$arch" || return 1
  local c="$app/Contents"
  local vendor="$c/Resources/node/node_modules/@openai/$FERB_CODEX_NPM/vendor/$FERB_CODEX_TRIPLE"
  local std="$c/Resources/python/lib/python3.12"
  local site="$std/site-packages"
  # Codex parts FERB turns off on every start (codex_exec.py DISABLED: code_mode_host and the shell tools) or never
  # starts (voice; its dylibs are re-signed by signing.sh, so OpenAI's library-validated voice host couldn't load them)
  rm -rf "${vendor:?}/bin/codex-code-mode-host" "${vendor:?}/codex-resources/voice" "${vendor:?}/codex-resources/zsh"
  # Python parts nothing imports: 2to3, pydoc's topic texts, the build configuration (python.o, Makefile), turtle,
  # test suites and benchmarks, the SDK's test helpers, and debug symbols
  rm -rf "${std:?}"/{lib2to3,pydoc_data,config-3.12-darwin,turtle.py} "${std:?}"/__pycache__/turtle.*(N) \
         "${site:?}/jsonschema/benchmarks" "${site:?}/claude_agent_sdk/testing"
  find "$site" \( -name '*.dSYM' -o \( -type d -name tests \) \) -prune -exec rm -rf {} +
  # One architecture per app
  local f
  local -a files
  files=("${(@f)$(find "$c" -type f \( -name '*.so' -o -name '*.dylib' -o -perm -u+x \) -print)}")
  for f in $files; do
    [[ -n $f && "$(file -b "$f")" == "Mach-O universal"* ]] || continue
    lipo "$f" -thin "$arch" -output "$f.thin" || { print -u2 "$f has no $arch code"; return 1 }
    mv "$f.thin" "$f"
  done
}
