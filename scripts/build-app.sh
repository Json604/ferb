#!/bin/zsh -f
# Build the packaged FERB.app (L27) and sign it once (scripts/signing.sh).
#
#   scripts/build-app.sh [--arch arm64|x86_64] [OUT_DIR]
#                                           build OUT_DIR/FERB.app (default build/app-<arch>) for one architecture
#                                           (default: this Mac's; an Intel app builds on Apple silicon under Rosetta)
#   scripts/build-app.sh --sign-only APP    sign an assembled FERB.app again, e.g. with a Developer ID; no rebuild
#
# The app holds: the notch UI as its main executable; FERB's own Python (uv's relocatable standalone CPython, pinned
# below) with the ferb package and its locked dependencies; the agent app FERB (Helpers/FERB.app: a native stub that
# embeds that Python and runs the daemon, so macOS names FERB in permission prompts); the `ferb` launcher; the pinned
# Codex; the secret helper; the local embedding model; the background service's definition (Contents/Library/
# LaunchAgents: unused while FERB has no Developer ID, kept so FERB.app can unregister a 2026-11 build's SMAppService
# registration, and for T-27; first launch writes the user LaunchAgent instead, DMN-513). Hearing (an app's sound to text) is the main executable's own
# modes (ui/mac/FERBNotch/Sources/FERBNotch/Hearing.swift); there is no helper. First launch sets the daemon up from inside the app (ferb/device/install.py).
# Env: SIGN_IDENTITY (default "-", ad-hoc) and, for a Developer ID, the notary credentials described in signing.sh.
set -euo pipefail
ROOT="${0:A:h:h}"
source "$ROOT/scripts/swiftpm-anonymous.sh"      # SwiftPM fetches anonymously: never the owner's keychain
source "$ROOT/scripts/signing.sh"
source "$ROOT/scripts/app-parts.sh"

die() { print -u2 "build-app.sh: $*"; exit 1 }

if [[ "${1:-}" == --sign-only ]]; then
  APP="${${2:?usage: build-app.sh --sign-only APP}:A}"
  [[ -f "$APP/Contents/Info.plist" ]] || die "$APP isn't an app bundle"
  ferb_check_notary_credentials
  WORK="${APP:h}/.sign-work"
  trap 'rm -rf "$WORK"' EXIT
  ferb_sign_app "$APP" "$WORK"
  if ferb_developer_id; then ferb_notarize_app "$APP" "$WORK"; fi
  print "Signed: $APP (identity: $SIGN_IDENTITY)"
  exit 0
fi

