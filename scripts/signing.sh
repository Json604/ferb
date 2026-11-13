# FERB.app's one signing step (L27, L34), sourced by build-app.sh and build-dmg.sh (zsh). Callers set ROOT (the
# checkout) before sourcing it.
#
# SIGN_IDENTITY is one of:
#   -        ad-hoc (the default): development builds and tests. macOS permissions don't carry over to a rebuild.
#   devid    FERB's Apple Developer ID (T-27), for published builds: the one valid "Developer ID Application: …
#            (<TEAMID>)" identity whose Team ID is in scripts/developer-team.txt, selected by its SHA-1 (never by its
#            name, which is the developer's legal name: it stays out of files and command lines). Each piece keeps
#            codesign's default designated requirement, which names the team, and is verified against
#              identifier "<its id>" and anchor apple generic and certificate leaf[subject.OU] = "<TEAMID>"
#            so every later build of the team keeps users' permissions (TCC) and Keychain access. Like any Developer
#            ID: a secure timestamp, library validation on, then notarization and a stapled ticket (below).
#   release  FERB's self-made release certificate (REL-403's builds; kept until the first Developer ID release passes
#            its live checks): the code-signing identity whose SHA-1 is in scripts/release-cert.sha1, named "FERB
#            Release Signing", in the owner's login keychain. Each piece macOS grants permissions to gets the
#            designated requirement
#              identifier "<its id>" and certificate leaf = H"<that SHA-1>"
#            so the next version, signed with the same certificate, keeps users' permissions (TCC) and Keychain
#            access. (Sparkle 2 accepts an update whose EdDSA signature matches the installed app's SUPublicEDKey, even
#            from another certificate: the certificate pins permissions, not updates; kb/plans/developer-id.md §1.)
#   other    a certificate's name or SHA-1 hash, as `security find-identity -v -p codesigning` lists them (development
#            only: a disk image takes devid or release). A Developer ID Application certificate also gets a secure
#            timestamp, notarization and a stapled ticket.
# A Developer ID build is signed, then notarized; the same bundle can be signed again, never rebuilt
# (`build-app.sh --sign-only APP`). Notary credentials come only from the environment, are passed to notarytool and
# are never written to a file or printed:
#   NOTARY_KEYCHAIN_PROFILE                     a profile saved with `xcrun notarytool store-credentials`, or
#   NOTARY_KEY, NOTARY_KEY_ID [, NOTARY_ISSUER]  an App Store Connect API key file, its ID (and issuer).
# FERB_NOTARIZE=0 signs a Developer ID build without notarizing it (no notary credentials needed, nothing sent to Apple):
# a build for the developer's own Mac, where the team signature alone keeps macOS's permissions from one build to the
# next. Such an app carries no stapled ticket, so a disk image (build-dmg.sh refuses the switch outright) and an appcast
# (ferb_check_devid_app) still refuse it.
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
# again after the next update (the EdDSA key, not this certificate, lets the update install).

: ${SIGN_IDENTITY:=-}
FERB_RELEASE_NAME="FERB Release Signing"

