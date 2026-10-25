#!/bin/zsh -f
# Build the packaged FERB.app (L27) and sign it once (scripts/signing.sh).
#
#   scripts/build-app.sh [OUT_DIR]          build OUT_DIR/FERB.app (default build/app), then sign it
#   scripts/build-app.sh --sign-only APP    sign an assembled FERB.app again, e.g. with a Developer ID; no rebuild
#
# The app holds: the notch UI as its main executable; FERB's own Python (uv's relocatable standalone CPython, pinned
# below) with the ferb package and its locked dependencies; the agent app FERB (Helpers/FERB.app: a native stub that
# embeds that Python and runs the daemon, so macOS names FERB in permission prompts); the `ferb` launcher; the pinned
# Codex; the secret helper; and the voice helper built from macapp/FERB (nested until it merges into the main app).
# First launch installs the daemon from inside the app (ferb/device/install.py). Env: SIGN_IDENTITY (default "-",
# ad-hoc) and, for a Developer ID, the notary credentials described in signing.sh.
set -euo pipefail
ROOT="${0:A:h:h}"
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

OUT="${${1:-$ROOT/build/app}:A}"
APP="$OUT/FERB.app"
WORK="$OUT/.work"                     # build cache (Swift, Python download, npm); `rm -rf` it for a clean build
ferb_check_notary_credentials
[[ "$(uname -m)" == arm64 ]] || die "FERB.app is built for Apple silicon (arm64)"
for tool in uv swift xcrun npm codesign ditto install_name_tool; do
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

print "==> Notch UI (main executable)"
NOTCH="$ROOT/ui/mac/FERBNotch"
# No build-machine paths in what ships: source paths remapped, debug symbols stripped
swift build --package-path "$NOTCH" -c release --scratch-path "$WORK/swift" -Xswiftc -file-prefix-map -Xswiftc "$ROOT=."
cp "$(swift build --package-path "$NOTCH" -c release --scratch-path "$WORK/swift" --show-bin-path)/FERBNotch" \
   "$CONTENTS/MacOS/FERBNotch"
strip -S "$CONTENTS/MacOS/FERBNotch"
cp "$NOTCH/Info.plist" "$CONTENTS/Info.plist"
plist "Set :CFBundleShortVersionString $SHORT_VERSION" "$CONTENTS/Info.plist"
plist "Set :CFBundleVersion $BUILD" "$CONTENTS/Info.plist"
plist "Add :FERBVersion string $VERSION" "$CONTENTS/Info.plist"
plist "Add :FERBSourceRevision string $REVISION" "$CONTENTS/Info.plist"
cp "$NOTCH/ThirdPartyNotices.txt" "$CONTENTS/Resources/ThirdPartyNotices.txt"
cp "$ROOT/THIRD_PARTY_NOTICES.md" "$CONTENTS/Resources/THIRD_PARTY_NOTICES.md"

print "==> Python $CPYTHON (uv's standalone CPython; relocatable)"
uv python install "$CPYTHON" --install-dir "$WORK/python" --no-bin --quiet
PY_SRC="$WORK/python/cpython-$CPYTHON-macos-aarch64-none"
[[ -x "$PY_SRC/bin/python$PY_MINOR" ]] || die "uv installed no CPython $CPYTHON under $WORK/python"
PYHOME="$CONTENTS/Resources/python"
PY="$PYHOME/bin/python$PY_MINOR"
ditto "$PY_SRC" "$PYHOME"
[[ "$("$PY" -I -c 'import platform; print(platform.python_version())')" == "$CPYTHON" ]] || die "not CPython $CPYTHON"
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
mkdir -p "$AGENT/Contents/MacOS"
cp "$ROOT/ui/mac/FERBAgent/Info.plist" "$AGENT/Contents/Info.plist"
plist "Set :CFBundleShortVersionString $SHORT_VERSION" "$AGENT/Contents/Info.plist"
plist "Set :CFBundleVersion $BUILD" "$AGENT/Contents/Info.plist"
xcrun clang -O2 -arch arm64 -mmacosx-version-min=14.2 -ffile-prefix-map="$ROOT=." \
  -I"$PY_SRC/include/python$PY_MINOR" "$ROOT/ui/mac/FERBAgent/main.c" "$PYHOME/lib/libpython$PY_MINOR.dylib" \
  -Wl,-rpath,@executable_path/../../../../Resources/python/lib -o "$AGENT/Contents/MacOS/FERB"
strip -S "$AGENT/Contents/MacOS/FERB"

print "==> ferb command"
cat > "$CONTENTS/Resources/bin/ferb" <<'EOF'
#!/bin/zsh -f
# FERB's command line, run by FERB.app's own Python. A link on PATH may point here (:A follows it).
exec "${0:A:h:h}/python/bin/python3.12" -I -B -m ferb.cli "$@"
EOF
chmod 755 "$CONTENTS/Resources/bin/ferb"

print "==> Codex $CODEX_VERSION (pinned in package-lock.json)"
mkdir -p "$WORK/node"
cp "$ROOT/package.json" "$ROOT/package-lock.json" "$WORK/node/"
(cd "$WORK/node" && npm ci --ignore-scripts --no-audit --no-fund --loglevel=error)
CODEX_PKG="$WORK/node/node_modules/@openai/codex-darwin-arm64"
got=$("$CODEX_PKG/vendor/aarch64-apple-darwin/bin/codex" --version | awk '{ print $NF }')
[[ "$got" == "$CODEX_VERSION" ]] || die "package-lock.json has Codex $got, ferb pins $CODEX_VERSION"
mkdir -p "$CONTENTS/Resources/node/node_modules/@openai"
ditto "$CODEX_PKG" "$CONTENTS/Resources/node/node_modules/@openai/codex-darwin-arm64"

print "==> Secret helper"
xcrun swiftc -O -target arm64-apple-macos14.2 -file-prefix-map "$ROOT=." -o "$CONTENTS/Helpers/ferb-secrets" \
  "$ROOT/ferb/identity/secret_helper.swift" -framework Security -framework CryptoKit
strip -S "$CONTENTS/Helpers/ferb-secrets"

print "==> Voice helper (macapp/FERB; merges into the main app after Task 20)"
VOICE="$CONTENTS/Helpers/FERB Voice.app"
IDENTITY=- "$ROOT/macapp/FERB/build.sh" "$VOICE" >/dev/null
plist "Set :CFBundleName FERB Voice" "$VOICE/Contents/Info.plist"   # its own name in permission prompts; same id
plist "Add :CFBundleDisplayName string FERB Voice" "$VOICE/Contents/Info.plist"

print "==> Signing (identity: $SIGN_IDENTITY)"
ferb_sign_app "$APP" "$WORK/sign"
if ferb_developer_id; then ferb_notarize_app "$APP" "$WORK"; fi

print "Built: $APP ($(du -sh "$APP" | cut -f1); FERB $VERSION, build $BUILD, $REVISION)"
ferb_size_report "$APP"
