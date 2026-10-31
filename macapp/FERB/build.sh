#!/bin/sh
# Build FERB.app into $1 (default: ./build/FERB.app) for $ARCH (default: this Mac's) and sign it with the local
# certificate (or $IDENTITY).
set -e
cd "$(dirname "$0")"
APP="${1:-build/FERB.app}"
rm -rf "$APP" && mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp Info.plist "$APP/Contents/Info.plist"
cp ../../assets/logo/AppIcon.icns "$APP/Contents/Resources/AppIcon.icns"     # FERB's logo (Info.plist CFBundleIconFile)
swiftc -O -target "${ARCH:-$(uname -m)}-apple-macos14.2" -o "$APP/Contents/MacOS/FERB" main.swift home.swift \
  -framework AppKit -framework Speech
codesign --force --sign "${IDENTITY:-FERB Local Code Signing}" "$APP"
