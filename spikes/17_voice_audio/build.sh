#!/bin/sh
# Build the spike app bundle and sign it (ad-hoc by default; IDENTITY="name" to use a local certificate).
set -e
cd "$(dirname "$0")"
APP=build/FERBAudioSpike.app
rm -rf "$APP" && mkdir -p "$APP/Contents/MacOS"
cp Info.plist "$APP/Contents/Info.plist"
swiftc -O -target arm64-apple-macos14.2 -o "$APP/Contents/MacOS/FERBAudioSpike" main.swift \
  -framework AppKit -framework AVFoundation -framework CoreAudio -framework Speech
codesign --force --sign "${IDENTITY:--}" "$APP"
codesign -dv "$APP" 2>&1 | grep -E "Identifier|Signature|CDHash" || true
