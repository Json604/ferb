# FERB.app's one signing step (L27, L34), sourced by build-app.sh and build-dmg.sh (zsh). Callers set ROOT (the
# checkout) before sourcing it.
#
# SIGN_IDENTITY is one of:
#   -        ad-hoc (the default): development builds and tests. macOS permissions don't carry over to a rebuild.
#   release  FERB's release certificate, for every published build: the code-signing identity whose SHA-1 is in
#            scripts/release-cert.sha1, named "FERB Release Signing", in the owner's login keychain. Each piece macOS
#            grants permissions to gets the designated requirement
#              identifier "<its id>" and certificate leaf = H"<that SHA-1>"
#            so the next version, signed with the same certificate, keeps users' permissions (TCC) and Keychain
#            access and passes the updater's same-certificate check.
#   other    a certificate's name or SHA-1 hash, as `security find-identity -v -p codesigning` lists them. A Developer
#            ID Application certificate also gets a secure timestamp, notarization and a stapled ticket: the same
#            bundle is signed again, never rebuilt (`build-app.sh --sign-only APP`). Notary credentials come only from
#            the environment, are passed to notarytool and are never written to a file or printed:
#              NOTARY_KEYCHAIN_PROFILE                     a profile saved with `xcrun notarytool store-credentials`, or
#              NOTARY_KEY, NOTARY_KEY_ID [, NOTARY_ISSUER]  an App Store Connect API key file, its ID (and issuer).
#
# The release certificate, created once by the owner (the private key never enters the repo):
#   umask 077; D=$(mktemp -d "$HOME/.ferb-release-cert.XXXXXX"); cd "$D"
#   read -rs "P?Passphrase: "; print; export P
#   /usr/bin/openssl req -x509 -newkey rsa:3072 -sha256 -days 7305 -subj "/CN=FERB Release Signing" \
#     -keyout key.pem -passout env:P -out cert.pem -addext "basicConstraints=critical,CA:false" \
#     -addext "keyUsage=critical,digitalSignature" -addext "extendedKeyUsage=critical,codeSigning"
#   /usr/bin/openssl pkcs12 -export -inkey key.pem -passin env:P -in cert.pem -name "FERB Release Signing" \
#     -out "FERB Release Signing.p12" -passout env:P
#   security import "FERB Release Signing.p12" -k "$HOME/Library/Keychains/login.keychain-db" -T /usr/bin/codesign
#   security add-trusted-cert -p codeSign -k "$HOME/Library/Keychains/login.keychain-db" cert.pem
# (the last line trusts the certificate for code signing, for your user only: codesign refuses an untrusted
# self-signed identity with "no identity found"; macOS asks for your login password.) macOS asks for the .p12's passphrase. The .p12 is the backup: keep it in two places you control (for example your
# password manager, next to the passphrase, and an offline USB drive). Then `cd ~; rm -rf "$D"; unset P`. Record the
# public SHA-1 (one line):
#   security find-identity -p codesigning | sed -n 's/.*) \([0-9A-F]\{40\}\) "FERB Release Signing".*/\1/p' \
#     | sort -u | tr 'A-F' 'a-f' > scripts/release-cert.sha1
# On another Mac, restore with the same `security import` of the backup. If it's lost, users grant permissions
# again after the next update, and automatic updates refuse a build signed with another certificate.

: ${SIGN_IDENTITY:=-}
FERB_RELEASE_NAME="FERB Release Signing"

