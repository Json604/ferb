#!/bin/zsh -f
# The Sparkle appcast for one architecture's release DMG (L34 Phase G): reads the version and build from the app in the
# DMG, checks that app asks for this architecture's feed, signs the DMG with FERB's EdDSA key (Sparkle's sign_update;
# the private key is in the owner's login Keychain) and writes build/appcast-<arch>.xml. Publishing is the owner's step
# (README, "Releasing").
#
#   scripts/release-appcast.sh TAG DMG ARCH     e.g. v0.28.0 build/FERB-AppleSilicon.dmg arm64
set -euo pipefail
ROOT="${0:A:h:h}"
NOTCH="$ROOT/ui/mac/FERBNotch"
die() { print -u2 "release-appcast.sh: $*"; exit 1 }

TAG=${1:?usage: release-appcast.sh TAG DMG ARCH}
DMG=${${2:?usage: release-appcast.sh TAG DMG ARCH}:A}
ARCH=${3:?usage: release-appcast.sh TAG DMG ARCH}
[[ "$ARCH" == (arm64|x86_64) ]] || die "ARCH is arm64 or x86_64"
[[ -f "$DMG" ]] || die "no DMG at $DMG"
SIGN=$(find "$NOTCH/.build/artifacts" -path '*/Sparkle/bin/sign_update' -type f 2>/dev/null | head -1)
[[ -n "$SIGN" ]] || die "Sparkle's sign_update isn't here yet: swift package --package-path ui/mac/FERBNotch resolve"

MNT=$(mktemp -d)
trap 'hdiutil detach -quiet "$MNT" 2>/dev/null || true; rmdir "$MNT" 2>/dev/null || true' EXIT
hdiutil attach -nobrowse -readonly -quiet -mountpoint "$MNT" "$DMG"
INFO="$MNT/FERB.app/Contents/Info.plist"
VERSION=$(/usr/libexec/PlistBuddy -c "Print :CFBundleShortVersionString" "$INFO")
BUILD=$(/usr/libexec/PlistBuddy -c "Print :CFBundleVersion" "$INFO")
MINOS=$(/usr/libexec/PlistBuddy -c "Print :LSMinimumSystemVersion" "$INFO")
FEED=$(/usr/libexec/PlistBuddy -c "Print :SUFeedURL" "$INFO" 2>/dev/null || true)
hdiutil detach -quiet "$MNT"
[[ "$FEED" == *"/appcast-$ARCH.xml" ]] || die "the app in ${DMG:t} checks '${FEED:-no feed}', not appcast-$ARCH.xml"

SIGNATURE=$("$SIGN" "$DMG")
mkdir -p "$ROOT/build"
/usr/bin/python3 -I "$ROOT/scripts/make_appcast.py" --version "$VERSION" --build "$BUILD" --tag "$TAG" \
  --dmg "${DMG:t}" --signature "$SIGNATURE" --min-macos "$MINOS" --out "$ROOT/build/appcast-$ARCH.xml"
print "Wrote build/appcast-$ARCH.xml for FERB $VERSION ($BUILD, $ARCH)."
