# FERB.app's Rust build (engine/PLAN.md part 13): its pins, parts and checks, sourced by build-app-rust.sh (zsh) after
# app-parts.sh, and by release-appcast.sh for the release constants. Nothing here builds, signs or downloads.

# ferb_rust_const FILE NAME: the value of `pub const NAME: &str = "…";` in the Rust source FILE (exactly one), or
# status 1. The Rust build's pins come from the engine's own sources, the constants the programs enforce when they
# run.
ferb_rust_const() {
  local file=${1:?} name=${2:?}
  local -a found
  [[ -f $file ]] || return 1
  found=(${(f)"$(sed -n "s/^ *pub const $name: &str = \"\\([^\"]*\\)\";.*/\\1/p" "$file")"})
  (( ${#found} == 1 )) || return 1
  print -r -- "$found[1]"
}

# ferb_rust_pins ROOT: the release constants, read from the checkout ROOT's Rust sources:
#   FERB_VERSION          FERB's version          engine/crates/ferbd/src/compose/mod.rs VERSION
#   FERB_CODEX_VERSION    the pinned Codex        engine/crates/ferb-runtimes/src/codex/mod.rs CODEX_VERSION
#   FERB_CLAUDE_VERSION   the pinned Claude Code  engine/crates/ferb-runtimes/src/claude/transport.rs CLAUDE_CODE_VERSION
#   FERB_MIN_EXTENSION    the oldest extension    engine/crates/ferb-browser/src/protocol.rs MIN_EXTENSION_VERSION
#   FERB_STORE_EXTENSION  its Web Store id        engine/crates/ferb-browser/src/autoinstall.rs STORE_EXTENSION_ID
# Status 1, naming the missing one, when any can't be read.
ferb_rust_pins() {
  local root=${1:?} c=${1:?}/engine/crates
  FERB_VERSION=$(ferb_rust_const "$c/ferbd/src/compose/mod.rs" VERSION) \
    || { print -u2 "no VERSION in engine/crates/ferbd/src/compose/mod.rs"; return 1 }
  FERB_CODEX_VERSION=$(ferb_rust_const "$c/ferb-runtimes/src/codex/mod.rs" CODEX_VERSION) \
    || { print -u2 "no CODEX_VERSION in engine/crates/ferb-runtimes/src/codex/mod.rs"; return 1 }
  FERB_CLAUDE_VERSION=$(ferb_rust_const "$c/ferb-runtimes/src/claude/transport.rs" CLAUDE_CODE_VERSION) \
    || { print -u2 "no CLAUDE_CODE_VERSION in engine/crates/ferb-runtimes/src/claude/transport.rs"; return 1 }
  FERB_MIN_EXTENSION=$(ferb_rust_const "$c/ferb-browser/src/protocol.rs" MIN_EXTENSION_VERSION) \
    || { print -u2 "no MIN_EXTENSION_VERSION in engine/crates/ferb-browser/src/protocol.rs"; return 1 }
  FERB_STORE_EXTENSION=$(ferb_rust_const "$c/ferb-browser/src/autoinstall.rs" STORE_EXTENSION_ID) \
    || { print -u2 "no STORE_EXTENSION_ID in engine/crates/ferb-browser/src/autoinstall.rs"; return 1 }
}

# ferb_rust_arch_names ARCH: app-parts.sh's names for ARCH (Codex's) plus the Rust build's:
#   FERB_RUST_TARGET   rustc's target for the daemon programs
#   FERB_CLAUDE_NPM    Claude Code's npm platform package (engine/packaging/package-lock.json)
#   FERB_CLAUDE_BIN    its binary in FERB.app, relative to Contents (ferbd compose/runtimes.rs claude_bin)
ferb_rust_arch_names() {
  ferb_arch_names "${1:-}" || return 1
  case $1 in
    arm64)  FERB_RUST_TARGET=aarch64-apple-darwin ;;
    x86_64) FERB_RUST_TARGET=x86_64-apple-darwin ;;
  esac
  FERB_CLAUDE_NPM="claude-code-darwin-$FERB_NPM_CPU"
  FERB_CLAUDE_BIN="Resources/node/node_modules/@anthropic-ai/$FERB_CLAUDE_NPM/claude"
}

# The Rust build's places in FERB.app, relative to Contents (ferb-device install.rs BUNDLE_AGENT,
# BUNDLE_RUST_NATIVE_HOST, BUNDLE_CLI, BUNDLE_MODELS; ferbd home.rs package_folder; setup_env.rs Sources).
FERB_RUST_AGENT="Helpers/FERB.app/Contents/MacOS/FERB"           # ferbd: the nested com.ferb.daemon app's executable
FERB_RUST_NATIVE_HOST="Helpers/ferb-native-host"
FERB_RUST_LINK="Helpers/ferb-link"                               # the phone link's helper (ferb-rpc link_peer HELPER)
FERB_RUST_POWER="Helpers/ferb-power"                             # the root helper (APP-986; ferb-core power::helper)
FERB_RUST_POWER_SERVICE="Library/LaunchDaemons/com.ferb.power.plist"  # its launchd definition (SMAppService.daemon)
FERB_RUST_CLI="Resources/bin/ferb"                               # the internal commands' program
FERB_RUST_PACKAGE="Resources/ferb"                               # the runtime Markdown, byte for byte
FERB_RUST_EXTENSION="Resources/extensions/chromium"

# ferb_rust_runtime_markdown ROOT: the runtime Markdown the Rust engine reads from FERB's package folder (AGENTS.md:
# executable input), relative to ROOT/engine/runtime, one per line, sorted: the role and supervisor prompts, the memory
# prompts, the abilities' procedures. Python files never.
ferb_rust_runtime_markdown() {
  local f=${1:?}/engine/runtime
  {
    (cd "$f" && find orchestrator/prompts abilities -type f -name '*.md' -print)
    (cd "$f" && find memory -maxdepth 1 -type f -name '*.md' -print)
  } | LC_ALL=C sort
}

# ferb_model_pin ROOT: the embedding model the Rust engine expects (engine/crates/ferb-memory/src/embeddings.rs):
# FERB_MODEL_NAME, FERB_MODEL_REPOSITORY, FERB_MODEL_REVISION, and FERB_MODEL_FILES ("name sha256" per entry).
ferb_model_pin() {
  local src=${1:?}/engine/crates/ferb-memory/src/embeddings.rs
  FERB_MODEL_NAME=$(ferb_rust_const "$src" MODEL_NAME) || return 1
  FERB_MODEL_REPOSITORY=$(ferb_rust_const "$src" MODEL_REPOSITORY) || return 1
  FERB_MODEL_REVISION=$(ferb_rust_const "$src" MODEL_REVISION) || return 1
  [[ ${#FERB_MODEL_REVISION} == 40 && $FERB_MODEL_REVISION != *[^0-9a-f]* ]] || return 1
  # MODEL_FILES: [(&str, &str); N] = [ ("name", "sha256"), ... ];
  FERB_MODEL_FILES=(${(f)"$(awk '/pub const MODEL_FILES/ { on = 1 } on { print } on && /^];/ { exit }' "$src" \
    | tr -d ' \n' | grep -o '("[^"]*","[0-9a-f]\{64\}",\{0,1\})' | sed 's/("\([^"]*\)","\([0-9a-f]*\)",\{0,1\})/\1 \2/')"})
  (( ${#FERB_MODEL_FILES} > 0 ))
}

# ferb_rust_python_left APP: every file, folder or Mach-O link in APP that belongs to a Python runtime (a name with
# python in it, .py/.pyc/.pyo, site-packages, __pycache__, a library linking libpython), one per line; nothing when
# the app holds no Python.
ferb_rust_python_left() {
  local app=${1:A} f
  find "$app" \( -iname '*python*' -o -name '*.py' -o -name '*.pyc' -o -name '*.pyo' -o -name site-packages \
    -o -name __pycache__ \) -print | sed "s#^$app/##"
  for f in "${(@f)$(ferb_macho_files "$app")}"; do
    [[ -n $f ]] || continue
    otool -L "$f" 2>/dev/null | grep -qi 'libpython' && print -r -- "${f#$app/} (links libpython)"
  done
  return 0
}

# ferb_rust_trim APP ARCH: remove the Codex parts FERB never runs and keep one architecture in every universal Mach-O
# file. Run before signing. Codex's code_mode_host and shell tools are turned off on every start; voice is never
# started (its dylibs are re-signed by signing.sh, so OpenAI's library-validated voice host couldn't load them); zsh
# shipped up to Codex 0.157.1.
ferb_rust_trim() {
  local app=${1:?} arch=${2:?}
  ferb_rust_arch_names "$arch" || return 1
  local vendor="$app/Contents/Resources/node/node_modules/@openai/$FERB_CODEX_NPM/vendor/$FERB_CODEX_TRIPLE"
  rm -rf "${vendor:?}/bin/codex-code-mode-host" "${vendor:?}/codex-resources/voice" "${vendor:?}/codex-resources/zsh"
  local f
  for f in "${(@f)$(ferb_macho_files "$app/Contents")}"; do
    [[ -n $f && "$(file -b "$f")" == "Mach-O universal"* ]] || continue
    lipo "$f" -thin "$arch" -output "$f.thin" || { print -u2 "$f has no $arch code"; return 1 }
    mv "$f.thin" "$f"
  done
}

# ferb_rust_size_report APP: the Rust FERB.app's size by part in MB (10^6 bytes), the largest first
ferb_rust_size_report() {
  local app=${1:A}
  local c="$app/Contents" total claude codex agent cli helpers models notch sparkle package arch
  total=$(_ferb_bytes "$app")
  claude=$(_ferb_bytes "$c/Resources/node/node_modules/@anthropic-ai")
  codex=$(_ferb_bytes "$c/Resources/node/node_modules/@openai")
  agent=$(_ferb_bytes "$c/Helpers/FERB.app")
  cli=$(_ferb_bytes "$c/Resources/bin")
  helpers=$(( $(_ferb_bytes "$c/Helpers") - agent ))
  models=$(_ferb_bytes "$c/Resources/models")
  notch=$(_ferb_bytes "$c/MacOS")
  sparkle=$(_ferb_bytes "$c/Frameworks")
  package=$(_ferb_bytes "$c/Resources/ferb" "$c/Resources/extensions")
  arch=$(lipo -archs "$c/MacOS/FERBNotch" 2>/dev/null || print unknown)
  local -a rows
  rows=("$claude|Claude Code (Resources/node/@anthropic-ai)"
        "$codex|Codex (Resources/node/@openai)"
        "$agent|Daemon: ferbd as the agent app (Helpers/FERB.app)"
        "$cli|ferb program (Resources/bin)"
        "$helpers|Helpers (native host, secrets, phone link)"
        "$models|Embedding model (Resources/models)"
        "$notch|Notch app (MacOS)"
        "$sparkle|Sparkle (Frameworks)"
        "$package|Runtime Markdown and extension (Resources/ferb, extensions)"
        "$(( total - claude - codex - agent - cli - helpers - models - notch - sparkle - package ))|Everything else")
  printf 'FERB.app (%s, Rust): %.1f MB\n' "$arch" $(( total / 1e6 ))
  local bytes name
  print -l -- $rows | sort -t'|' -k1,1 -rn | while IFS='|' read -r bytes name; do
    printf '%8.1f MB  %s\n' $(( bytes / 1e6 )) "$name"
  done
}

# ferb_release_constants ROOT [ENGINE]: what release-appcast.sh checks a release against: FERB_STORE_EXTENSION and
# FERB_MIN_EXTENSION from the Rust sources (ferb_rust_pins). Only the Rust build is released: ENGINE, when given, is
# rust; anything else (python: an app carrying Contents/Resources/python) returns 1.
ferb_release_constants() {
  local root=${1:?}
  if [[ -n "${2:-}" && "$2" != rust ]]; then
    print -u2 "only FERB.app's Rust build is released, not a $2 one"
    return 1
  fi
  ferb_rust_pins "$root" || return 1
  [[ -n "$FERB_STORE_EXTENSION" && -n "$FERB_MIN_EXTENSION" ]]
}
