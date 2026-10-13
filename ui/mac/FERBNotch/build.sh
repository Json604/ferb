#!/bin/zsh
set -euo pipefail
PROJECT_DIR="${0:A:h}"
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
cp "$PROJECT_DIR/Info.plist" "$APP_DIR/Contents/Info.plist"
cp "$PROJECT_DIR/ThirdPartyNotices.txt" "$APP_DIR/Contents/Resources/ThirdPartyNotices.txt"
codesign --force --sign - "$APP_DIR"
print "Built: $APP_DIR"