# The Developer ID team from scripts/developer-team.txt (public: every signed binary shows it), in uppercase; nothing
# when missing or malformed. FERB_TEST_TEAM_ID, when set (even empty), replaces the file: tests only.
ferb_team_id() {
  local t=""
  if (( ${+FERB_TEST_TEAM_ID} )); then
    t=$FERB_TEST_TEAM_ID
  elif [[ -f "$ROOT/scripts/developer-team.txt" ]]; then
    t=$(<"$ROOT/scripts/developer-team.txt")
  fi
  t=${(U)t//[[:space:]]/}
  if [[ ${#t} == 10 && $t != *[^0-9A-Z]* ]]; then print -r -- "$t"; fi
}
FERB_TEAM_ID=$(ferb_team_id)          # what a Developer ID piece, app or image is checked against, in any mode

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
# a second certificate with the same name can't sign). Devid: the one valid Developer ID Application identity of the
# recorded team, by its SHA-1 (an Apple Development or Developer ID Installer certificate of the same team never
# matches; two valid ones of the team are refused, not guessed). SIGN_IDENTITY itself stays "release" or "devid", so
# a build script this one starts (build-dmg.sh -> build-app.sh) signs the same way. Otherwise the certificate it
# names, by its common name (a hash or a partial name both work, as with codesign); ad-hoc is "-"; an identity the
# keychain doesn't list stays as given. FERB_IDENTITY_NAME is never printed (a Developer ID's is a legal name).
ferb_resolve_identity() {
  FERB_SIGN_ID="$SIGN_IDENTITY"
  FERB_IDENTITY_NAME="$SIGN_IDENTITY"
  FERB_RELEASE_SHA1=""
  [[ "$SIGN_IDENTITY" == "-" ]] && return 0
  local line
  if [[ "$SIGN_IDENTITY" == devid ]]; then
    if [[ -z "$FERB_TEAM_ID" ]]; then
      print -u2 "signing.sh: scripts/developer-team.txt holds no Team ID (10 letters and digits, from developer.apple.com → Membership)"
      return 1
    fi
    local -a found
    found=(${(f)"$(security find-identity -v -p codesigning ${FERB_SIGN_KEYCHAIN:+"$FERB_SIGN_KEYCHAIN"} 2>/dev/null \
      | grep -F -- "\"Developer ID Application: " | grep -F -- "($FERB_TEAM_ID)\"" \
      | sed -n 's/^ *[0-9]*) \([0-9A-Fa-f]\{40\}\) ".*/\1/p' | tr 'a-f' 'A-F' | sort -u)"})
    if (( ${#found} != 1 )); then
      if (( ${#found} == 0 )); then
        print -u2 "signing.sh: no valid \"Developer ID Application\" identity of team $FERB_TEAM_ID in your keychains (import its .p12 backup into the login keychain)"
      else
        print -u2 "signing.sh: ${#found} valid \"Developer ID Application\" identities of team $FERB_TEAM_ID (${(j:, :)found}): delete the old one from the login keychain"
      fi
      return 1
    fi
    FERB_SIGN_ID=$found[1]
    line=$(security find-identity -v -p codesigning ${FERB_SIGN_KEYCHAIN:+"$FERB_SIGN_KEYCHAIN"} 2>/dev/null | grep -i -F -- "$FERB_SIGN_ID" | head -1 || true)
    FERB_IDENTITY_NAME="${${line#*\"}%\"*}"
    return 0
  fi
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

ferb_developer_id() { [[ "$SIGN_IDENTITY" == devid || "$FERB_IDENTITY_NAME" == "Developer ID Application:"* ]] }
ferb_release() { [[ -n "${FERB_RELEASE_SHA1:-}" ]] }
ferb_devid() { [[ "$SIGN_IDENTITY" == devid ]] }

# ferb_requirement ID: what a release-signed piece with identifier ID meets, and pins as its designated requirement
ferb_requirement() { print -r -- "identifier \"$1\" and certificate leaf = H\"$FERB_RELEASE_SHA1\"" }

# ferb_team_requirement [ID]: what a piece FERB's Developer ID team signed meets (with identifier ID, when given);
# codesign's default designated requirement for that piece implies it
ferb_team_requirement() {
  print -r -- "${1:+identifier \"$1\" and }anchor apple generic and certificate leaf[subject.OU] = \"$FERB_TEAM_ID\""
}

# ferb_check_gatekeeper exec|open PATH [TEAM]: Gatekeeper accepts PATH (an app: exec; a disk image: open, by its own
# signature) as notarized Developer ID code, signed by TEAM when given. The certificate's name (a legal name) is never
# printed.
ferb_check_gatekeeper() {
  local -a ctx
  [[ $1 == open ]] && ctx=(--context context:primary-signature)
  local out rc=0
  out=$(spctl -a -t "$1" $ctx -vv "$2" 2>&1) || rc=$?
  local source=${${(M)${(f)out}:#source=*}#source=}
  if (( rc != 0 )) || [[ "$source" != "Notarized Developer ID" ]]; then
    print -u2 "Gatekeeper doesn't accept $2 as notarized Developer ID code (source: ${source:-none})."
    return 1
  fi
  local origin=${${(M)${(f)out}:#origin=*}#origin=}
  if [[ -n "${3:-}" && "$origin" != "Developer ID Application: "*"($3)" ]]; then
    print -u2 "$2 is notarized, but not as team $3's."
    return 1
  fi
}

# ferb_check_devid_app APP: the app is signed by FERB's Developer ID team, carries its stapled notarization ticket
# and Gatekeeper accepts it as notarized; what a devid disk image and appcast hold
ferb_check_devid_app() {
  if [[ -z "$FERB_TEAM_ID" ]]; then
    print -u2 "scripts/developer-team.txt holds no Team ID to check $1 against."
    return 1
  fi
  if ! codesign --verify --strict -R="$(ferb_team_requirement com.ferb.FERB)" "$1" 2>/dev/null; then
    print -u2 "$1 isn't signed by FERB's Developer ID (team $FERB_TEAM_ID): build it with SIGN_IDENTITY=devid."
    return 1
  fi
  if ! xcrun stapler validate "$1" >/dev/null 2>&1; then
    print -u2 "$1 carries no stapled notarization ticket: build it with SIGN_IDENTITY=devid (it notarizes and staples)."
    return 1
  fi
  ferb_check_gatekeeper exec "$1" "$FERB_TEAM_ID"
}

# ferb_check_release_app [APP]: a disk image is what people download, so it holds only an app signed with FERB's
# Developer ID (devid: team-signed, notarized, stapled) or, until that release passes its live checks, FERB's release
# certificate; then the next version keeps their permissions. FERB_DEV_DMG=1 lets any app in, for test images only.
# Without APP only the identity is checked (before a build).
ferb_check_release_app() {
  [[ "${FERB_DEV_DMG:-}" == 1 ]] && return 0
  if ferb_devid; then
    [[ -z "${1:-}" ]] && return 0
    ferb_check_devid_app "$1"
    return
  fi
  if ! ferb_release; then
    print -u2 "A disk image holds only a FERB.app signed with SIGN_IDENTITY=devid (FERB's Developer ID) or SIGN_IDENTITY=release (FERB_DEV_DMG=1 makes a test image)."
    return 1
  fi
  [[ -z "${1:-}" ]] && return 0
  codesign --verify --strict -R="$(ferb_requirement com.ferb.FERB)" "$1" 2>/dev/null && return 0
  print -u2 "$1 isn't signed with FERB's release certificate ($FERB_RELEASE_SHA1): build it with SIGN_IDENTITY=release."
  return 1
}

# ferb_notarizes: this build is notarized (a Developer ID, unless FERB_NOTARIZE=0 asks for a local build)
ferb_notarizes() { ferb_developer_id && [[ "${FERB_NOTARIZE:-1}" != 0 ]] }

# ferb_notarize_built APP WORKDIR: what a build does after signing: notarize and staple, or say it didn't
ferb_notarize_built() {
  if ferb_notarizes; then
    ferb_notarize_app "$1" "$2"
  elif ferb_developer_id; then
    print "Not notarized (FERB_NOTARIZE=0): a Developer ID build for this Mac only; a disk image or appcast refuses it."
  fi
}

ferb_check_notary_credentials() {
  ferb_notarizes || return 0
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
# bundle's CFBundleIdentifier) gets FERB's release requirement as its designated requirement. Devid: codesign's
# default designated requirement, which already names the team.
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

# ferb_macho_files DIR: every regular file under DIR that file(1) calls Mach-O (by its content, whatever its name or
# mode), one path per line, deepest first. file(1) adds a "PATH (for architecture X):" line per slice of a universal
# file; the file's own line covers it.
ferb_macho_files() {
  find "$1" -type f -print0 | xargs -0 file -N -F $'\t' -- \
    | awk -F'\t' '$1 ~ / \(for architecture [^)]*\):$/ { next }
                  $2 ~ /^ *Mach-O/ { n = split($1, parts, "/"); print n "\t" $1 }' | sort -rn | cut -f2-
}

# ferb_check_signed_code APP: notarization refuses a bundle with one unsigned executable or library, so every Mach-O
# file in it must carry a valid signature, in every mode. In devid mode each is also FERB's team's (or, kept as is,
# another team's Developer ID: ferb_vendor_signed), never ad-hoc or another certificate.
ferb_check_signed_code() {
  local f out bad=0
  for f in "${(@f)$(ferb_macho_files "$1")}"; do
    [[ -z $f ]] && continue
    if ! out=$(codesign --verify --strict "$f" 2>&1); then
      print -u2 "signing.sh: ${f#$1/} is Mach-O code without a valid signature: ${${(f)out}[-1]}"
      bad=1
    elif ferb_devid && ! codesign --verify --strict -R="$(ferb_team_requirement)" "$f" 2>/dev/null \
        && ! ferb_vendor_signed "$f"; then
      print -u2 "signing.sh: ${f#$1/} isn't signed by FERB's Developer ID team ($FERB_TEAM_ID)"
      bad=1
    fi
  done
  return $bad
}

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

# ferb_sign_app APP WORKDIR: every nested Mach-O file first (deepest first; found by content, so no name or mode
# leaves one unsigned), then Sparkle and the nested app, then the app; then verify, and check every Mach-O file.
ferb_sign_app() {
  local app=${1:A} work=$2 f main
  mkdir -p "$work"
  ferb_entitlements daemon "$work/daemon.entitlements"
  ferb_entitlements python "$work/python.entitlements"
  ferb_entitlements app "$work/app.entitlements"
  main="$app/Contents/MacOS/$(/usr/libexec/PlistBuddy -c 'Print :CFBundleExecutable' "$app/Contents/Info.plist" 2>/dev/null || print FERBNotch)"
  for f in "${(@f)$(ferb_macho_files "$app/Contents")}"; do
    [[ -z $f || $f == $main ]] && continue
    # Sparkle and the nested app are signed as bundles below
    [[ $f == "$app/Contents/Frameworks/"* || ${f#$app/Contents/} == *.app/* ]] && continue
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
  ferb_check_signed_code "$app" || return 1
  if ferb_release || ferb_devid; then   # each piece macOS grants permissions to meets FERB's release or team requirement
    local piece ident requirement what=release
    ferb_devid && what="Developer ID team"
    for piece ident in "$app" com.ferb.FERB "$app/Contents/Helpers/FERB.app" com.ferb.daemon \
        "$app/Contents/Resources/python/bin/python3.12" com.ferb.python \
        "$app/Contents/Helpers/ferb-secrets" com.ferb.secrets; do
      [[ -e $piece ]] || continue
      if ferb_devid; then requirement=$(ferb_team_requirement $ident); else requirement=$(ferb_requirement $ident); fi
      codesign --verify --strict -R="$requirement" "$piece" \
        || { print -u2 "signing.sh: $piece doesn't meet FERB's $what requirement"; return 1 }
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
