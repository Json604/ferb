#!/bin/zsh
# The intro playground (PLAYGROUND.md): builds the notch app (debug) and opens it as "FERB Setup Concept"
# (bundle id dev.ferb.playground.concept), which runs the real first-run intro with everything behind it faked.
#   ui/mac/FERBNotch/playground.sh               build, then open the playground fresh (the intro from the top)
#   ui/mac/FERBNotch/playground.sh --reopen      build, then open it keeping the last run's state
#   ui/mac/FERBNotch/playground.sh --build-only  build the app bundle, don't open it
#   ui/mac/FERBNotch/playground.sh --play        open the app playground and start Play 0.27.1 at once
set -euo pipefail
PROJECT_DIR="${0:A:h}"
REPO="${PROJECT_DIR:h:h:h}"
source "$REPO/scripts/swiftpm-anonymous.sh"      # SwiftPM fetches anonymously: never the owner's keychain

OPEN=1
CONCEPT_NAME="FERB Setup Concept"
CONCEPT_ID="dev.ferb.playground.concept"
CONCEPT_HOME="ferb-setup-concept"
ARGS=()
for arg in "$@"; do
  case "$arg" in
    --arrival) CONCEPT_NAME="FERB Arrival Concept"; CONCEPT_ID="dev.ferb.playground.arrival"; CONCEPT_HOME="ferb-arrival-concept" ;;
    --build-only) OPEN=0 ;;
    --reopen) ARGS+=(--reopen) ;;
    --app) ARGS+=(--app) ;;
    --pause-before=*) ARGS+=("$arg") ;;
    --play) ARGS+=(--app --play); CONCEPT_NAME="FERB Arrival Concept"; CONCEPT_ID="dev.ferb.playground.arrival"; CONCEPT_HOME="ferb-arrival-concept" ;;
    *) print -u2 "Usage: playground.sh [--arrival] [--app] [--play] [--reopen] [--build-only]"; exit 1 ;;
  esac
done

# The playground's home: a temporary folder, never FERB's real ~/.ferb (the app checks again, resolving symlinks).
REAL_FERB_HOME="$(dscl . -read "/Users/$(id -un)" NFSHomeDirectory | awk '{print $2}')/.ferb"
PLAYGROUND_HOME="${FERB_HOME:-$(getconf DARWIN_USER_TEMP_DIR)$CONCEPT_HOME}"
real="${REAL_FERB_HOME:A}"
given="${PLAYGROUND_HOME:A}"
if [[ "$given" == "$real" || "$given" == "$real"/* || "$real" == "$given"/* ]]; then
  print -u2 "playground.sh: FERB_HOME ($PLAYGROUND_HOME) is or holds your real ~/.ferb; the playground never uses it."
  exit 1
fi
mkdir -p "$PLAYGROUND_HOME"

swift build --package-path "$PROJECT_DIR" -c debug
BINARY_DIR="$(swift build --package-path "$PROJECT_DIR" -c debug --show-bin-path)"
APP="$PROJECT_DIR/build/$CONCEPT_NAME.app"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources" "$APP/Contents/Frameworks"
cp "$BINARY_DIR/FERBNotch" "$APP/Contents/MacOS/FERBNotch"
# The binary links Sparkle.framework; the playground never starts it (no feed, no updater object)
rm -rf "$APP/Contents/Frameworks/Sparkle.framework"
ditto "$BINARY_DIR/Sparkle.framework" "$APP/Contents/Frameworks/Sparkle.framework"
install_name_tool -add_rpath @executable_path/../Frameworks "$APP/Contents/MacOS/FERBNotch" 2>/dev/null || true
# The sounds and the mark, where Bundle.module looks inside an app (as scripts/build-app.sh puts them)
rm -rf "$APP/Contents/Resources/FERBNotch_FERBNotch.bundle"
ditto "$BINARY_DIR/FERBNotch_FERBNotch.bundle" "$APP/Contents/Resources/FERBNotch_FERBNotch.bundle"
cp "$PROJECT_DIR/Info.plist" "$APP/Contents/Info.plist"
# Its own identity: never FERB.app's (com.ferb.FERB) or the development notch's (dev.ferb.notch) preferences,
# permission grants or Keychain access.
for key value in CFBundleIdentifier "$CONCEPT_ID" CFBundleName "$CONCEPT_NAME" CFBundleDisplayName "$CONCEPT_NAME"; do
  /usr/libexec/PlistBuddy -c "Set :$key $value" "$APP/Contents/Info.plist"
done
cp "$REPO/assets/logo/AppIcon.icns" "$APP/Contents/Resources/AppIcon.icns"
codesign --force --sign - "$APP"
print "Built: $APP"
print "Home:  $PLAYGROUND_HOME"
(( OPEN )) || exit 0

# A playground still open from before quits first (only this bundle's own process), so the new build is the one shown.
for pid in $(pgrep -f "$APP/Contents/MacOS/FERBNotch" || true); do
  kill "$pid" 2>/dev/null || true
done
while pgrep -f "$APP/Contents/MacOS/FERBNotch" >/dev/null; do sleep 0.1; done
open -n --env "FERB_HOME=$PLAYGROUND_HOME" "$APP" --args "${ARGS[@]}"
print "Opened $CONCEPT_NAME (its control panel is a separate window)."