# The release certificate's SHA-1 from scripts/release-cert.sha1, in lowercase; nothing when missing or malformed.
# FERB_TEST_RELEASE_SHA1, when set (even empty), replaces the file: tests only. FERB_SIGN_KEYCHAIN, when set, is the one
# keychain that is searched and signed from (tests with a throwaway certificate; never the owner's login keychain).
ferb_release_sha1() {
  local h=""
  if (( ${+FERB_TEST_RELEASE_SHA1} )); then
    h=$FERB_TEST_RELEASE_SHA1
  elif [[ -f "$ROOT/scripts/release-cert.sha1" ]]; then
    h=$(<"$ROOT/scripts/release-cert.sha1")
  fi
  h=${(L)h//[[:space:]]/}
  if [[ ${#h} == 40 && $h != *[^0-9a-f]* ]]; then print -r -- "$h"; fi
}

# SIGN_IDENTITY resolved once, when this file is sourced, into FERB_SIGN_ID (what codesign gets) and
# FERB_IDENTITY_NAME. Release: exactly the identity with the recorded SHA-1, selected by that hash (never by name, so
# a second certificate with the same name can't sign); SIGN_IDENTITY itself stays "release", so a build script this
# one starts (build-dmg.sh -> build-app.sh) signs the same way. Otherwise the certificate it names, by its common name
# (a hash or a partial name both work, as with codesign); ad-hoc is "-"; an identity the keychain doesn't list stays
# as given.
ferb_resolve_identity() {
  FERB_SIGN_ID="$SIGN_IDENTITY"
  FERB_IDENTITY_NAME="$SIGN_IDENTITY"
  FERB_RELEASE_SHA1=""
  [[ "$SIGN_IDENTITY" == "-" ]] && return 0
  local line
  if [[ "$SIGN_IDENTITY" == release ]]; then
    FERB_RELEASE_SHA1=$(ferb_release_sha1)
    if [[ -z "$FERB_RELEASE_SHA1" ]]; then
      print -u2 "signing.sh: scripts/release-cert.sha1 holds no SHA-1 of \"$FERB_RELEASE_NAME\" (creating it: the top of scripts/signing.sh)"
      return 1
    fi
    # Every code-signing identity, valid or not: macOS never trusts a self-made certificate; codesign signs with it
    line=$(security find-identity -p codesigning ${FERB_SIGN_KEYCHAIN:+"$FERB_SIGN_KEYCHAIN"} 2>/dev/null | grep -i -F -- "$FERB_RELEASE_SHA1" | head -1 || true)
    if [[ "$line" != *"\"$FERB_RELEASE_NAME\""* ]]; then
      print -u2 "signing.sh: no \"$FERB_RELEASE_NAME\" with SHA-1 $FERB_RELEASE_SHA1 in your keychains (restoring the backup: the top of scripts/signing.sh)"
      return 1
    fi
    FERB_SIGN_ID=${(U)FERB_RELEASE_SHA1}
    FERB_IDENTITY_NAME=$FERB_RELEASE_NAME
    return 0
  fi
  line=$(security find-identity -v -p codesigning 2>/dev/null | grep -F -i -- "$SIGN_IDENTITY" | head -1 || true)
  if [[ "$line" == *'"'*'"'* ]]; then FERB_IDENTITY_NAME="${${line#*\"}%\"*}"; fi
  return 0
}
ferb_resolve_identity || return 1

ferb_developer_id() { [[ "$FERB_IDENTITY_NAME" == "Developer ID Application:"* ]] }
ferb_release() { [[ -n "${FERB_RELEASE_SHA1:-}" ]] }

# ferb_requirement ID: what a release-signed piece with identifier ID meets, and pins as its designated requirement
ferb_requirement() { print -r -- "identifier \"$1\" and certificate leaf = H\"$FERB_RELEASE_SHA1\"" }

# ferb_check_release_app [APP]: a disk image is what people download, so it holds only an app signed with FERB's
# release certificate; then the next version keeps their permissions and passes the updater's same-certificate check.
# A Developer ID build is notarized instead. FERB_DEV_DMG=1 lets any app in, for test images only. Without APP only
# the identity is checked (before a build).
ferb_check_release_app() {
  [[ "${FERB_DEV_DMG:-}" == 1 ]] && return 0
  ferb_developer_id && return 0
  if ! ferb_release; then
    print -u2 "A disk image holds only a release-signed FERB.app: set SIGN_IDENTITY=release (FERB_DEV_DMG=1 makes a test image)."
    return 1
  fi
  [[ -z "${1:-}" ]] && return 0
  codesign --verify --strict -R="$(ferb_requirement com.ferb.FERB)" "$1" 2>/dev/null && return 0
  print -u2 "$1 isn't signed with FERB's release certificate ($FERB_RELEASE_SHA1): build it with SIGN_IDENTITY=release."
  return 1
}

ferb_check_notary_credentials() {
  ferb_developer_id || return 0
  [[ -n "${NOTARY_KEYCHAIN_PROFILE:-}" ]] && return 0
  [[ -n "${NOTARY_KEY:-}" && -n "${NOTARY_KEY_ID:-}" ]] && return 0
  print -u2 "A Developer ID build is notarized: set NOTARY_KEYCHAIN_PROFILE (saved with \`xcrun notarytool store-credentials\`), or NOTARY_KEY and NOTARY_KEY_ID (plus NOTARY_ISSUER for a team key)."
  return 1
}

# $1: daemon | python | app; $2: the entitlements file to write. Only what each needs under the hardened runtime.
ferb_entitlements() {
  local -a keys
  case $1 in
    daemon) keys=(com.apple.security.automation.apple-events) ;;     # Automation checks and osascript (permissions.py)
    python) keys=() ;;                                                # the `ferb` command and the native host
    app) keys=(com.apple.security.device.audio-input) ;;              # dictation / push-to-talk microphone
  esac
  # Ad-hoc (or any non-Developer-ID) code has no Team ID, so library validation would refuse every extension module
  # the interpreter loads, and the app's Sparkle.framework (L34 Phase G). One Developer ID team signs all of them, so
  # validation stays on in that build.
  if [[ $1 == (daemon|python|app) ]] && ! ferb_developer_id; then keys+=(com.apple.security.cs.disable-library-validation); fi
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
# a Developer ID (it needs Apple's timestamp server). Release: an item with an identifier (`--identifier ID`, or a
# bundle's CFBundleIdentifier) gets FERB's release requirement as its designated requirement.
ferb_sign() {
  local target=$1; shift
  local stamp=--timestamp=none
  ferb_developer_id && stamp=--timestamp
  local -a req kc
  [[ -n "${FERB_SIGN_KEYCHAIN:-}" ]] && kc=(--keychain "$FERB_SIGN_KEYCHAIN")
  if ferb_release; then
    local id="" i
    for (( i = 1; i < $#; i++ )); do
      if [[ "${@[i]}" == --identifier ]]; then id=${@[i+1]}; fi
    done
    if [[ -z $id && -f "$target/Contents/Info.plist" ]]; then
      id=$(/usr/libexec/PlistBuddy -c 'Print :CFBundleIdentifier' "$target/Contents/Info.plist" 2>/dev/null || true)
    fi
    if [[ -n $id ]]; then req=(--requirements "=designated => $(ferb_requirement "$id")"); fi
  fi
  local log rc=0
  log=$(codesign --force --sign "$FERB_SIGN_ID" --options runtime $stamp $req $kc "$@" "$target" 2>&1) || rc=$?
  print -r -- "$log" | { grep -v ': replacing existing signature$' || true } >&2
  if (( rc != 0 )); then print -u2 "signing.sh: codesign failed for $target"; return $rc; fi
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
  [[ "$FERB_IDENTITY_NAME" == *"($team)" ]] && return 1            # our own team: sign it again
  codesign --verify --strict "$f" 2>/dev/null
}

# ferb_sign_sparkle APP: Sparkle.framework (L34 Phase G), signed inside out as Sparkle documents for apps signed outside
# Xcode: its XPC services (the Downloader keeps its entitlements), Autoupdate, Updater.app, then the framework.
ferb_sign_sparkle() {
  local fw="$1/Contents/Frameworks/Sparkle.framework"
  [[ -d "$fw" ]] || return 0
  local b="$fw/Versions/B"
  if [[ -d "$b/XPCServices/Installer.xpc" ]]; then ferb_sign "$b/XPCServices/Installer.xpc"; fi
  if [[ -d "$b/XPCServices/Downloader.xpc" ]]; then
    ferb_sign "$b/XPCServices/Downloader.xpc" --preserve-metadata=entitlements
  fi
  ferb_sign "$b/Autoupdate"
  ferb_sign "$b/Updater.app"
  ferb_sign "$fw"
}

# ferb_sign_app APP WORKDIR: every nested binary first (deepest first), then nested apps, then the app; then verify.
ferb_sign_app() {
  local app=${1:A} work=$2 f main
  mkdir -p "$work"
  ferb_entitlements daemon "$work/daemon.entitlements"
  ferb_entitlements python "$work/python.entitlements"
  ferb_entitlements app "$work/app.entitlements"
  main="$app/Contents/MacOS/$(/usr/libexec/PlistBuddy -c 'Print :CFBundleExecutable' "$app/Contents/Info.plist" 2>/dev/null || print FERBNotch)"
  local -a files
  files=("${(@f)$(find "$app/Contents" \( -path "$app/Contents/Frameworks" -o -name '*.app' \) -prune -o -type f \( -name '*.so' -o -name '*.dylib' -o -perm -u+x \) -print \
                  | awk -F/ '{ print NF "\t" $0 }' | sort -rn | cut -f2-)}")
  for f in $files; do
    [[ -z $f || $f == $main ]] && continue
    ferb_is_macho "$f" || continue
    ferb_vendor_signed "$f" && continue
    case $f in
      "$app/Contents/Resources/python/bin/python3.12")
        ferb_sign "$f" --identifier com.ferb.python --entitlements "$work/python.entitlements" ;;
      "$app/Contents/Helpers/ferb-secrets")
        ferb_sign "$f" --identifier com.ferb.secrets ;;
      *)
        ferb_sign "$f" ;;
    esac
  done
  ferb_sign_sparkle "$app"
  local nested id
  for nested in "$app"/Contents/Helpers/*.app(N); do
    id=$(/usr/libexec/PlistBuddy -c 'Print :CFBundleIdentifier' "$nested/Contents/Info.plist" 2>/dev/null || true)
    if [[ $id != com.ferb.daemon ]]; then         # the agent app FERB, which runs the daemon, is the only one
      print -u2 "signing.sh: $nested is no piece of FERB.app (id ${id:-none})"; return 1
    fi
    ferb_sign "$nested" --entitlements "$work/daemon.entitlements"
  done
  ferb_sign "$app" --entitlements "$work/app.entitlements"
  codesign --verify --deep --strict --verbose=2 "$app"
  if ferb_release; then             # each piece macOS grants permissions to meets FERB's release requirement
    local piece ident
    for piece ident in "$app" com.ferb.FERB "$app/Contents/Helpers/FERB.app" com.ferb.daemon \
        "$app/Contents/Resources/python/bin/python3.12" com.ferb.python \
        "$app/Contents/Helpers/ferb-secrets" com.ferb.secrets; do
      [[ -e $piece ]] || continue
      codesign --verify --strict -R="$(ferb_requirement $ident)" "$piece" \
        || { print -u2 "signing.sh: $piece doesn't meet FERB's release requirement"; return 1 }
    done
  fi
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
