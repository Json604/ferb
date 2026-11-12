#!/bin/zsh -f
# Package FERB.app into disk images with an Applications link (L27, L34): one per architecture. An image holds only a
# FERB.app signed by FERB's Developer ID team, notarized and stapled (SIGN_IDENTITY=devid; the image itself is then
# signed, notarized, stapled and checked with Gatekeeper too) or, until that release passes its live checks, a
# release-signed one (SIGN_IDENTITY=release); scripts/signing.sh. FERB_DEV_DMG=1 makes a test image of any app.
#
# The image opens as a designed window (REL-408, REL-409; scripts/dmg_layout.py): FERB on the left, the Applications
# link on the right, over a still of FERB's startup field (assets/dmg, from scripts/make_dmg_background.py). The words
# follow the signing identity: a Developer ID build (notarized) says only "Drag FERB to Applications"; any other also
# says how to get past Gatekeeper the first time. FERB_DMG_VARIANT=before-devid|after-devid overrides it for previews.
# dmg_layout.py writes the window's .DS_Store onto the mounted image (never by driving Finder), run with
# $FERB_DMG_PYTHON, by default the checkout's uv environment, whose dev group pins ds_store and mac_alias.
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
VOLUME_ICON="$ROOT/assets/logo/AppIcon.icns"   # FERB's logo (scripts/make_logo.py) as the image's volume icon
STAGE="" MOUNT="" RW=""
trap 'if [[ -n "$MOUNT" ]]; then hdiutil detach "$MOUNT" -force >/dev/null 2>&1 || true; fi
      if [[ -n "$STAGE" ]]; then rm -rf "$STAGE"; fi
      if [[ -n "$RW" ]]; then rm -f "$RW"; fi' EXIT

dmg_name() {                          # $1: an architecture, as lipo names it
  case ${1:-} in
    arm64)  print FERB-AppleSilicon.dmg ;;
    x86_64) print FERB-Intel.dmg ;;
    *) return 1 ;;
  esac
}

layout_python() {                     # dmg_layout.py's interpreter: FERB_DMG_PYTHON, or the checkout's uv environment
  if [[ -n "${FERB_DMG_PYTHON:-}" ]]; then
    "$FERB_DMG_PYTHON" -I "$@"
  else
    uv run --frozen --project "$ROOT" python -I "$@"
  fi
}

dmg_variant() {                       # the window's words follow the identity (REL-409)
  case ${FERB_DMG_VARIANT:-} in
    before-devid|after-devid) print -r -- "$FERB_DMG_VARIANT" ;;
    "") if ferb_developer_id; then print after-devid; else print before-devid; fi ;;
    *) die "FERB_DMG_VARIANT is before-devid or after-devid, not '$FERB_DMG_VARIANT'" ;;
  esac
}

make_dmg() {                          # $1: an assembled FERB.app, $2: the image to write
  local app=${1:A} dmg=${2:A} variant
  ferb_check_release_app "$app" || exit 1
  variant=$(dmg_variant) || exit 1
  local bg="$ROOT/assets/dmg/background-$variant.png" bg2="$ROOT/assets/dmg/background-$variant@2x.png"
  [[ -f "$bg" && -f "$bg2" ]] || die "no $bg and its @2x (uv run python scripts/make_dmg_background.py)"
  STAGE="${dmg:h}/.dmg-staging"
  rm -rf "${STAGE:?}" "${dmg:?}"
  mkdir -p "$STAGE"
  ditto "$app" "$STAGE/FERB.app"
  ln -s /Applications "$STAGE/Applications"
  mkdir "$STAGE/.background"            # one TIFF holding the 1x and 2x backgrounds: Retina screens get the 2x
  tiffutil -cathidpicheck "$bg" "$bg2" -out "$STAGE/.background/FERB.tiff" >/dev/null 2>&1
  cp "$VOLUME_ICON" "$STAGE/.VolumeIcon.icns"
  # The volume shows that icon only with the custom-icon flag on its root, which a writable image can take: build
  # one, set the flag, then compress it into the read-only image
  RW="${dmg:h}/.dmg-rw.dmg"
  MOUNT="${dmg:h}/.dmg-mount"
  hdiutil create -volname FERB -srcfolder "$STAGE" -fs HFS+ -format UDRW -ov "$RW" >/dev/null
  rm -rf "${STAGE:?}"
  STAGE=""
  hdiutil attach -nobrowse -noautoopen -noverify -mountpoint "$MOUNT" "$RW" >/dev/null
  xattr -wx com.apple.FinderInfo 0000000000000000040000000000000000000000000000000000000000000000 "$MOUNT"
  layout_python "$ROOT/scripts/dmg_layout.py" ds-store "$MOUNT"
  rm -rf "$MOUNT/.fseventsd"            # the mount's own event log, nothing of the image
  hdiutil detach "$MOUNT" >/dev/null
  MOUNT=""
  hdiutil convert "$RW" -format ULMO -ov -o "$dmg" >/dev/null
  rm -f "${RW:?}"
  RW=""
  if ferb_developer_id; then             # signed by the resolved identity (devid: its SHA-1, never the name)
    codesign --force --sign "$FERB_SIGN_ID" --timestamp ${FERB_SIGN_KEYCHAIN:+--keychain} ${FERB_SIGN_KEYCHAIN:+"$FERB_SIGN_KEYCHAIN"} "$dmg"
    ferb_notarize "$dmg" "$dmg"
    if ferb_devid; then                   # what a browser download meets: the image's own notarized team signature
      ferb_check_gatekeeper open "$dmg" "$FERB_TEAM_ID" || exit 1
    else
      ferb_check_gatekeeper open "$dmg" || exit 1
    fi
  fi
  print "Built: $dmg ($(du -h "$dmg" | cut -f1), window: $variant)"
}

ferb_check_notary_credentials
if (( $# == 0 )); then
  ferb_check_release_app || exit 1                # before the builds, not after
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
