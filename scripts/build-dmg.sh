#!/bin/zsh -f
# Package FERB.app into a disk image with an Applications link (L27). Ad-hoc by default; with a Developer ID
# SIGN_IDENTITY the image is also signed, notarized and stapled (scripts/signing.sh).
#
#   scripts/build-dmg.sh [APP] [DMG]   defaults: build/app/FERB.app (built first if it's missing) and
#                                      build/FERB-<version>.dmg
set -euo pipefail
ROOT="${0:A:h:h}"
source "$ROOT/scripts/signing.sh"

APP="${1:-$ROOT/build/app/FERB.app}"
ferb_check_notary_credentials
if [[ ! -d "$APP" ]]; then
  if [[ $# -ge 1 ]]; then print -u2 "build-dmg.sh: no app at $APP"; exit 1; fi
  "$ROOT/scripts/build-app.sh" "${APP:h}"
fi
APP=${APP:A}
VERSION=$(/usr/libexec/PlistBuddy -c "Print :CFBundleShortVersionString" "$APP/Contents/Info.plist")
DMG="${${2:-$ROOT/build/FERB-$VERSION.dmg}:A}"
STAGE="${DMG:h}/.dmg-staging"
rm -rf "$STAGE" "$DMG"
mkdir -p "$STAGE"
trap 'rm -rf "$STAGE"' EXIT
ditto "$APP" "$STAGE/FERB.app"
ln -s /Applications "$STAGE/Applications"
hdiutil create -volname FERB -srcfolder "$STAGE" -fs HFS+ -format ULMO -ov "$DMG" >/dev/null
if ferb_developer_id; then
  codesign --force --sign "$SIGN_IDENTITY" --timestamp "$DMG"
  ferb_notarize "$DMG" "$DMG"
fi
print "Built: $DMG ($(du -h "$DMG" | cut -f1))"
