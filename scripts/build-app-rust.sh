#!/bin/zsh -f
# Build FERB.app with the Rust engine (rust/PLAN.md part 13) and sign it once (scripts/signing.sh). No Python inside.
#
#   scripts/build-app-rust.sh [--arch arm64|x86_64] [OUT_DIR]
#                                           build OUT_DIR/FERB.app (default build/rust-app-<arch>) for one architecture
#                                           (default: this Mac's). A built app is signed again, e.g. with a Developer
#                                           ID, by `scripts/build-app.sh --sign-only APP` (the signing is shared).
#
# The app holds: the notch UI as its main executable (the same Swift build as build-app.sh); the agent app FERB
# (Helpers/FERB.app, bundle id com.ferb.daemon, ui/mac/FERBAgent/Info.plist): its executable is `ferbd`, so macOS names
# FERB in permission prompts and its grants attach to it as they did to the C host; `ferbd` runs there only as the
# daemon, the models' tool bridge and the grant probe (rust/crates/ferbd/src/host.rs); `ferb-native-host` beside it in
# Helpers; the `ferb` program (Resources/bin/ferb) for FERB.app's internal commands; the runtime Markdown the engine
# reads (Resources/ferb, byte for byte from ferb/); the extension; the pinned Codex (package-lock.json) and Claude
# Code (rust/packaging/package-lock.json); the pinned secret helper; the local embedding model; the background
# service's definition naming `FERB daemon`. The database migrations and the o200k tokenizer are compiled into ferbd.
# Pins (version, Codex, Claude Code, embedding model) come from the Rust sources (scripts/rust-parts.sh), not Python's.
# Env: SIGN_IDENTITY as for build-app.sh (default "-", ad hoc); FERB_NOTARIZE=0 with devid signs for this Mac only
# (scripts/signing.sh). Every Mach-O file in the bundle ends up signed.
# Never installs, registers or starts anything.
set -euo pipefail
ROOT="${0:A:h:h}"
source "$ROOT/scripts/swiftpm-anonymous.sh"      # SwiftPM fetches anonymously: never the owner's keychain
source "$ROOT/scripts/signing.sh"
source "$ROOT/scripts/app-parts.sh"
source "$ROOT/scripts/rust-parts.sh"

die() { print -u2 "build-app-rust.sh: $*"; exit 1 }

