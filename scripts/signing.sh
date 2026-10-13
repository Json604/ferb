# FERB.app's one signing step (L27), sourced by build-app.sh and build-dmg.sh (zsh).
#
# SIGN_IDENTITY is "-" (ad-hoc, the default) or a codesign identity. A "Developer ID Application: …" identity also
# gets a secure timestamp, notarization and a stapled ticket: the same bundle is signed again, never rebuilt
# (`build-app.sh --sign-only APP`). Notary credentials come only from the environment, are passed to notarytool
# and are never written to a file or printed:
#   NOTARY_KEYCHAIN_PROFILE                      a profile saved with `xcrun notarytool store-credentials`, or
#   NOTARY_KEY, NOTARY_KEY_ID [, NOTARY_ISSUER]   an App Store Connect API key file, its ID (and a team key's issuer).

: ${SIGN_IDENTITY:=-}

ferb_developer_id() { [[ "$SIGN_IDENTITY" == "Developer ID Application:"* ]] }

ferb_check_notary_credentials() {
  ferb_developer_id || return 0
  [[ -n "${NOTARY_KEYCHAIN_PROFILE:-}" ]] && return 0
  [[ -n "${NOTARY_KEY:-}" && -n "${NOTARY_KEY_ID:-}" ]] && return 0
  print -u2 "A Developer ID build is notarized: set NOTARY_KEYCHAIN_PROFILE (saved with \`xcrun notarytool store-credentials\`), or NOTARY_KEY and NOTARY_KEY_ID (plus NOTARY_ISSUER for a team key)."
  return 1
}

# $1: daemon | app | voice; $2: the entitlements file to write. Only what each needs under the hardened runtime.
ferb_entitlements() {
  local -a keys
  case $1 in
    daemon) keys=(com.apple.security.automation.apple-events) ;;     # Automation checks and osascript (permissions.py)
    app|voice) keys=(com.apple.security.device.audio-input) ;;       # dictation / push-to-talk microphone
  esac
  # Ad-hoc (or any non-Developer-ID) code has no Team ID, so library validation would refuse every extension module
  # the interpreter loads. One Developer ID team signs all of them, so validation stays on in that build.
  if [[ $1 == daemon ]] && ! ferb_developer_id; then keys+=(com.apple.security.cs.disable-library-validation); fi
  {
    print '<?xml version="1.0" encoding="UTF-8"?>'
    print '<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">'
    print '<plist version="1.0"><dict>'
    local key
    for key in $keys; do print "<key>$key</key><true/>"; done
    print '</dict></plist>'
  } > "$2"
}

# ferb_sign PATH [codesign options…]: hardened runtime always (notarization requires it); a secure timestamp only for
# a Developer ID (it needs Apple's timestamp server).
ferb_sign() {
  local target=$1; shift
  local stamp=--timestamp=none
  ferb_developer_id && stamp=--timestamp
  codesign --force --sign "$SIGN_IDENTITY" --options runtime $stamp "$@" "$target" 2>&1 \
    | { grep -v ': replacing existing signature$' || true } >&2
}

ferb_is_macho() { [[ "$(file -b "$1")" == Mach-O* ]] }

# An executable another team signed with its Developer ID (the bundled Claude and Codex CLIs): kept exactly as its
# vendor signed and notarized it. Libraries are always signed again: they load into FERB's own processes.
ferb_vendor_signed() {
  local f=$1 info team
  [[ $f == *.so || $f == *.dylib || ! -x $f ]] && return 1
  info=$(codesign -dv --verbose=2 "$f" 2>&1) || return 1
  [[ $info == *"Authority=Developer ID Application:"* ]] || return 1
  team=${${info##*TeamIdentifier=}%%$'\n'*}
  [[ "$SIGN_IDENTITY" == *"($team)" ]] && return 1                  # our own team: sign it again
  codesign --verify --strict "$f" 2>/dev/null
}

# ferb_sign_app APP WORKDIR: every nested binary first (deepest first), then nested apps, then the app; then verify.
ferb_sign_app() {
  local app=${1:A} work=$2 f main
  mkdir -p "$work"
  ferb_entitlements daemon "$work/daemon.entitlements"
  ferb_entitlements app "$work/app.entitlements"
  ferb_entitlements voice "$work/voice.entitlements"
  main="$app/Contents/MacOS/$(/usr/libexec/PlistBuddy -c 'Print :CFBundleExecutable' "$app/Contents/Info.plist" 2>/dev/null || print FERBNotch)"
  local -a files
  files=("${(@f)$(find "$app/Contents" -name '*.app' -prune -o -type f \( -name '*.so' -o -name '*.dylib' -o -perm -u+x \) -print \
                  | awk -F/ '{ print NF "\t" $0 }' | sort -rn | cut -f2-)}")
  for f in $files; do
    [[ -z $f || $f == $main ]] && continue
    ferb_is_macho "$f" || continue
    ferb_vendor_signed "$f" && continue
    case $f in
      "$app/Contents/Resources/python/bin/python3.12")
        ferb_sign "$f" --identifier com.ferb.daemon --entitlements "$work/daemon.entitlements" ;;
      "$app/Contents/Helpers/ferb-secrets")
        ferb_sign "$f" --identifier com.ferb.secrets ;;
      *)
        ferb_sign "$f" ;;
    esac
  done
  local nested
  for nested in "$app"/Contents/Helpers/*.app(N); do
    ferb_sign "$nested" --entitlements "$work/voice.entitlements"
  done
  ferb_sign "$app" --entitlements "$work/app.entitlements"
  codesign --verify --deep --strict --verbose=2 "$app"
}

# ferb_notarize FILE STAPLE_TARGET: submit (zip or dmg), wait, require "Accepted", staple the ticket and check it.
ferb_notarize() {
  local -a creds
  if [[ -n "${NOTARY_KEYCHAIN_PROFILE:-}" ]]; then
    creds=(--keychain-profile "$NOTARY_KEYCHAIN_PROFILE")
  else
    creds=(--key "$NOTARY_KEY" --key-id "$NOTARY_KEY_ID")
    [[ -n "${NOTARY_ISSUER:-}" ]] && creds+=(--issuer "$NOTARY_ISSUER")
  fi
  local out state id
  out=$(xcrun notarytool submit "$1" --wait --output-format json $creds) || { print -u2 "notarytool couldn't submit $1"; return 1 }
  state=$(print -r -- "$out" | plutil -extract status raw -o - - 2>/dev/null || true)
  if [[ "$state" != Accepted ]]; then
    id=$(print -r -- "$out" | plutil -extract id raw -o - - 2>/dev/null || true)
    print -u2 "Apple's notary service answered '${state:-nothing}' for $1; \`xcrun notarytool log $id\` (same credentials) says why."
    return 1
  fi
  xcrun stapler staple "$2"
  xcrun stapler validate "$2"
}

# ferb_notarize_app APP WORKDIR: notarization takes a zip of the app; the ticket is stapled to the app itself.
ferb_notarize_app() {
  local zip="$2/FERB-notarize.zip"
  ditto -c -k --keepParent "$1" "$zip"
  ferb_notarize "$zip" "$1"
  rm -f "$zip"
}
