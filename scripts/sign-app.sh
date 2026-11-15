#!/bin/zsh -f
# Sign an assembled FERB.app again (scripts/signing.sh), e.g. with a Developer ID; no rebuild. Every nested Mach-O file,
# Sparkle, the agent app and the app are signed inside out and checked, then a Developer ID build is notarized and
# stapled (FERB_NOTARIZE=0 skips that for a build on this Mac only).
#
#   scripts/sign-app.sh APP
#
# Env: SIGN_IDENTITY (default "-", ad hoc; devid or release for a published build) and, for a Developer ID, the notary
# credentials described in signing.sh. Never installs, registers or starts anything.
set -euo pipefail
ROOT="${0:A:h:h}"
source "$ROOT/scripts/signing.sh"

die() { print -u2 "sign-app.sh: $*"; exit 1 }

APP="${${1:?usage: sign-app.sh APP}:A}"
[[ -f "$APP/Contents/Info.plist" ]] || die "$APP isn't an app bundle"
ferb_check_notary_credentials
WORK="${APP:h}/.sign-work"
trap 'rm -rf "$WORK"' EXIT
ferb_sign_app "$APP" "$WORK"
ferb_notarize_built "$APP" "$WORK"
print "Signed: $APP (identity: $SIGN_IDENTITY)"
