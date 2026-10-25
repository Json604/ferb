#!/bin/zsh
set -euo pipefail
PROJECT_DIR="${0:A:h}"
source "${PROJECT_DIR:h:h:h}/scripts/swiftpm-anonymous.sh"      # SwiftPM fetches anonymously: never the owner's keychain
CONFIGURATION="${1:-debug}"
if [[ "$CONFIGURATION" != debug && "$CONFIGURATION" != release ]]; then
  print -u2 "Usage: ./build.sh [debug|release]"
  exit 1
fi
swift build --package-path "$PROJECT_DIR" -c "$CONFIGURATION"
BINARY_DIR="$(swift build --package-path "$PROJECT_DIR" -c "$CONFIGURATION" --show-bin-path)"
APP_DIR="$PROJECT_DIR/build/FERB Notch.app"
mkdir -p "$APP_DIR/Contents/MacOS"
mkdir -p "$APP_DIR/Contents/Resources"
cp "$BINARY_DIR/FERBNotch" "$APP_DIR/Contents/MacOS/FERBNotch"
# Sparkle.framework (L34 Phase G): the binary links it; a development build has no feed, so it never checks
mkdir -p "$APP_DIR/Contents/Frameworks"
rm -rf "$APP_DIR/Contents/Frameworks/Sparkle.framework"
ditto "$BINARY_DIR/Sparkle.framework" "$APP_DIR/Contents/Frameworks/Sparkle.framework"
install_name_tool -add_rpath @executable_path/../Frameworks "$APP_DIR/Contents/MacOS/FERBNotch"
cp "$PROJECT_DIR/Info.plist" "$APP_DIR/Contents/Info.plist"
# The development build keeps its own identity, so macOS never confuses it (or its permissions) with the packaged
# FERB.app that scripts/build-app.sh makes from the same Info.plist.
for key value in CFBundleIdentifier dev.ferb.notch CFBundleName "FERB Notch" CFBundleDisplayName "FERB Notch"; do
  /usr/libexec/PlistBuddy -c "Set :$key $value" "$APP_DIR/Contents/Info.plist"
done
cp "$PROJECT_DIR/ThirdPartyNotices.txt" "$APP_DIR/Contents/Resources/ThirdPartyNotices.txt"
codesign --force --sign - "$APP_DIR"
print "Built: $APP_DIR"
