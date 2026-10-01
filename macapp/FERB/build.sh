#!/bin/sh
# Build FERB.app into $1 (default: ./build/FERB.app) and sign it with the local certificate (or $IDENTITY).
set -e
cd "$(dirname "$0")"
APP="${1:-build/FERB.app}"
rm -rf "$APP" && mkdir -p "$APP/Contents/MacOS"
cp Info.plist "$APP/Contents/Info.plist"
swiftc -O -target arm64-apple-macos14.2 -o "$APP/Contents/MacOS/FERB" main.swift \
  -framework AppKit -framework AVFoundation -framework CoreAudio -framework Speech
codesign --force --sign "${IDENTITY:-FERB Local Code Signing}" "$APP"
