#!/bin/zsh -f
# The Sparkle appcast for one architecture's release DMG (L34 Phase G): checks the DMG and its app are FERB's Developer
# ID's, notarized and stapled, reads the version and build from the app in the DMG, checks that app asks for this
# architecture's feed, signs the DMG with FERB's EdDSA key (Sparkle's sign_update; the private key is in the owner's
# login Keychain) and writes build/appcast-<arch>.xml. The EdDSA signature covers the DMG's final bytes, so it comes
# after build-dmg.sh stapled the ticket, which this refuses to skip. Publishing is the owner's step (README,
# "Releasing").
#
#   scripts/release-appcast.sh TAG DMG ARCH     e.g. v0.28.0 build/FERB-AppleSilicon.dmg arm64
#
# SIGN_IDENTITY=release checks a DMG of the self-made release certificate instead (kept until the first Developer ID
# release passes its live checks).
set -euo pipefail
ROOT="${0:A:h:h}"
NOTCH="$ROOT/ui/mac/FERBNotch"
die() { print -u2 "release-appcast.sh: $*"; exit 1 }

TAG=${1:?usage: release-appcast.sh TAG DMG ARCH}
DMG=${${2:?usage: release-appcast.sh TAG DMG ARCH}:A}
ARCH=${3:?usage: release-appcast.sh TAG DMG ARCH}
[[ "$ARCH" == (arm64|x86_64) ]] || die "ARCH is arm64 or x86_64"
[[ -f "$DMG" ]] || die "no DMG at $DMG"
source "$ROOT/scripts/signing.sh"

MNT=$(mktemp -d)
trap 'hdiutil detach -quiet "$MNT" 2>/dev/null || true; rmdir "$MNT" 2>/dev/null || true' EXIT
hdiutil attach -nobrowse -readonly -quiet -mountpoint "$MNT" "$DMG"
if ferb_release; then
  codesign --verify --strict -R="$(ferb_requirement com.ferb.FERB)" "$MNT/FERB.app" 2>/dev/null \
    || die "the app in ${DMG:t} isn't signed with FERB's release certificate"
else
  ferb_check_devid_app "$MNT/FERB.app" || die "the app in ${DMG:t} isn't FERB's notarized Developer ID build (above)"
  xcrun stapler validate "$DMG" >/dev/null 2>&1 \
    || die "${DMG:t} carries no stapled notarization ticket (scripts/build-dmg.sh staples it)"
  ferb_check_gatekeeper open "$DMG" "$FERB_TEAM_ID" || die "${DMG:t} isn't FERB's notarized Developer ID image (above)"
fi
INFO="$MNT/FERB.app/Contents/Info.plist"
VERSION=$(/usr/libexec/PlistBuddy -c "Print :CFBundleShortVersionString" "$INFO")
BUILD=$(/usr/libexec/PlistBuddy -c "Print :CFBundleVersion" "$INFO")
MINOS=$(/usr/libexec/PlistBuddy -c "Print :LSMinimumSystemVersion" "$INFO")
FEED=$(/usr/libexec/PlistBuddy -c "Print :SUFeedURL" "$INFO" 2>/dev/null || true)
FULL_VERSION=$(/usr/libexec/PlistBuddy -c "Print :FERBVersion" "$INFO")
TEAM=$(codesign -dv "$MNT/FERB.app" 2>&1 | sed -n 's/^TeamIdentifier=//p')
hdiutil detach -quiet "$MNT"
# The first build signed with the Developer ID: every appcast from then on names it as Sparkle's
# minimumAutoupdateVersion, so an installed self-signed FERB asks the owner before that update (macOS asks for every
# permission once more after the signer changes, APP-903) and Developer ID builds update silently among themselves.
FIRST_DEVID="$ROOT/scripts/developer-id-first-build.txt"
MIN_AUTO=""
if [[ -s "$FIRST_DEVID" ]]; then MIN_AUTO=$(<"$FIRST_DEVID"); fi
if [[ -n "$TEAM" && "$TEAM" != "not set" ]] && [[ -z "$MIN_AUTO" || "$BUILD" < "$MIN_AUTO" ]]; then
  # the earliest Developer ID build of either architecture (each DMG's app has its own build number)
  print "$BUILD" > "$FIRST_DEVID"
  MIN_AUTO=$BUILD
  print "Recorded $BUILD as FERB's first Developer ID build in scripts/developer-id-first-build.txt: commit it."
fi
[[ "$FEED" == *"/appcast-$ARCH.xml" ]] || die "the app in ${DMG:t} checks '${FEED:-no feed}', not appcast-$ARCH.xml"
"$ROOT/tools/kb/run" gate release || die "the repo gate refused the release (above)"   # kb/setup.toml release_gate
SIGN=$(find "$NOTCH/.build/artifacts" -path '*/Sparkle/bin/sign_update' -type f 2>/dev/null | head -1)
[[ -n "$SIGN" ]] || die "Sparkle's sign_update isn't here yet: swift package --package-path ui/mac/FERBNotch resolve"

# The release checks (scripts/release_checks.py): a bumped, non-development version named by the tag, and the Chrome
# Web Store already serving an extension this FERB accepts.
RELEASED=(${(f)"$(git -C "$ROOT" ls-remote --tags --refs origin 'v*' | sed 's#.*refs/tags/##')"})
STORE_ID=$(sed -n 's/^STORE_EXTENSION_ID = "\([a-p]*\)".*/\1/p' "$ROOT/ferb/act/browser/autoinstall.py")
MIN_EXT=$(sed -n 's/^MIN_EXTENSION_VERSION = "\(.*\)"$/\1/p' "$ROOT/ferb/act/browser/ext_protocol.py")
STORE_EXT=$(curl -fsS --max-time 20 "https://clients2.google.com/service/update2/crx?response=updatecheck&acceptformat=crx3&prodversion=140.0&x=id%3D$STORE_ID%26uc" \
  | sed -n 's/.*<updatecheck[^>]* version="\([0-9.]*\)".*/\1/p' || true)
/usr/bin/python3 -I "$ROOT/scripts/release_checks.py" --version "$FULL_VERSION" --tag "$TAG" --released "${RELEASED[@]}" \
  --store-extension "$STORE_EXT" --min-extension "$MIN_EXT" || die "not releasable yet (above)"

SIGNATURE=$("$SIGN" "$DMG")
mkdir -p "$ROOT/build"
/usr/bin/python3 -I "$ROOT/scripts/make_appcast.py" --version "$VERSION" --build "$BUILD" --tag "$TAG" \
  --dmg "${DMG:t}" --signature "$SIGNATURE" --min-macos "$MINOS" --minimum-autoupdate "$MIN_AUTO" \
  --out "$ROOT/build/appcast-$ARCH.xml"
print "Wrote build/appcast-$ARCH.xml for FERB $VERSION ($BUILD, $ARCH)."
