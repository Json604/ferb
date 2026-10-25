#!/bin/zsh -f
# Package FERB.app into disk images with an Applications link (L27, L34): one per architecture. Ad-hoc by default;
# with a Developer ID SIGN_IDENTITY each image is also signed, notarized and stapled (scripts/signing.sh).
#
#   scripts/build-dmg.sh             build both apps (scripts/build-app.sh --arch arm64, then x86_64) into
#                                    $FERB_DMG_DIR/app-<arch>, then FERB-AppleSilicon.dmg and FERB-Intel.dmg there
#   scripts/build-dmg.sh APP [DMG]   one image of an assembled app; default $FERB_DMG_DIR/<name for its architecture>
#
# FERB_DMG_DIR defaults to build/.
set -euo pipefail
ROOT="${0:A:h:h}"
source "$ROOT/scripts/signing.sh"

die() { print -u2 "build-dmg.sh: $*"; exit 1 }
DMG_DIR="${${FERB_DMG_DIR:-$ROOT/build}:A}"
FIRST_OPEN="First open — System Settings → Privacy & Security → Open Anyway.txt"
STAGE=""
trap 'if [[ -n "$STAGE" ]]; then rm -rf "$STAGE"; fi' EXIT

dmg_name() {                          # $1: an architecture, as lipo names it
  case ${1:-} in
    arm64)  print FERB-AppleSilicon.dmg ;;
    x86_64) print FERB-Intel.dmg ;;
    *) return 1 ;;
  esac
}

make_dmg() {                          # $1: an assembled FERB.app, $2: the image to write
  local app=${1:A} dmg=${2:A}
  STAGE="${dmg:h}/.dmg-staging"
  rm -rf "${STAGE:?}" "${dmg:?}"
  mkdir -p "$STAGE"
  ditto "$app" "$STAGE/FERB.app"
  ln -s /Applications "$STAGE/Applications"
  print -l -- "FERB isn't signed by an Apple-registered developer, so macOS stops it the first time:" \
    "1. Drag FERB to Applications and open it from there." \
    "2. Open System Settings → Privacy & Security, scroll to Security and click Open Anyway next to FERB." \
    "3. Enter your password. FERB opens, and macOS remembers your choice." > "$STAGE/$FIRST_OPEN"
  hdiutil create -volname FERB -srcfolder "$STAGE" -fs HFS+ -format ULMO -ov "$dmg" >/dev/null
  rm -rf "${STAGE:?}"
  STAGE=""
  if ferb_developer_id; then
    codesign --force --sign "$SIGN_IDENTITY" --timestamp "$dmg"
    ferb_notarize "$dmg" "$dmg"
  fi
  print "Built: $dmg ($(du -h "$dmg" | cut -f1))"
}

ferb_check_notary_credentials
if (( $# == 0 )); then
  mkdir -p "$DMG_DIR"
  for arch in arm64 x86_64; do
    "$ROOT/scripts/build-app.sh" --arch $arch "$DMG_DIR/app-$arch"
    make_dmg "$DMG_DIR/app-$arch/FERB.app" "$DMG_DIR/$(dmg_name $arch)"
  done
  exit 0
fi
APP="$1"
[[ -d "$APP" ]] || die "no app at $APP"
if (( $# >= 2 )); then
  DMG="$2"
else
  arch=$(lipo -archs "$APP/Contents/MacOS/FERBNotch" 2>/dev/null || true)
  name=$(dmg_name "$arch") || die "$APP isn't a one-architecture FERB.app (lipo says '${arch:-nothing}')"
  mkdir -p "$DMG_DIR"
  DMG="$DMG_DIR/$name"
fi
make_dmg "$APP" "$DMG"