ARCH=$(uname -m)
if [[ "${1:-}" == --arch ]]; then
  (( $# >= 2 )) || die "usage: build-app-rust.sh [--arch arm64|x86_64] [OUT_DIR]"
  ARCH=$2
  shift 2
fi
ferb_rust_arch_names "$ARCH" || die "--arch is arm64 or x86_64, not '$ARCH'"
if [[ "$ARCH" != "$(uname -m)" ]]; then
  # Codex's and Claude Code's version checks run the Intel binaries under Rosetta
  [[ "$ARCH" == x86_64 ]] || die "an arm64 FERB.app is built on Apple silicon"
  arch -x86_64 /usr/bin/true 2>/dev/null || die "building the Intel app here needs Rosetta: softwareupdate --install-rosetta"
fi
OUT="${${1:-$ROOT/build/rust-app-$ARCH}:A}"
APP="$OUT/FERB.app"
WORK="$OUT/.work"                     # build cache (Swift, cargo, npm, the model); `rm -rf` it for a clean build
ferb_check_notary_credentials
[[ -f "$HOME/.cargo/env" ]] && source "$HOME/.cargo/env"
for tool in swift xcrun npm cargo rustup codesign ditto lipo curl shasum python3; do
  command -v $tool >/dev/null || die "needs $tool"
done
# rustup picks the toolchain by the working folder: rust/rust-toolchain.toml's pinned one, from rust/
rustup_here() { (cd "$ROOT/rust" && "$@") }
rustup_here rustup target list --installed 2>/dev/null | grep -qx "$FERB_RUST_TARGET" \
  || die "the Rust target $FERB_RUST_TARGET isn't installed: (cd rust && rustup target add $FERB_RUST_TARGET)"

ferb_rust_pins "$ROOT" || die "the Rust sources don't name FERB's pins (above)"
ferb_model_pin "$ROOT" || die "rust/crates/ferb-memory/src/embeddings.rs doesn't name the embedding model's files"
VERSION=$FERB_VERSION
SHORT_VERSION=${${VERSION%%[a-z]*}%.}
BUILD=$(date -u +%Y%m%d%H%M)                           # increases with every build; the service records it
REVISION=$(git -C "$ROOT" rev-parse --short HEAD 2>/dev/null || print unknown)
[[ -z "$(git -C "$ROOT" status --porcelain 2>/dev/null)" ]] || REVISION="$REVISION-dirty"

rm -rf "$APP"
mkdir -p "$WORK" "$APP/Contents/MacOS" "$APP/Contents/Resources/bin" "$APP/Contents/Helpers"
CONTENTS="$APP/Contents"
plist() { /usr/libexec/PlistBuddy -c "$1" "$2" }

print "==> Notch UI (main executable, $ARCH)"
NOTCH="$ROOT/ui/mac/FERBNotch"
SWIFT=(swift build --package-path "$NOTCH" -c release --arch "$ARCH" --scratch-path "$WORK/swift")
"${SWIFT[@]}" -Xswiftc -file-prefix-map -Xswiftc "$ROOT=."
NOTCH_BIN=$("${SWIFT[@]}" --show-bin-path)
cp "$NOTCH_BIN/FERBNotch" "$CONTENTS/MacOS/FERBNotch"
strip -S "$CONTENTS/MacOS/FERBNotch"
cp "$NOTCH/Info.plist" "$CONTENTS/Info.plist"
plist "Set :CFBundleShortVersionString $SHORT_VERSION" "$CONTENTS/Info.plist"
plist "Set :CFBundleVersion $BUILD" "$CONTENTS/Info.plist"
plist "Add :FERBVersion string $VERSION" "$CONTENTS/Info.plist"
plist "Add :FERBSourceRevision string $REVISION" "$CONTENTS/Info.plist"
cp "$NOTCH/ThirdPartyNotices.txt" "$CONTENTS/Resources/ThirdPartyNotices.txt"
NOTCH_RESOURCES="$NOTCH_BIN/FERBNotch_FERBNotch.bundle"
[[ -d "$NOTCH_RESOURCES/Contents/Resources/Sounds" ]] || die "swift build left no sound family in $NOTCH_RESOURCES"
ditto "$NOTCH_RESOURCES" "$CONTENTS/Resources/FERBNotch_FERBNotch.bundle"
diff -rq "$NOTCH/Sources/FERBNotch/Resources/Sounds" "$CONTENTS/Resources/FERBNotch_FERBNotch.bundle/Contents/Resources/Sounds" \
  || die "the app's sounds differ from ui/mac/FERBNotch/Sources/FERBNotch/Resources/Sounds (rm -rf $WORK/swift and build again)"
cp "$NOTCH/ferb-notification-banner.caf" "$CONTENTS/Resources/ferb-notification-banner.caf"
ICON="$ROOT/assets/logo/AppIcon.icns"
cp "$ICON" "$CONTENTS/Resources/AppIcon.icns"
cp "$ROOT/THIRD_PARTY_NOTICES.md" "$CONTENTS/Resources/THIRD_PARTY_NOTICES.md"
cp "$ROOT/LICENSE" "$CONTENTS/Resources/LICENSE"

print "==> Sparkle (automatic updates)"
[[ -d "$NOTCH_BIN/Sparkle.framework" ]] || die "swift build left no Sparkle.framework in $NOTCH_BIN"
mkdir -p "$CONTENTS/Frameworks"
ditto "$NOTCH_BIN/Sparkle.framework" "$CONTENTS/Frameworks/Sparkle.framework"
install_name_tool -add_rpath @executable_path/../Frameworks "$CONTENTS/MacOS/FERBNotch"
if [[ "${FERB_UPDATES:-on}" == off ]]; then
  print "    FERB_UPDATES=off: this build never checks for updates"
else
  KEY_FILE="$ROOT/scripts/sparkle-public-key.txt"
  UPDATES_KEY=""
  if [[ ! -s "$KEY_FILE" ]]; then
    [[ "$SIGN_IDENTITY" != (release|devid) ]] || die "no $KEY_FILE: the owner runs scripts/sparkle-keys.sh once (or build with FERB_UPDATES=off)"
    print "    no $KEY_FILE: this non-release build carries no feed and never checks for updates"
  else
    UPDATES_KEY="$(<"$KEY_FILE")"
  fi
  if [[ -n "$UPDATES_KEY" ]]; then
    plist "Add :SUFeedURL string https://github.com/Json604/ferb/releases/latest/download/appcast-$ARCH.xml" "$CONTENTS/Info.plist"
    plist "Add :SUPublicEDKey string $UPDATES_KEY" "$CONTENTS/Info.plist"
    plist "Add :SUEnableAutomaticChecks bool true" "$CONTENTS/Info.plist"
    plist "Add :SUAutomaticallyUpdate bool true" "$CONTENTS/Info.plist"
    plist "Add :SUScheduledCheckInterval integer 86400" "$CONTENTS/Info.plist"
    plist "Add :SUEnableSystemProfiling bool false" "$CONTENTS/Info.plist"
  fi
fi

print "==> ferbd, ferb and ferb-native-host $VERSION for $FERB_RUST_TARGET (release; Cargo.lock)"
# No build-machine path in what ships: the checkout, the cargo home and this build folder are remapped
CARGO_DIR=${CARGO_HOME:-$HOME/.cargo}
RUSTFLAGS="--remap-path-prefix=$ROOT=. --remap-path-prefix=$CARGO_DIR=/cargo --remap-path-prefix=$WORK=/build"
# The oldest macOS FERB.app runs on (the agent's LSMinimumSystemVersion, as the C host was built): without it an Intel
# build targets 10.12 and links Swift's back-deployment libraries by @rpath, which dyld can't find
MACOSX_DEPLOYMENT_TARGET=$(/usr/libexec/PlistBuddy -c 'Print :LSMinimumSystemVersion' "$ROOT/ui/mac/FERBAgent/Info.plist")
export RUSTFLAGS CARGO_TARGET_DIR="$WORK/cargo" MACOSX_DEPLOYMENT_TARGET
# ferbd is a workspace member: the workspace's Cargo.lock locks it and ferb-native-host alike
[[ -f "$ROOT/rust/Cargo.lock" ]] || die "rust/Cargo.lock is missing: a release builds only locked dependencies"
rustup_here cargo build --release --locked --target "$FERB_RUST_TARGET" \
  --manifest-path "$ROOT/rust/crates/ferbd/Cargo.toml" --bin ferbd --bin ferb
rustup_here cargo build --release --locked --target "$FERB_RUST_TARGET" --manifest-path "$ROOT/rust/Cargo.toml" \
  -p ferb-native-host
print "    $(rustup_here rustc --version)"
BUILT="$WORK/cargo/$FERB_RUST_TARGET/release"
for program in ferbd ferb ferb-native-host; do
  [[ "$(lipo -archs "$BUILT/$program")" == "$ARCH" ]] || die "cargo built no $ARCH $program"
  # only the system's libraries, by absolute path: nothing for dyld to look up through an @rpath
  ! otool -L "$BUILT/$program" | tail -n +2 | grep -q '@rpath' || die "$program links a library by @rpath"
done

print "==> FERB agent app (runs the daemon: ferbd as com.ferb.daemon's executable)"
AGENT="$CONTENTS/Helpers/FERB.app"
mkdir -p "$AGENT/Contents/MacOS" "$AGENT/Contents/Resources"
cp "$ROOT/ui/mac/FERBAgent/Info.plist" "$AGENT/Contents/Info.plist"     # the same id and usage descriptions
cp "$ICON" "$AGENT/Contents/Resources/AppIcon.icns"
plist "Set :CFBundleShortVersionString $SHORT_VERSION" "$AGENT/Contents/Info.plist"
plist "Set :CFBundleVersion $BUILD" "$AGENT/Contents/Info.plist"
cp "$BUILT/ferbd" "$CONTENTS/$FERB_RUST_AGENT"
cp "$BUILT/ferb-native-host" "$CONTENTS/$FERB_RUST_NATIVE_HOST"
cp "$BUILT/ferb" "$CONTENTS/$FERB_RUST_CLI"
chmod 755 "$CONTENTS/$FERB_RUST_AGENT" "$CONTENTS/$FERB_RUST_NATIVE_HOST" "$CONTENTS/$FERB_RUST_CLI"

print "==> Background service definition (FERB daemon; for SMAppService with a Developer ID, DMN-587)"
SERVICE="$CONTENTS/Library/LaunchAgents/com.ferb.daemon.plist"     # ferb-device install.rs BUNDLE_SERVICE
mkdir -p "${SERVICE:h}"
cp "$ROOT/ui/mac/FERBAgent/com.ferb.daemon.plist" "$SERVICE"
plist "Set :EnvironmentVariables:FERB_APP_BUILD $BUILD" "$SERVICE"
plist "Delete :ProgramArguments" "$SERVICE"
plist "Add :ProgramArguments array" "$SERVICE"
plist "Add :ProgramArguments:0 string FERB" "$SERVICE"
plist "Add :ProgramArguments:1 string daemon" "$SERVICE"                # ferb-device RUST_DAEMON_ARG
plutil -lint "$SERVICE" >/dev/null || die "the service definition isn't a valid plist"

print "==> Runtime Markdown (ferb/, byte for byte) and the extension"
PACKAGE="$CONTENTS/$FERB_RUST_PACKAGE"
MARKDOWN=(${(f)"$(ferb_rust_runtime_markdown "$ROOT")"})
(( ${#MARKDOWN} > 0 )) || die "no runtime Markdown under $ROOT/ferb"
for rel in $MARKDOWN; do
  mkdir -p "$PACKAGE/${rel:h}"
  cp "$ROOT/ferb/$rel" "$PACKAGE/$rel"
  cmp -s "$ROOT/ferb/$rel" "$PACKAGE/$rel" || die "ferb/$rel changed on its way into the app"
done
[[ -f "$PACKAGE/orchestrator/prompts/supervisor.md" && -f "$PACKAGE/memory/profile.md" ]] \
  || die "the runtime Markdown is incomplete in $PACKAGE"
EXT="$CONTENTS/$FERB_RUST_EXTENSION"
mkdir -p "${EXT:h}"
ditto "$ROOT/extensions/chromium" "$EXT"
find "$EXT" -name .DS_Store -delete
[[ -f "$EXT/manifest.json" && -f "$EXT/background.js" ]] || die "the extension is incomplete in $EXT"

print "==> Local embedding model ($FERB_MODEL_NAME @ ${FERB_MODEL_REVISION[1,12]}; SHA-256s from ferb-memory)"
MODEL_DIR="models/embeddings/$FERB_MODEL_NAME@${FERB_MODEL_REVISION[1,12]}"   # ferb-device EMBEDDING_MODEL_FOLDER
MODEL_CACHE="$WORK/$MODEL_DIR"
mkdir -p "$MODEL_CACHE"
for entry in $FERB_MODEL_FILES; do
  name=${entry%% *}
  want=${entry##* }
  if [[ ! -f "$MODEL_CACHE/$name" || "$(shasum -a 256 "$MODEL_CACHE/$name" | cut -d' ' -f1)" != "$want" ]]; then
    curl -fsSL --max-time 300 -o "$MODEL_CACHE/$name.part" \
      "https://huggingface.co/$FERB_MODEL_REPOSITORY/resolve/$FERB_MODEL_REVISION/$name" \
      || die "the embedding model's $name couldn't be fetched"
    mv "$MODEL_CACHE/$name.part" "$MODEL_CACHE/$name"
  fi
  [[ "$(shasum -a 256 "$MODEL_CACHE/$name" | cut -d' ' -f1)" == "$want" ]] || die "the embedding model's $name has the wrong SHA-256"
done
mkdir -p "$CONTENTS/Resources/${MODEL_DIR:h}"
ditto "$MODEL_CACHE" "$CONTENTS/Resources/$MODEL_DIR"

print "==> Codex $FERB_CODEX_VERSION for $ARCH (pinned in package-lock.json)"
mkdir -p "$WORK/node"
cp "$ROOT/package.json" "$ROOT/package-lock.json" "$WORK/node/"
(cd "$WORK/node" && npm ci --ignore-scripts --no-audit --no-fund --loglevel=error --os=darwin --cpu="$FERB_NPM_CPU")
CODEX_PKG="$WORK/node/node_modules/@openai/$FERB_CODEX_NPM"
[[ -d "$CODEX_PKG" ]] || die "npm installed no $FERB_CODEX_NPM"
got=$("$CODEX_PKG/vendor/$FERB_CODEX_TRIPLE/bin/codex" --version | awk '{ print $NF }')
[[ "$got" == "$FERB_CODEX_VERSION" ]] || die "package-lock.json has Codex $got, ferb-runtimes pins $FERB_CODEX_VERSION"
mkdir -p "$CONTENTS/Resources/node/node_modules/@openai"
ditto "$CODEX_PKG" "$CONTENTS/Resources/node/node_modules/@openai/$FERB_CODEX_NPM"

print "==> Claude Code $FERB_CLAUDE_VERSION for $ARCH (pinned in rust/packaging/package-lock.json)"
mkdir -p "$WORK/claude-node"
cp "$ROOT/rust/packaging/package.json" "$ROOT/rust/packaging/package-lock.json" "$WORK/claude-node/"
(cd "$WORK/claude-node" && npm ci --ignore-scripts --no-audit --no-fund --loglevel=error --os=darwin --cpu="$FERB_NPM_CPU")
CLAUDE_PKG="$WORK/claude-node/node_modules/@anthropic-ai/$FERB_CLAUDE_NPM"
[[ -x "$CLAUDE_PKG/claude" ]] || die "npm installed no $FERB_CLAUDE_NPM"
# ferbd checks the same at its first use of Claude (compose/runtimes.rs checked_claude): an empty environment
got=$(env -i PATH=/usr/bin:/bin "$CLAUDE_PKG/claude" --version | awk '{ print $1 }')
[[ "$got" == "$FERB_CLAUDE_VERSION" ]] || die "rust/packaging/package-lock.json has Claude Code $got, ferb-runtimes pins $FERB_CLAUDE_VERSION"
mkdir -p "$CONTENTS/Resources/node/node_modules/@anthropic-ai"
ditto "$CLAUDE_PKG" "$CONTENTS/Resources/node/node_modules/@anthropic-ai/$FERB_CLAUDE_NPM"

print "==> Secret helper (pinned, never compiled here: scripts/secret_helper.py)"
python3 -I "$ROOT/scripts/secret_helper.py" --check || die "the pinned secret helper doesn't match its source"
lipo -thin "$ARCH" -output "$CONTENTS/Helpers/ferb-secrets" "$ROOT/assets/secret-helper/ferb-secrets"
chmod 755 "$CONTENTS/Helpers/ferb-secrets"

print "==> Trimming what FERB never runs ($ARCH only)"
ferb_rust_trim "$APP" "$ARCH"
CHECK=$(mktemp -d "$WORK/codex-check.XXXXXX")
(cd "$CHECK" && HOME="$CHECK" CODEX_HOME="$CHECK" "$CONTENTS/$FERB_CODEX_BIN" debug models --bundled > models.json) \
  && plutil -extract models.0 json -o - "$CHECK/models.json" >/dev/null \
  || die "Codex doesn't start without its trimmed parts"
rm -rf "${CHECK:?}"
ferb_check_arch "$APP" "$ARCH" || die "FERB.app ($ARCH) holds code for another architecture"
LEFT=$(ferb_rust_python_left "$APP")
[[ -z "$LEFT" ]] || die "Python is left in the Rust build: ${(j:, :)${(f)LEFT}}"

print "==> Signing (identity: $SIGN_IDENTITY)"
ferb_sign_app "$APP" "$WORK/sign"
ferb_notarize_built "$APP" "$WORK"

print "Built: $APP ($(du -sh "$APP" | cut -f1); FERB $VERSION, build $BUILD, $REVISION; Rust engine)"
ferb_rust_size_report "$APP"