ARCH=$(uname -m)                      # FERB.app's one architecture: arm64 (Apple silicon) or x86_64 (Intel)
if [[ "${1:-}" == --arch ]]; then
  (( $# >= 2 )) || die "usage: build-app.sh [--arch arm64|x86_64] [OUT_DIR]"
  ARCH=$2
  shift 2
fi
ferb_arch_names "$ARCH" || die "--arch is arm64 or x86_64, not '$ARCH'"
if [[ "$ARCH" != "$(uname -m)" ]]; then
  # The Intel app is built on Apple silicon by running its Intel Python and Codex under Rosetta
  [[ "$ARCH" == x86_64 ]] || die "an arm64 FERB.app is built on Apple silicon"
  arch -x86_64 /usr/bin/true 2>/dev/null || die "building the Intel app here needs Rosetta: softwareupdate --install-rosetta"
fi
OUT="${${1:-$ROOT/build/app-$ARCH}:A}"
APP="$OUT/FERB.app"
WORK="$OUT/.work"                     # build cache (Swift, Python download, npm); `rm -rf` it for a clean build
ferb_check_notary_credentials
for tool in uv swift xcrun npm codesign ditto install_name_tool lipo; do
  command -v $tool >/dev/null || die "needs $tool"
done

VERSION=$(sed -n 's/^__version__ = "\(.*\)"$/\1/p' "$ROOT/ferb/__init__.py")
SHORT_VERSION=${${VERSION%%[a-z]*}%.}                  # 0.27.0.dev1 -> 0.27.0
BUILD=$(date -u +%Y%m%d%H%M)                           # increases with every build; the LaunchAgent records it
REVISION=$(git -C "$ROOT" rev-parse --short HEAD 2>/dev/null || print unknown)
[[ -z "$(git -C "$ROOT" status --porcelain 2>/dev/null)" ]] || REVISION="$REVISION-dirty"
PY_MINOR=$(sed -n 's/^PYTHON_VERSION = "\(.*\)"$/\1/p' "$ROOT/ferb/device/install.py")
CPYTHON=3.12.13                       # the exact CPython every build ships (change it on purpose, then test the app)
[[ "$CPYTHON" == "$PY_MINOR".* ]] || die "CPython $CPYTHON isn't the $PY_MINOR that ferb/device/install.py names"
CODEX_VERSION=$(sed -n 's/^CODEX_VERSION = "\(.*\)"$/\1/p' "$ROOT/ferb/runtimes/codex_exec.py")

rm -rf "$APP"
mkdir -p "$WORK" "$APP/Contents/MacOS" "$APP/Contents/Resources/bin" "$APP/Contents/Helpers"
CONTENTS="$APP/Contents"
plist() { /usr/libexec/PlistBuddy -c "$1" "$2" }

print "==> Notch UI (main executable, $ARCH)"
NOTCH="$ROOT/ui/mac/FERBNotch"
# No build-machine paths in what ships: source paths remapped, debug symbols stripped
SWIFT=(swift build --package-path "$NOTCH" -c release --arch "$ARCH" --scratch-path "$WORK/swift")
"${SWIFT[@]}" -Xswiftc -file-prefix-map -Xswiftc "$ROOT=."
cp "$("${SWIFT[@]}" --show-bin-path)/FERBNotch" "$CONTENTS/MacOS/FERBNotch"
strip -S "$CONTENTS/MacOS/FERBNotch"
cp "$NOTCH/Info.plist" "$CONTENTS/Info.plist"
plist "Set :CFBundleShortVersionString $SHORT_VERSION" "$CONTENTS/Info.plist"
plist "Set :CFBundleVersion $BUILD" "$CONTENTS/Info.plist"
plist "Add :FERBVersion string $VERSION" "$CONTENTS/Info.plist"
plist "Add :FERBSourceRevision string $REVISION" "$CONTENTS/Info.plist"
cp "$NOTCH/ThirdPartyNotices.txt" "$CONTENTS/Resources/ThirdPartyNotices.txt"
# The notch's SwiftPM resources (FERB's mark, the sound family): Bundle.module finds them in Contents/Resources
NOTCH_RESOURCES="$("${SWIFT[@]}" --show-bin-path)/FERBNotch_FERBNotch.bundle"
[[ -d "$NOTCH_RESOURCES/Contents/Resources/Sounds" ]] || die "swift build left no sound family in $NOTCH_RESOURCES"
ditto "$NOTCH_RESOURCES" "$CONTENTS/Resources/FERBNotch_FERBNotch.bundle"
# An incremental build can keep a stale copy (an older family beside the new files): what ships must equal the
# committed sounds (ALAC .caf and their CREDITS.md) exactly
diff -rq "$NOTCH/Sources/FERBNotch/Resources/Sounds" "$CONTENTS/Resources/FERBNotch_FERBNotch.bundle/Contents/Resources/Sounds" \
  || die "the app's sounds differ from ui/mac/FERBNotch/Sources/FERBNotch/Resources/Sounds (rm -rf $WORK/swift and build again)"
# The banners' notification sound, only here: macOS plays a banner's sound only from the app's own Resources (or
# ~/Library/Sounds); the notch's in-app cue plays its own dry file from the Sounds bundle
cp "$NOTCH/ferb-notification-banner.caf" "$CONTENTS/Resources/ferb-notification-banner.caf"
[[ -f "$CONTENTS/Resources/ferb-notification-banner.caf" ]] || die "FERB.app has no banner sound in Contents/Resources"
ICON="$ROOT/assets/logo/AppIcon.icns"     # FERB's logo (scripts/make_logo.py makes it from assets/logo/ferb-logo.svg)
cp "$ICON" "$CONTENTS/Resources/AppIcon.icns"                     # Info.plist CFBundleIconFile
cp "$ROOT/THIRD_PARTY_NOTICES.md" "$CONTENTS/Resources/THIRD_PARTY_NOTICES.md"

print "==> Sparkle (automatic updates, L34 Phase G)"
NOTCH_BIN=$("${SWIFT[@]}" --show-bin-path)           # plan A's swift build for $ARCH
[[ -d "$NOTCH_BIN/Sparkle.framework" ]] || die "swift build left no Sparkle.framework in $NOTCH_BIN"
mkdir -p "$CONTENTS/Frameworks"
ditto "$NOTCH_BIN/Sparkle.framework" "$CONTENTS/Frameworks/Sparkle.framework"
install_name_tool -add_rpath @executable_path/../Frameworks "$CONTENTS/MacOS/FERBNotch"
if [[ "${FERB_UPDATES:-on}" == off ]]; then
  print "    FERB_UPDATES=off: this build never checks for updates"
else
  KEY_FILE="$ROOT/scripts/sparkle-public-key.txt"
  if [[ ! -s "$KEY_FILE" ]]; then
    [[ "$SIGN_IDENTITY" != release ]] || die "no $KEY_FILE: the owner runs scripts/sparkle-keys.sh once (or build with FERB_UPDATES=off)"
    print "    no $KEY_FILE: this non-release build carries no feed and never checks for updates"
    UPDATES_KEY=""
  else
    UPDATES_KEY="$(<"$KEY_FILE")"
  fi
  if [[ -n "${UPDATES_KEY:-}" ]]; then
  # One feed per architecture; only GitHub's public release list is asked; no system profile is sent (L34)
  plist "Add :SUFeedURL string https://github.com/Json604/ferb/releases/latest/download/appcast-$ARCH.xml" "$CONTENTS/Info.plist"
  plist "Add :SUPublicEDKey string $UPDATES_KEY" "$CONTENTS/Info.plist"
  plist "Add :SUEnableAutomaticChecks bool true" "$CONTENTS/Info.plist"
  plist "Add :SUAutomaticallyUpdate bool true" "$CONTENTS/Info.plist"
  plist "Add :SUScheduledCheckInterval integer 86400" "$CONTENTS/Info.plist"
  plist "Add :SUEnableSystemProfiling bool false" "$CONTENTS/Info.plist"
  fi
fi

print "==> Python $CPYTHON for $ARCH (uv's standalone CPython; relocatable)"
PY_KEY="cpython-$CPYTHON-macos-$FERB_UV_ARCH-none"
uv python install "$PY_KEY" --install-dir "$WORK/python" --no-bin --quiet
PY_SRC="$WORK/python/$PY_KEY"
[[ -x "$PY_SRC/bin/python$PY_MINOR" ]] || die "uv installed no CPython $CPYTHON for $ARCH under $WORK/python"
PYHOME="$CONTENTS/Resources/python"
PY="$PYHOME/bin/python$PY_MINOR"
ditto "$PY_SRC" "$PYHOME"
[[ "$("$PY" -I -c 'import platform; print(platform.python_version(), platform.machine())')" == "$CPYTHON $ARCH" ]] \
  || die "not CPython $CPYTHON for $ARCH"
rm -f "$PYHOME/lib/python$PY_MINOR/EXTERNALLY-MANAGED"     # the app's Python is sealed, not uv-managed
# uv recorded its install path as libpython's id; the interpreter finds it through @executable_path/../lib
install_name_tool -id "@rpath/libpython$PY_MINOR.dylib" "$PYHOME/lib/libpython$PY_MINOR.dylib" 2>/dev/null
codesign --force --sign - "$PYHOME/lib/libpython$PY_MINOR.dylib"   # changed code must be re-signed to load
STDLIB="$PYHOME/lib/python$PY_MINOR"
# Parts FERB never uses: Tk, IDLE, the test suite, ensurepip and pip (the bundle is never pip-installed into).
rm -rf "$STDLIB"/{test,idlelib,tkinter,turtledemo,ensurepip} "$STDLIB"/lib-dynload/_tkinter*.so \
       "$PYHOME"/lib/{libtcl,libtk,tcl,tk,itcl,thread}*(N) "$PYHOME"/bin/{idle,pydoc,pip,2to3}*(N)

print "==> ferb and its locked dependencies (uv.lock)"
(cd "$ROOT" && uv export --frozen --no-dev --no-emit-project --format requirements-txt --quiet \
   -o "$WORK/requirements.txt")
uv pip install --python "$PY" --require-hashes --quiet -r "$WORK/requirements.txt"
uv pip install --python "$PY" --no-deps --reinstall-package ferb --quiet "$ROOT"
rm -rf "$STDLIB"/site-packages/{pip,PyObjCTest} "$STDLIB"/site-packages/pip-*.dist-info(N)
rm -f "$STDLIB"/site-packages/*.dist-info/direct_url.json(N)      # it names this checkout's folder
# Console scripts point at this build folder, and headers are for compiling: FERB runs `python -m` only.
find "$PYHOME/bin" -mindepth 1 \( ! -name 'python*' -o -name '*-config' \) -delete
rm -rf "$PYHOME"/{include,share} "$PYHOME"/lib/pkgconfig
# Compiled once here and never checked against the sources, so FERB never writes into its own signed bundle.
# uv wrote its install folder (this build's) into the build configuration; put back the standalone build's own
# placeholder, so no build-machine path ships (it is only read when compiling extensions, which FERB never does).
"$PY" -I -c 'import pathlib, sys
for name in sys.argv[2:]:
    f = pathlib.Path(name)
    f.write_text(f.read_text().replace(sys.argv[1], "/install"))' "$PY_SRC" "$STDLIB"/_sysconfigdata_*.py(N)
# Recorded source paths start at FERB.app, not this build folder (Python shows the real path once imported).
"$PY" -I -m compileall -q -f -j 0 --invalidation-mode unchecked-hash -s "$APP" -p FERB.app "$STDLIB" >/dev/null \
  || die "compiling the bundled Python sources failed"

print "==> FERB agent app (runs the daemon; embeds the app's Python)"
AGENT="$CONTENTS/Helpers/FERB.app"
mkdir -p "$AGENT/Contents/MacOS" "$AGENT/Contents/Resources"
cp "$ROOT/ui/mac/FERBAgent/Info.plist" "$AGENT/Contents/Info.plist"
cp "$ICON" "$AGENT/Contents/Resources/AppIcon.icns"   # macOS shows it for "FERB" in Privacy lists and Activity Monitor
plist "Set :CFBundleShortVersionString $SHORT_VERSION" "$AGENT/Contents/Info.plist"
plist "Set :CFBundleVersion $BUILD" "$AGENT/Contents/Info.plist"
xcrun clang -O2 -arch "$ARCH" -mmacosx-version-min=14.2 -ffile-prefix-map="$ROOT=." \
  -I"$PY_SRC/include/python$PY_MINOR" "$ROOT/ui/mac/FERBAgent/main.c" "$PYHOME/lib/libpython$PY_MINOR.dylib" \
  -Wl,-rpath,@executable_path/../../../../Resources/python/lib -o "$AGENT/Contents/MacOS/FERB"
strip -S "$AGENT/Contents/MacOS/FERB"

print "==> Background service definition (kept for SMAppService's unregister and T-27; the daemon runs as a user LaunchAgent, DMN-513)"
SERVICE="$CONTENTS/Library/LaunchAgents/com.ferb.daemon.plist"     # ferb/device/install.py BUNDLE_SERVICE
mkdir -p "${SERVICE:h}"
cp "$ROOT/ui/mac/FERBAgent/com.ferb.daemon.plist" "$SERVICE"
plist "Set :EnvironmentVariables:FERB_APP_BUILD $BUILD" "$SERVICE"
plutil -lint "$SERVICE" >/dev/null || die "the service definition isn't a valid plist"

print "==> Local embedding model (potion-base-8M, MIT; pinned revision and SHA-256s: ferb/memory/embeddings.py)"
# Fetched once into the build cache with its hashes checked, then copied in, so a new install needs no download
"$PY" -I -B -c 'import sys; from pathlib import Path; from ferb.memory.embeddings import default_root, download
download(default_root(Path(sys.argv[1])))' "$WORK" || die "the embedding model couldn't be fetched"
mkdir -p "$CONTENTS/Resources/models"                               # ferb/device/install.py BUNDLE_MODELS
ditto "$WORK/models/embeddings" "$CONTENTS/Resources/models/embeddings"

print "==> ferb command"
cat > "$CONTENTS/Resources/bin/ferb" <<'EOF'
#!/bin/zsh -f
# FERB's command line, run by FERB.app's own Python. A link on PATH may point here (:A follows it).
exec "${0:A:h:h}/python/bin/python3.12" -I -B -m ferb.cli "$@"
EOF
chmod 755 "$CONTENTS/Resources/bin/ferb"

print "==> Codex $CODEX_VERSION for $ARCH (pinned in package-lock.json)"
mkdir -p "$WORK/node"
cp "$ROOT/package.json" "$ROOT/package-lock.json" "$WORK/node/"
(cd "$WORK/node" && npm ci --ignore-scripts --no-audit --no-fund --loglevel=error --os=darwin --cpu="$FERB_NPM_CPU")
CODEX_PKG="$WORK/node/node_modules/@openai/$FERB_CODEX_NPM"
[[ -d "$CODEX_PKG" ]] || die "npm installed no $FERB_CODEX_NPM"
got=$("$CODEX_PKG/vendor/$FERB_CODEX_TRIPLE/bin/codex" --version | awk '{ print $NF }')
[[ "$got" == "$CODEX_VERSION" ]] || die "package-lock.json has Codex $got, ferb pins $CODEX_VERSION"
mkdir -p "$CONTENTS/Resources/node/node_modules/@openai"
ditto "$CODEX_PKG" "$CONTENTS/Resources/node/node_modules/@openai/$FERB_CODEX_NPM"

print "==> Secret helper (pinned, never compiled here: scripts/secret_helper.py)"
# The Keychain admits FERB's team-less helper to each saved secret by its exact code hash, so its bytes never change
# between releases unless its code does (REL-404)
python3 -I "$ROOT/scripts/secret_helper.py" --check || die "the pinned secret helper doesn't match its source"
lipo -thin "$ARCH" -output "$CONTENTS/Helpers/ferb-secrets" "$ROOT/assets/secret-helper/ferb-secrets"
chmod 755 "$CONTENTS/Helpers/ferb-secrets"

print "==> Trimming what FERB never runs ($ARCH only)"
ferb_trim_app "$APP" "$ARCH"
# The trimmed Codex must still start: it prints its own model catalog offline, from an empty home (codex_exec.py
# bundled_catalog does the same at the daemon's start)
CHECK=$(mktemp -d "$WORK/codex-check.XXXXXX")
(cd "$CHECK" && HOME="$CHECK" CODEX_HOME="$CHECK" "$CONTENTS/$FERB_CODEX_BIN" debug models --bundled > models.json) \
  && "$PY" -I -c 'import json, sys; assert json.load(open(sys.argv[1]))["models"]' "$CHECK/models.json" \
  || die "Codex doesn't start without its trimmed parts"
rm -rf "${CHECK:?}"
ferb_check_arch "$APP" "$ARCH" || die "FERB.app ($ARCH) holds code for another architecture"

print "==> Signing (identity: $SIGN_IDENTITY)"
ferb_sign_app "$APP" "$WORK/sign"
if ferb_developer_id; then ferb_notarize_app "$APP" "$WORK"; fi

print "Built: $APP ($(du -sh "$APP" | cut -f1); FERB $VERSION, build $BUILD, $REVISION)"
ferb_size_report "$APP"
