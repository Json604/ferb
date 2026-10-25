# Packaging Phase B: Stable Signing — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Every published FERB.app is signed with one long-lived, self-made certificate, and each piece macOS grants permissions to pins that certificate in its designated requirement, so users' permissions (TCC) and Keychain access survive updates.

**Architecture:**
- The owner creates the certificate "FERB Release Signing" once and keeps it in the login keychain; its public SHA-1 is committed in `scripts/release-cert.sha1`.
- `SIGN_IDENTITY=release` makes `scripts/signing.sh` sign only with the identity that has exactly that SHA-1, and gives every TCC-relevant piece the explicit designated requirement `identifier "<id>" and certificate leaf = H"<sha1>"`.
- `build-dmg.sh` packs only release-signed apps.
- The installer pins a nested helper to that same certificate when FERB.app carries it.
- A two-build test on a second macOS user proves the grants hold before Phases F and G rely on it.

**Tech Stack:** zsh (`scripts/signing.sh`, `scripts/build-dmg.sh`), `codesign`, `security`, `/usr/bin/openssl` (LibreSSL 3.3.6), Python (`ferb/device/install.py`), pytest.

**Spec:** `docs/superpowers/specs/2026-10-24-ferb-for-everyone-design.md` (L34), section "Phase B — Stable signing".

## Global Constraints

- Owner decision (spec, 2026-10-24): "Free items only; no Developer ID or notarization for now." Spec: "Every release is signed with one stable FERB certificate."
- Spec: "Release builds sign with one long-lived self-made code-signing certificate (not ad-hoc); `signing.sh` documents creating and backing it up. The designated requirement pins that certificate, so TCC grants survive updates."
- Spec: "Verified first with a two-build test on a second macOS user: grant permissions to build N, install build N+1 signed with the same certificate, confirm the grants hold. If TCC doesn't honour it there, stop and report to the owner."
- The private key never enters the repo, a log, a trace or a model provider (CLAUDE.md rule 14). The owner runs every command that touches it; agents only read its public SHA-1.
- Ad-hoc stays the default for development builds and tests. The Developer ID path in `signing.sh` stays as it is (for later).
- Bare minimum, correct behaviour first (owner, 2026-10-25: "focus on correct working").
- Work on `main`; commit by path (other agents have uncommitted work in this checkout); check `git diff --cached` before each commit; commit messages cite L34 and end with `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.
- Never install, quit or replace the running notch or the owner's daemon, and never touch `~/.ferb` on the owner's account. Builds go to `/private/tmp/...` with `nice -n 10`; installs happen only on the second macOS user account.
- After each code task: the focused tests, then `uv run pytest -q`, `uv run mypy ferb/core` and `git diff --check`.
- Order: execute after packaging Phase A (`docs/superpowers/plans/2026-10-25-packaging-a-shrink-two-arch.md`). Tasks 1, 3 and 4 don't depend on it. Task 2 and Task 5 name the one place that differs if Phase A hasn't landed.

## Review Focus

- **Two certificates named "FERB Release Signing"** (an old test one and the real one, or the backup imported into a second keychain): only the identity with the committed SHA-1 signs, never "a" certificate with that name. Pinned by Task 1's `test_release_signing_refuses_a_missing_or_other_certificate_before_signing_anything`.
- **A disk image of an ad-hoc or otherwise signed app made by mistake**: its users would lose their grants on every later update, and the updater would refuse the next real release. `build-dmg.sh` refuses unless `FERB_DEV_DMG=1`. Pinned by Task 2's `test_a_disk_image_holds_only_a_release_signed_app`.
- **Build N+1 (other code hashes, same certificate) installed over build N**: the secret helper's recorded requirement stays byte-for-byte the same, so the Keychain and the helper check keep working with no new setup. Pinned by Task 3's `test_an_update_signed_with_the_same_certificate_keeps_the_pinned_requirements`.
- **Uppercase and lowercase hashes** (`security find-identity` prints uppercase; requirements may print either): compared case-insensitively everywhere. Pinned by Task 3's test with an uppercase app leaf, and by Task 5's real test through `certificate_leaf()`.
- **A helper signed with another certificate, or a loosened requirement (`… or anchor apple`), inside a validly signed app**: refused, never pinned. Pinned by Task 3's `test_bundle_secret_helper_signed_with_ferb_apps_own_certificate_is_pinned_to_that_certificate`.

## Owner questions

- **B1 — A second macOS user account:** the two-build test (Task 6) needs a second user account on your Mac, so that your own FERB, permissions and `~/.ferb` stay untouched. May Task 6 create one named "FERB Test" (Administrator; you choose its password; you can delete it afterwards), or do you already have one to use?

Residual noted, not asked: the release key lives in the login keychain with only `codesign` allowed to use it without a prompt (`-T /usr/bin/codesign`), so any process running as the owner could sign something with it through `codesign`. A separate keychain locked by its own password would close that, at one password per release. Phase B keeps the login keychain, as directed.

## Interfaces this phase produces (Phases F and G rely on these)

- **Signing identity:** a code-signing identity whose certificate is named `FERB Release Signing`. It is self-signed, subject `/CN=FERB Release Signing` only, RSA 3072, key usage Digital Signature, extended key usage Code Signing, `CA:false`, valid 7305 days (to 2046). macOS lists it as `CSSMERR_TP_NOT_TRUSTED`, which is expected and harmless; it is never marked trusted.
- **Keychain location:** the owner's login keychain, `~/Library/Keychains/login.keychain-db`, with `/usr/bin/codesign` in the key's access list. The owner's backup is the passphrase-protected `FERB Release Signing.p12`, kept outside the repo.
- **Pinned hash:** `scripts/release-cert.sha1` holds the certificate's SHA-1, 40 lowercase hex digits and a newline. It is committed in Task 4 and public (every signed app carries the certificate).
- **How `scripts/signing.sh` is called:** a build script sets `ROOT` (the checkout), exports `SIGN_IDENTITY`, then runs `source "$ROOT/scripts/signing.sh"`.
  - With `SIGN_IDENTITY=release`, sourcing checks the keychain. It sets `FERB_RELEASE_SHA1` (lowercase), `FERB_SIGN_ID` (the same SHA-1 in uppercase, what codesign gets, so it selects exactly that identity) and `FERB_IDENTITY_NAME="FERB Release Signing"`. `SIGN_IDENTITY` stays `release`, so build scripts started from it sign the same way. A missing or other certificate fails the `source` (status 1) with a message.
  - In every mode, `FERB_SIGN_ID` is the identity codesign gets: `-`, the release SHA-1, or the given name/hash.
  - `ferb_release` succeeds only in release mode.
  - `ferb_requirement ID` prints `identifier "ID" and certificate leaf = H"<sha1>"`.
  - `ferb_sign PATH [codesign options…]` signs one item inside-out. In release mode it adds `--requirements "=designated => $(ferb_requirement ID)"` when the options hold `--identifier ID` or PATH is a bundle with a `CFBundleIdentifier`. Libraries without an identifier keep codesign's own requirement.
  - `ferb_sign_app APP WORKDIR` signs everything in FERB.app deepest first, then verifies, and in release mode checks the five pieces below with `codesign --verify --strict -R=…`.
  - `ferb_check_release_app [APP]` is the disk-image gate.
  - Command lines:
    - `SIGN_IDENTITY=release scripts/build-app.sh [--arch arm64|x86_64] OUT_DIR`;
    - `SIGN_IDENTITY=release scripts/build-app.sh --sign-only APP` re-signs without rebuilding;
    - `SIGN_IDENTITY=release scripts/build-dmg.sh` makes both images (Phase A);
    - `FERB_DEV_DMG=1` makes a test image of any app.
- **Designated requirements** (`codesign -d -r- <piece>` prints `designated => ` followed by each):
  - `FERB.app`: `identifier "com.ferb.FERB" and certificate leaf = H"<sha1>"`
  - `Contents/Helpers/FERB.app` (the agent that runs the daemon): `identifier "com.ferb.daemon" and certificate leaf = H"<sha1>"`
  - `Contents/Helpers/FERB Voice.app`: `identifier "com.ferb.app" and certificate leaf = H"<sha1>"`
  - `Contents/Resources/python/bin/python3.12`: `identifier "com.ferb.python" and certificate leaf = H"<sha1>"`
  - `Contents/Helpers/ferb-secrets`: `identifier "com.ferb.secrets" and certificate leaf = H"<sha1>"`
- **For Phase G (Sparkle):**
  - Sparkle checks a downloaded version against the running app's designated requirement, which is the `com.ferb.FERB` string above.
  - Sparkle.framework and its `Autoupdate`, `Updater.app`, `Downloader.xpc` and `Installer.xpc` must be signed with `ferb_sign`, inside-out, before `ferb_sign "$app"`. `ferb_sign_app`'s walk prunes every `*.app` and seals no `*.xpc` or `*.framework` bundle, so Phase G extends it.
  - Release-signed code has no Team ID. Expect the main app to need `com.apple.security.cs.disable-library-validation` (`ferb_entitlements app`) once it loads Sparkle.framework, as the daemon and Python already have for the same reason. Phase G verifies this.
- **For Phase F (`SMAppService.mainApp`):** the app's signature and requirement are the `com.ferb.FERB` string above and stay the same across releases. Whether macOS accepts the registration without a Developer ID is Phase F's check.

## Files

- Modify `scripts/signing.sh`: release mode, the explicit requirements, the disk-image gate, and the certificate doc in its header.
- Modify `scripts/build-dmg.sh`: refuse apps that aren't release-signed.
- Create `scripts/release-cert.sha1`: the certificate's public SHA-1 (Task 4, after the owner creates it).
- Modify `ferb/device/install.py`: `certificate_leaf()`; `sealed_requirement` accepts FERB.app's own certificate.
- Modify `tests/support/app_bundle.py`: `BundleRunner(app_requirement=…)`.
- Modify tests: `tests/test_packaging.py` and `tests/test_install.py`.
- Modify docs: `README.md`, `ARCHITECTURE.md`, `CHANGELOG.md`, `docs/OWNER_ACCEPTANCE.md`, `docs/0.27_DECISIONS.toml`.

---

### Task 1: Release mode in `signing.sh`

**Files:**
- Modify: `scripts/signing.sh:1-25` (header, identity resolution), `:56-64` (`ferb_sign`), `:114-116` (end of `ferb_sign_app`)
- Test: `tests/test_packaging.py`

**Interfaces:**
- Consumes: the stub tools `_stubs`, `_fake_app`, `_sign_only`, `_signed` and `_ent` (existing, `tests/test_packaging.py:61-146`).
- Produces: `FERB_RELEASE_NAME`, `ferb_release_sha1`, `ferb_release`, `ferb_requirement ID`, the release behaviour of `ferb_resolve_identity`, `ferb_sign` and `ferb_sign_app`, and the test constants `SHA` and `RELEASE_LISTED` plus `_release_env()`.

- [ ] **Step 1: Write the failing tests**

Add to `tests/test_packaging.py`, after `test_developer_id_without_notary_credentials_stops_before_signing`:

```python
SHA = "0123456789abcdef0123456789abcdef01234567"
RELEASE_LISTED = f'  1) {SHA.upper()} "FERB Release Signing" (CSSMERR_TP_NOT_TRUSTED)'
RELEASE_PIECES = (("", "com.ferb.FERB"), ("Contents/Helpers/FERB.app", "com.ferb.daemon"),
                  ("Contents/Helpers/FERB Voice.app", "com.ferb.app"),
                  ("Contents/Resources/python/bin/python3.12", "com.ferb.python"),
                  ("Contents/Helpers/ferb-secrets", "com.ferb.secrets"))


def _release_env() -> dict[str, str]:
    """SIGN_IDENTITY=release with the stub keychain listing FERB's release certificate (self-made: never trusted)."""
    return {"SIGN_IDENTITY": "release", "FERB_TEST_RELEASE_SHA1": SHA, "STUB_IDENTITIES": RELEASE_LISTED}


def test_release_signing_uses_only_the_pinned_certificate_and_pins_it_in_each_requirement(tmp_path: Path) -> None:
    app, calls, done = _sign_only(tmp_path, **_release_env())
    assert done.returncode == 0, done.stderr + done.stdout
    sign_calls = [c for c in calls if "|--sign|" in c and "--verify" not in c]
    assert sign_calls and all(f"|--sign|{SHA.upper()}|" in c and "|--timestamp=none|" in c for c in sign_calls)
    for rel, ident in RELEASE_PIECES:
        piece = app / rel if rel else app
        call = next(c for c in sign_calls if c.endswith(f"|{piece}"))
        assert f'|--requirements|=designated => identifier "{ident}" and certificate leaf = H"{SHA}"|' in call, call
        assert f'codesign|--verify|--strict|-R=identifier "{ident}" and certificate leaf = H"{SHA}"|{piece}' in calls
    lib = next(c for c in sign_calls if c.endswith("libpython3.12.dylib"))
    assert "--requirements" not in lib                                  # libraries keep codesign's own requirement
    assert not any("vendor-tool" in c for c in sign_calls)              # another team's signature is kept
    assert "com.apple.security.cs.disable-library-validation" in _ent(tmp_path, app / "Contents/Helpers/FERB.app")
    assert not any(c.startswith("xcrun") for c in calls)                # a self-made certificate is never notarized


def test_release_signing_refuses_a_missing_or_other_certificate_before_signing_anything(tmp_path: Path) -> None:
    cases = ({**_release_env(), "STUB_IDENTITIES": ""},                                        # not in the keychain
             {**_release_env(), "STUB_IDENTITIES": f'  1) {"F" * 40} "FERB Release Signing"'},  # same name, other one
             {**_release_env(), "FERB_TEST_RELEASE_SHA1": ""},                                  # no recorded SHA-1
             {**_release_env(), "FERB_TEST_RELEASE_SHA1": "not-a-sha1"})
    for i, env in enumerate(cases):
        (tmp_path / str(i)).mkdir()
        _, calls, done = _sign_only(tmp_path / str(i), **env)
        assert done.returncode != 0 and "FERB Release Signing" in done.stderr, (i, done.stderr)
        assert not _signed(calls), i


def test_release_mode_stays_release_for_the_build_scripts_it_starts(tmp_path: Path) -> None:
    """build-dmg.sh starts build-app.sh: SIGN_IDENTITY must still say `release` there, not a bare hash that would skip
    the pinned requirements; codesign gets the hash through FERB_SIGN_ID."""
    env = {**_stubs(tmp_path), **_release_env(), "ROOT": str(ROOT)}
    done = subprocess.run(["/bin/zsh", "-f", "-c",
                           'source "$ROOT/scripts/signing.sh" && print -r -- "$SIGN_IDENTITY $FERB_SIGN_ID"'],
                          capture_output=True, text=True, env=env)
    assert done.stdout.strip() == f"release {SHA.upper()}", done.stderr
```

- [ ] **Step 2: Run them to verify they fail**

Run: `uv run pytest tests/test_packaging.py -k "release_signing or release_mode" -v`
Expected: FAIL. Today `release` is passed to codesign as an identity name: the stub signs with `--sign|release|`, nothing is refused, and `FERB_SIGN_ID` is empty.

- [ ] **Step 3: Rewrite the top of `scripts/signing.sh` (lines 1-25)**

Replace lines 1-25 (from the first comment line through `ferb_developer_id() { … }`) with:

```zsh
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
# macOS asks for the .p12's passphrase. The .p12 is the backup: keep it in two places you control (for example your
# password manager, next to the passphrase, and an offline USB drive). Then `cd ~; rm -rf "$D"; unset P`. Record the
# public SHA-1 (one line):
#   security find-identity -p codesigning | sed -n 's/.*) \([0-9A-F]\{40\}\) "FERB Release Signing".*/\1/p' \
#     | sort -u | tr 'A-F' 'a-f' > scripts/release-cert.sha1
# On another Mac, restore with the same `security import` of the backup. If it's lost, users grant permissions
# again after the next update, and automatic updates refuse a build signed with another certificate.

: ${SIGN_IDENTITY:=-}
FERB_RELEASE_NAME="FERB Release Signing"

# The release certificate's SHA-1 from scripts/release-cert.sha1, in lowercase; nothing when missing or malformed.
# FERB_TEST_RELEASE_SHA1, when set (even empty), replaces the file: tests only.
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
    line=$(security find-identity -p codesigning 2>/dev/null | grep -i -F -- "$FERB_RELEASE_SHA1" | head -1 || true)
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
```

- [ ] **Step 4: Pin the requirement in `ferb_sign`**

Replace `ferb_sign` (original lines 56-64) with:

```zsh
# ferb_sign PATH [codesign options…]: hardened runtime always (notarization requires it); a secure timestamp only for
# a Developer ID (it needs Apple's timestamp server). Release: an item with an identifier (`--identifier ID`, or a
# bundle's CFBundleIdentifier) gets FERB's release requirement as its designated requirement.
ferb_sign() {
  local target=$1; shift
  local stamp=--timestamp=none
  ferb_developer_id && stamp=--timestamp
  local -a req
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
  codesign --force --sign "$FERB_SIGN_ID" --options runtime $stamp $req "$@" "$target" 2>&1 \
    | { grep -v ': replacing existing signature$' || true } >&2
}
```

(`build-dmg.sh` keeps `codesign --force --sign "$SIGN_IDENTITY" --timestamp "$dmg"` for a Developer ID image; there `SIGN_IDENTITY` is the Developer ID itself, which is the same as `FERB_SIGN_ID`.)

- [ ] **Step 5: Check the five pieces at the end of `ferb_sign_app`**

In `ferb_sign_app`, after the final `codesign --verify --deep --strict --verbose=2 "$app"` line and before the closing `}`, add:

```zsh
  if ferb_release; then             # each piece macOS grants permissions to meets FERB's release requirement
    local piece ident
    for piece ident in "$app" com.ferb.FERB "$app/Contents/Helpers/FERB.app" com.ferb.daemon \
        "$app/Contents/Helpers/FERB Voice.app" com.ferb.app \
        "$app/Contents/Resources/python/bin/python3.12" com.ferb.python \
        "$app/Contents/Helpers/ferb-secrets" com.ferb.secrets; do
      [[ -e $piece ]] || continue
      codesign --verify --strict -R="$(ferb_requirement $ident)" "$piece" \
        || { print -u2 "signing.sh: $piece doesn't meet FERB's release requirement"; return 1 }
    done
  fi
```

- [ ] **Step 6: Run the tests**

Run: `uv run pytest tests/test_packaging.py -v`
Expected: the two new tests PASS. The existing ad-hoc, Developer ID, other-certificate and DMG tests still PASS: ad-hoc and named identities behave as before.

- [ ] **Step 7: Full checks and commit**

Run: `uv run pytest -q && uv run mypy ferb/core && git diff --check`

```bash
git add scripts/signing.sh tests/test_packaging.py
git diff --cached --stat
git commit -m "feat(L34): SIGN_IDENTITY=release signs only with FERB's recorded certificate and pins it in each piece's designated requirement (packaging B, task 1)

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 2: A disk image holds only a release-signed app

**Files:**
- Modify: `scripts/signing.sh` (add `ferb_check_release_app` after `ferb_requirement`)
- Modify: `scripts/build-dmg.sh` (header; two calls)
- Test: `tests/test_packaging.py` (`STUB_CODESIGN`, `_stubs`, a new test)

**Interfaces:**
- Consumes: `ferb_release`, `ferb_requirement`, `ferb_developer_id` (Task 1); `SHA`, `RELEASE_LISTED`, `_release_env` (Task 1).
- Produces: `ferb_check_release_app [APP]`, which returns 0 when `FERB_DEV_DMG=1`, for a Developer ID identity, or in release mode when APP is absent or meets `ferb_requirement com.ferb.FERB`. Otherwise it prints why and returns 1.

- [ ] **Step 1: Teach the stub codesign to fail a verify, and default test images to dev**

In `tests/test_packaging.py`, in `STUB_CODESIGN`, add after the `print -r -- "codesign|…" >> "$STUB_LOG"` line:

```zsh
if [[ " $* " == *" --verify "* && -n "${STUB_VERIFY_FAILS:-}" ]]; then exit 1; fi
```

In `_stubs`, add `"FERB_DEV_DMG": "1"` to the returned dict. Every existing image test makes a test image.

- [ ] **Step 2: Write the failing test**

```python
def test_a_disk_image_holds_only_a_release_signed_app(tmp_path: Path) -> None:
    """What people download carries FERB's release certificate (L34), so their permissions and automatic updates carry
    over to the next version; an ad-hoc or otherwise signed app is refused unless FERB_DEV_DMG=1 (test images)."""
    app = _fake_app(tmp_path / "out")
    dmg = tmp_path / "out" / "FERB.dmg"
    env = _stubs(tmp_path)
    del env["FERB_DEV_DMG"]
    adhoc = subprocess.run([str(BUILD_DMG), str(app), str(dmg)], capture_output=True, text=True, env=env)
    assert adhoc.returncode != 0 and "SIGN_IDENTITY=release" in adhoc.stderr and not dmg.exists()
    env.update(_release_env())
    ok = subprocess.run([str(BUILD_DMG), str(app), str(dmg)], capture_output=True, text=True, env=env)
    assert ok.returncode == 0, ok.stderr + ok.stdout
    calls = (tmp_path / "calls.log").read_text().splitlines()
    assert f'codesign|--verify|--strict|-R=identifier "com.ferb.FERB" and certificate leaf = H"{SHA}"|{app}' in calls
    dmg.unlink()
    env["STUB_VERIFY_FAILS"] = "1"                                          # the app is signed some other way
    other = subprocess.run([str(BUILD_DMG), str(app), str(dmg)], capture_output=True, text=True, env=env)
    assert other.returncode != 0 and "isn't signed with FERB's release certificate" in other.stderr
    assert not dmg.exists()
```

- [ ] **Step 3: Run it to verify it fails**

Run: `uv run pytest tests/test_packaging.py::test_a_disk_image_holds_only_a_release_signed_app -v`
Expected: FAIL. The ad-hoc image is made (return code 0).

- [ ] **Step 4: Add the gate to `scripts/signing.sh`**

After the `ferb_requirement` line, add:

```zsh
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
```

- [ ] **Step 5: Call it from `scripts/build-dmg.sh`**

In Phase A's `scripts/build-dmg.sh`:
- In `make_dmg`, right after `local app=${1:A} dmg=${2:A}`, add `ferb_check_release_app "$app" || exit 1`.
- Right after `if (( $# == 0 )); then`, add `  ferb_check_release_app || exit 1                # before the builds, not after`.
- In the header, replace "Ad-hoc by default; with a Developer ID SIGN_IDENTITY each image is also signed, notarized and stapled (scripts/signing.sh)." with "An image holds only a release-signed FERB.app (SIGN_IDENTITY=release) or a Developer ID one, which is also signed, notarized and stapled (scripts/signing.sh); FERB_DEV_DMG=1 makes a test image of any app."

If Phase A hasn't landed (the single-image script), add `ferb_check_release_app "$APP" || exit 1` right after its line `APP=${APP:A}` instead, and change the same header sentence.

- [ ] **Step 6: Run the tests**

Run: `uv run pytest tests/test_packaging.py -v`
Expected: PASS, including Phase A's DMG tests (they run with `FERB_DEV_DMG=1` from `_stubs`) and the Developer ID image test.

- [ ] **Step 7: Full checks and commit**

Run: `uv run pytest -q && uv run mypy ferb/core && git diff --check`

```bash
git add scripts/signing.sh scripts/build-dmg.sh tests/test_packaging.py
git diff --cached --stat
git commit -m "feat(L34): a disk image holds only a FERB.app signed with the release certificate; FERB_DEV_DMG=1 for test images (packaging B, task 2)

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 3: The installer pins helpers to FERB.app's own certificate

Today `Installer.sealed_requirement` (`ferb/device/install.py:419-438`) accepts only a code-hash requirement (ad-hoc) or a Developer ID team. A release-signed FERB.app's `ferb-secrets` and `FERB Voice.app` carry `identifier "…" and certificate leaf = H"…"`, so setup would refuse them ("pinned by neither…"). This task makes setup accept them.

**Files:**
- Modify: `ferb/device/install.py` (new `certificate_leaf` after `trusted_requirement`; `sealed_requirement`)
- Modify: `tests/support/app_bundle.py` (`BundleRunner`)
- Test: `tests/test_install.py`

**Interfaces:**
- Produces: `certificate_leaf(requirement: str) -> str | None` returns the lowercase SHA-1 when the requirement is exactly `identifier "<id>" and certificate leaf = H"<40 hex>"`, else None. `BundleRunner(…, app_requirement: str | None = None)` answers `codesign -d -r-` on the app bundle itself with `app_requirement`.

- [ ] **Step 1: Let the fake runner give the app its own requirement**

In `tests/support/app_bundle.py`, change `BundleRunner`:

```python
class BundleRunner(FakeRunner):
    """FakeRunner, plus codesign answers for FERB.app's code and copies of it: code hash `cdhash` and designated
    requirement `requirement` (ad-hoc by default), the app bundle's own requirement `app_requirement` (None: the same
    `requirement`), the app's Team ID `team` (None: ad-hoc); `bad_signature` makes `codesign --verify` fail; the
    linked Codex reports `codex_version`."""

    def __init__(self, home: Path, bad_signature: bool = False, codex_version: str = "", cdhash: str = CDHASH,
                 requirement: str = PINNED, team: str | None = None, app_requirement: str | None = None) -> None:
        super().__init__(home)
        self.bad_signature, self.codex_version = bad_signature, codex_version
        self.cdhash, self.requirement, self.team = cdhash, requirement, team
        self.app_requirement = app_requirement

    def __call__(self, cmd: list[str]) -> "subprocess.CompletedProcess[str]":
        if cmd[0].endswith("codesign") and "-r-" in cmd:
            self.cmds.append(cmd)
            is_app = Path(cmd[-1]).name == "FERB.app" and "/Contents/" not in cmd[-1]   # not the nested agent
            requirement = self.app_requirement if is_app and self.app_requirement is not None else self.requirement
            prefix = "# " if requirement.startswith("cdhash") else ""
            return subprocess.CompletedProcess(cmd, 0, "", f"Executable={cmd[-1]}\n{prefix}designated => "
                                                           f"{requirement}\n")
```

(The rest of `__call__` stays as it is.)

- [ ] **Step 2: Write the failing tests**

Add to `tests/test_install.py`, after `test_bundle_secret_helper_with_a_developer_id_must_carry_the_apps_own_team`:

```python
LEAF = "0123456789abcdef0123456789abcdef01234567"


def _release(identifier: str, leaf: str = LEAF) -> str:
    """The designated requirement scripts/signing.sh gives a release-signed piece (L34)."""
    return f'identifier "{identifier}" and certificate leaf = H"{leaf}"'


def test_bundle_secret_helper_signed_with_ferb_apps_own_certificate_is_pinned_to_that_certificate(
        tmp_path: Path) -> None:
    """FERB's release signing (L34): the helper's requirement names FERB.app's own certificate, so the next version,
    signed with the same certificate, still meets it."""
    inst = bundled(tmp_path, requirement=_release("com.ferb.secrets"),
                   app_requirement=_release("com.ferb.FERB", LEAF.upper()))          # case doesn't matter
    inst.install_secret_helper()
    assert (inst.home / "bin" / "ferb-secrets.requirement").read_text().strip() == _release("com.ferb.secrets")
    refused = (_release("com.ferb.secrets", "f" * 40),                              # another certificate
               _release("com.ferb.secrets") + " or anchor apple",                   # anything added to the form
               f'certificate leaf = H"{LEAF}"')                                      # no identifier
    for i, requirement in enumerate(refused):
        inst = bundled(tmp_path / str(i), requirement=requirement, app_requirement=_release("com.ferb.FERB"))
        with pytest.raises(InstallError, match="neither"):
            inst.install_secret_helper()
    inst = bundled(tmp_path / "adhoc", requirement=_release("com.ferb.secrets"), app_requirement=PINNED)
    with pytest.raises(InstallError, match="neither"):                              # the app carries no certificate
        inst.install_secret_helper()


def test_an_update_signed_with_the_same_certificate_keeps_the_pinned_requirements(tmp_path: Path) -> None:
    """Build N+1 replaces build N in place: other code hashes, the same certificate. The recorded requirement is
    unchanged, so the secret helper's check (ferb/identity/helper.py) and its Keychain access carry over."""
    import shutil
    inst = bundled(tmp_path, requirement=_release("com.ferb.secrets"), app_requirement=_release("com.ferb.FERB"))
    inst.install()
    pinned = (inst.home / "bin" / "ferb-secrets.requirement").read_text()
    assert inst.bundle is not None
    shutil.rmtree(inst.bundle)                                                       # the owner replaced FERB.app
    newer = "9" * 40
    make_ferb_app(inst.bundle.parent, build="202610260900", sealed=newer)
    inst.run.cdhash = newer  # type: ignore[attr-defined]
    inst.install(update_only=True)
    assert (inst.home / "bin" / "ferb-secrets.requirement").read_text() == pinned
```

- [ ] **Step 3: Run them to verify they fail**

Run: `uv run pytest tests/test_install.py -k "own_certificate or same_certificate" -v`
Expected: FAIL with `InstallError: ferb-secrets is pinned by neither its exact code hash nor FERB.app's Developer ID team`.

- [ ] **Step 4: Implement**

In `ferb/device/install.py`, after `trusted_requirement` (line 103), add:

```python
def certificate_leaf(requirement: str) -> str | None:
    """The certificate a requirement of exactly FERB's release form pins (scripts/signing.sh: `identifier "…" and
    certificate leaf = H"<SHA-1>"`), as a lowercase SHA-1; None for any other requirement."""
    m = re.fullmatch(r'identifier "[^"]+" and certificate leaf = H"([0-9a-fA-F]{40})"', requirement.strip())
    return m.group(1).lower() if m else None
```

In `sealed_requirement`, replace the docstring's last two sentences and the tail of the function (from `if requirement == f'cdhash H"{sealed}"':` to the `raise`) so the function reads:

```python
    def sealed_requirement(self, code: Path, nested: str) -> str:
        """The requirement to pin for `code`, a copy of FERB.app's nested `nested` (check_bundle verified the app's own
        signature first): only if it is exactly the code that signature seals (the same code hash), and then its
        code hash (ad-hoc), FERB's release requirement naming the very certificate FERB.app carries (L34), or a
        Developer ID requirement naming FERB.app's own team. A merely valid signature, another certificate or
        another team is refused."""
        assert self.bundle is not None
        sealed = sealed_cdhash(self.bundle, nested)
        shown = self.run(['/usr/bin/codesign', '-dv', '--verbose=4', str(code)])
        if not sealed or codesign_field((shown.stdout or '') + (shown.stderr or ''), 'CDHash') != sealed:
            raise InstallError(f"{code.name} isn't the one FERB.app's signature seals; download FERB again")
        shown = self.run(['/usr/bin/codesign', '-d', '-r-', str(code)])
        requirement = designated_requirement((shown.stdout or '') + (shown.stderr or ''))
        app = self.run(['/usr/bin/codesign', '-dv', str(self.bundle)])
        team = codesign_field((app.stdout or '') + (app.stderr or ''), 'TeamIdentifier')
        if requirement == f'cdhash H"{sealed}"':
            return requirement
        own = self.run(['/usr/bin/codesign', '-d', '-r-', str(self.bundle)])
        leaf = certificate_leaf(designated_requirement((own.stdout or '') + (own.stderr or '')))
        if leaf is not None and certificate_leaf(requirement) == leaf:
            return requirement
        if (team and team != 'not set' and 'anchor apple generic' in requirement
                and re.search(rf'certificate leaf\[subject\.OU\] = "?{re.escape(team)}"?(\s|\)|$)', requirement)):
            return requirement
        raise InstallError(f"{code.name} is pinned by neither its exact code hash nor FERB.app's own certificate "
                           "or Developer ID team")
```

- [ ] **Step 5: Run the tests**

Run: `uv run pytest tests/test_install.py tests/test_setup_flow.py tests/test_secret_helper.py -q`
Expected: PASS. The existing ad-hoc and Developer ID tests still match "neither" and their pins.

- [ ] **Step 6: Full checks and commit**

Run: `uv run pytest -q && uv run mypy ferb/core && git diff --check`

```bash
git add ferb/device/install.py tests/support/app_bundle.py tests/test_install.py
git diff --cached --stat
git commit -m "feat(L34): setup pins FERB.app's helpers to the release certificate the app itself carries, so updates keep the pins (packaging B, task 3)

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 4: The owner creates the certificate; its SHA-1 is recorded

**Files:**
- Create: `scripts/release-cert.sha1`
- Test: `tests/test_packaging.py`

- [ ] **Step 1: Write the failing test**

```python
def test_the_release_certificates_sha1_is_recorded() -> None:
    """scripts/release-cert.sha1 names FERB's one release certificate (L34): public, 40 lowercase hex digits."""
    text = (ROOT / "scripts" / "release-cert.sha1").read_text()
    assert len(text) == 41 and text.endswith("\n") and all(ch in "0123456789abcdef" for ch in text[:40])
```

Run: `uv run pytest tests/test_packaging.py::test_the_release_certificates_sha1_is_recorded -v`
Expected: FAIL with `FileNotFoundError`.

- [ ] **Step 2: Owner steps (the owner types these in Terminal; an agent never runs them or sees the passphrase)**

```zsh
# 1. A private folder for the few minutes the files exist (the key file is encrypted with your passphrase even there)
umask 077
D=$(mktemp -d "$HOME/.ferb-release-cert.XXXXXX") && cd "$D"
# 2. A passphrase for the key and its backup: typed twice, never shown, not saved in shell history
read -rs "P?Passphrase for FERB's release key: "; print
read -rs "P2?The same passphrase again: "; print
if [[ -n "$P" && "$P" == "$P2" ]]; then export P; unset P2; print OK; else print "Empty or different: repeat step 2"; fi
# 3. The certificate: self-made, code signing only, valid 20 years (7305 days), a name and nothing else
/usr/bin/openssl req -x509 -newkey rsa:3072 -sha256 -days 7305 -subj "/CN=FERB Release Signing" \
  -keyout key.pem -passout env:P -out cert.pem \
  -addext "basicConstraints=critical,CA:false" -addext "keyUsage=critical,digitalSignature" \
  -addext "extendedKeyUsage=critical,codeSigning"
# 4. Key and certificate in one passphrase-protected file: this is also the backup
/usr/bin/openssl pkcs12 -export -inkey key.pem -passin env:P -in cert.pem -name "FERB Release Signing" \
  -out "FERB Release Signing.p12" -passout env:P
# 5. Into your login keychain; only codesign may use the key. macOS asks for the .p12's passphrase in a window.
security import "FERB Release Signing.p12" -k "$HOME/Library/Keychains/login.keychain-db" -T /usr/bin/codesign
# 6. Show the .p12 in Finder, then copy it to two places you control: your password manager (attach it to an item
#    that also holds the passphrase) and a USB drive you keep offline
open -R "$D/FERB Release Signing.p12"
```

When both copies exist:

```zsh
# 7. Remove the working folder (key.pem, cert.pem and this copy of the .p12) and the passphrase from the shell
cd ~ && rm -rf "${D:?}" && unset P D
```

- [ ] **Step 3: Record and check the SHA-1 (agent or owner; reads only public data)**

```zsh
cd ~/Desktop/FERB
security find-identity -p codesigning | grep -c '"FERB Release Signing"'
security find-identity -p codesigning | sed -n 's/.*) \([0-9A-F]\{40\}\) "FERB Release Signing".*/\1/p' \
  | sort -u | tr 'A-F' 'a-f' > scripts/release-cert.sha1
wc -l < scripts/release-cert.sha1
security find-certificate -c "FERB Release Signing" -p | /usr/bin/openssl x509 -noout -subject -enddate
fp=$(security find-certificate -c "FERB Release Signing" -p | /usr/bin/openssl x509 -noout -fingerprint -sha1 \
  | sed 's/.*=//; s/://g' | tr 'A-F' 'a-f')
[[ "$fp" == "$(<scripts/release-cert.sha1)" ]] && print MATCH
```

Expected:
- `1` from the count, then `1` from `wc -l`.
- `subject= /CN=FERB Release Signing` and a `notAfter=` date in 2046.
- `MATCH`.

If the count is 2 or more, an older certificate with that name exists. Ask the owner which one is the release certificate, delete the other in Keychain Access, and run this step again.

- [ ] **Step 4: One signing probe (the owner watches: macOS may ask once)**

```zsh
cp /usr/bin/true /private/tmp/ferb-sign-probe
sha=$(<scripts/release-cert.sha1)
codesign --force --sign "${(U)sha}" --requirements "=designated => identifier \"probe\" and certificate leaf = H\"$sha\"" \
  /private/tmp/ferb-sign-probe
codesign -d -r- /private/tmp/ferb-sign-probe
rm -f /private/tmp/ferb-sign-probe
```

Expected: if macOS shows "codesign wants to sign using key "FERB Release Signing"", the owner clicks **Always Allow** and enters the login password. The output is `designated => identifier "probe" and certificate leaf = H"<the same SHA-1>"`. Record the letter case codesign printed the hash in.

- [ ] **Step 5: Run the test, and commit only the public hash**

Run: `uv run pytest tests/test_packaging.py -q`
Expected: PASS.

```bash
git add scripts/release-cert.sha1 tests/test_packaging.py
git diff --cached
git commit -m "build(L34): record the public SHA-1 of FERB's release signing certificate (packaging B, task 4)

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

`git diff --cached` must show exactly one 40-character line in `scripts/release-cert.sha1` and the test: no key, no `.p12`, no passphrase.

---

### Task 5: A real release-signed build

**Files:**
- Test: `tests/test_packaging.py`

- [ ] **Step 1: Write the real test**

It uses Phase A's `real_bundle` fixture. If Phase A hasn't landed, add this fixture below `_real_bundle` first:

```python
@pytest.fixture(scope="module")
def real_bundle(tmp_path_factory: pytest.TempPathFactory) -> Path:
    """One built FERB.app for every real-bundle test (FERB_APP_BUNDLE, or FERB_BUILD_APP=1 builds it once)."""
    return _real_bundle(tmp_path_factory)
```

Then add:

```python
def test_real_release_bundle_pins_ferbs_certificate_in_every_piece_macos_grants_to(real_bundle: Path,
                                                                                  tmp_path: Path) -> None:
    if os.environ.get("FERB_RELEASE_BUNDLE") != "1":
        pytest.skip("set FERB_RELEASE_BUNDLE=1 with FERB_APP_BUNDLE pointing at a SIGN_IDENTITY=release build")
    from ferb.device.install import Installer, certificate_leaf, designated_requirement
    sha = (ROOT / "scripts" / "release-cert.sha1").read_text().strip()
    for rel, ident in RELEASE_PIECES:
        piece = real_bundle / rel if rel else real_bundle
        shown = subprocess.run(["codesign", "-d", "-r-", str(piece)], capture_output=True, text=True)
        requirement = designated_requirement(shown.stdout + shown.stderr)
        assert requirement.startswith(f'identifier "{ident}" ') and certificate_leaf(requirement) == sha, requirement
        details = subprocess.run(["codesign", "-dv", "--verbose=2", str(piece)], capture_output=True, text=True).stderr
        assert "Authority=FERB Release Signing" in details and "TeamIdentifier=not set" in details, details
    copy = tmp_path / "ferb-secrets"
    shutil.copyfile(real_bundle / "Contents/Helpers/ferb-secrets", copy)       # what setup copies into ~/.ferb/bin
    pinned = Installer(tmp_path / ".ferb", bundle=real_bundle).sealed_requirement(copy, "Helpers/ferb-secrets")
    assert certificate_leaf(pinned) == sha
```

Run: `uv run pytest tests/test_packaging.py -q`
Expected: PASS (the new test skips without `FERB_RELEASE_BUNDLE`).

- [ ] **Step 2: Build with the release certificate (the owner watches: macOS may ask once to use the key)**

```bash
rm -rf /private/tmp/ferb-release-check
SIGN_IDENTITY=release nice -n 10 scripts/build-app.sh /private/tmp/ferb-release-check
codesign -d -r- /private/tmp/ferb-release-check/FERB.app
```

Expected:
- The build prints `Signing (identity: release)` and ends with no `doesn't meet FERB's release requirement`.
- The output shows `designated => identifier "com.ferb.FERB" and certificate leaf = H"<sha1>"`.
- If macOS asks, the owner clicks **Always Allow**.

(Without Phase A the command is the same; it builds arm64.)

- [ ] **Step 3: Run every real-bundle test on it**

```bash
FERB_APP_BUNDLE=/private/tmp/ferb-release-check/FERB.app FERB_RELEASE_BUNDLE=1 \
  uv run pytest tests/test_packaging.py -k real -v
```

Expected: all PASS, including the structure, trimming and size tests of Phase A, and `--verify --deep --strict` on a self-signed signature.

- [ ] **Step 4: Commit**

```bash
git add tests/test_packaging.py
git diff --cached --stat
git commit -m "test(L34): a real release-signed FERB.app pins the release certificate in all five pieces and in setup's helper pin (packaging B, task 5)

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 6: Two-build TCC test on a second macOS user (owner steps; stop rule)

Do this before any work that relies on the grants surviving (Phases F and G). **If any grant made to build N is gone for build N+1, or macOS asks for one again: stop, change nothing, and report to the owner and the controller with both doctor files and screenshots of the Privacy & Security panes.** That is the spec's stop rule.

- [ ] **Step 1: Two builds, same certificate, different code (owner's account, Terminal)**

```zsh
cd ~/Desktop/FERB
rm -rf /private/tmp/ferb-tcc && mkdir -p /private/tmp/ferb-tcc
SIGN_IDENTITY=release nice -n 10 scripts/build-app.sh /private/tmp/ferb-tcc/n
sleep 61                    # CFBundleVersion counts minutes: build N+1 gets a later number, so its code differs
SIGN_IDENTITY=release nice -n 10 scripts/build-app.sh /private/tmp/ferb-tcc/n1
for b in n n1; do
  print "== $b"
  /usr/libexec/PlistBuddy -c "Print :CFBundleVersion" /private/tmp/ferb-tcc/$b/FERB.app/Contents/Info.plist
  codesign -dv /private/tmp/ferb-tcc/$b/FERB.app 2>&1 | grep '^CDHash='
  codesign -dv /private/tmp/ferb-tcc/$b/FERB.app/Contents/Helpers/FERB.app 2>&1 | grep '^CDHash='
  codesign -d -r- /private/tmp/ferb-tcc/$b/FERB.app 2>&1 | grep designated
done
```

Expected:
- Two different build numbers.
- The app's `CDHash=` lines differ between `n` and `n1`, and so do the agent's.
- The `designated =>` lines are identical.

If either CDHash pair is equal, the test would prove nothing: wait another minute and rebuild `n1`.

- [ ] **Step 2: Two disk images, marked as downloaded**

```zsh
mkdir -p /Users/Shared/ferb-tcc
SIGN_IDENTITY=release scripts/build-dmg.sh /private/tmp/ferb-tcc/n/FERB.app /Users/Shared/ferb-tcc/FERB-N.dmg
SIGN_IDENTITY=release scripts/build-dmg.sh /private/tmp/ferb-tcc/n1/FERB.app /Users/Shared/ferb-tcc/FERB-N1.dmg
for d in /Users/Shared/ferb-tcc/FERB-N.dmg /Users/Shared/ferb-tcc/FERB-N1.dmg; do
  xattr -w com.apple.quarantine "0081;$(printf %x $(date +%s));Safari;" "$d"    # as a Safari download
done
chmod 644 /Users/Shared/ferb-tcc/*.dmg
```

- [ ] **Step 3: The second account (owner's answer to B1)**

Use the account the owner chose. Otherwise create it: System Settings → Users & Groups → **Add User…** → New User: Administrator, Full Name "FERB Test", a password of the owner's choice. Switch to it with the menu bar's user menu (fast user switching keeps the owner's session and FERB running) and skip the Apple Account setup.

- [ ] **Step 4: Install build N and grant permissions (as FERB Test; record what macOS shows at each step)**

1. Finder → Go → Home → File → New Folder, named `Applications`.
2. Finder → Go → Go to Folder… → `/Users/Shared/ferb-tcc` → open `FERB-N.dmg`. Drag FERB from the disk image window onto the `Applications` folder in the home folder, **not** onto the image's Applications link (that is the shared `/Applications`). Eject the image.
3. Double-click `~/Applications/FERB`. macOS says it can't verify FERB: click **Done**. Open System Settings → Privacy & Security, scroll to Security, click **Open Anyway** next to FERB and enter the password; FERB opens.
4. In the **Set Up FERB** window, answer setup's questions and grant each permission it asks for: Accessibility, Screen Recording, Full Disk Access (setup says how), Input Monitoring if offered, and Speech Recognition for FERB Voice. Answer **n** to the ChatGPT sign-in; this test doesn't need it.
5. In Terminal (as FERB Test):

```zsh
~/.local/bin/ferb doctor > /Users/Shared/ferb-tcc/doctor-N.txt 2>&1
grep -E 'permission |hearing' /Users/Shared/ferb-tcc/doctor-N.txt
plutil -extract EnvironmentVariables.FERB_APP_BUILD raw ~/Library/LaunchAgents/com.ferb.daemon.plist
```

Expected baseline: `permission accessibility`, `permission screen_recording` and `permission full_disk_access` read `granted`, and the build number is N's. If one isn't granted, grant it in System Settings now and run the three lines again: the baseline must hold the grants. Also look at System Settings → Privacy & Security → Accessibility, Screen & System Audio Recording, Full Disk Access (FERB on) and Speech Recognition (FERB Voice on), and take a screenshot of each.

- [ ] **Step 5: Install build N+1 over it**

1. Menu bar FERB icon → **Quit** (the background service keeps running).
2. Open `/Users/Shared/ferb-tcc/FERB-N1.dmg`, drag FERB onto `~/Applications` → **Replace**, then eject.
3. Double-click `~/Applications/FERB`. Record whether macOS blocks it again and needs Open Anyway (possible, since this copy is a new download; it doesn't decide the test). FERB's window updates the background service for the new build. Record whether anything asks for a permission or says to grant one again (expected: nothing).

```zsh
~/.local/bin/ferb doctor > /Users/Shared/ferb-tcc/doctor-N1.txt 2>&1
plutil -extract EnvironmentVariables.FERB_APP_BUILD raw ~/Library/LaunchAgents/com.ferb.daemon.plist
diff <(grep -E 'permission |hearing' /Users/Shared/ferb-tcc/doctor-N.txt) \
     <(grep -E 'permission |hearing' /Users/Shared/ferb-tcc/doctor-N1.txt) && print SAME
```

**Pass:**
- The build number is N+1's (the daemon now runs the new build).
- The diff prints `SAME`.
- In System Settings every toggle from Step 4 is still on, with no second FERB entry.
- Nothing asked for a permission.

Take the same screenshots.

**Fail:** anything else. Stop (see the top of this task).

- [ ] **Step 6: Clean up**

As FERB Test: `~/.local/bin/ferb uninstall` (answer **N** to deleting Keychain items), then move `~/Applications/FERB.app` to the Trash and log out. On the owner's account: `rm -rf /private/tmp/ferb-tcc /Users/Shared/ferb-tcc`. Delete the FERB Test user only if the owner wants (System Settings → Users & Groups).

- [ ] **Step 7: Record the result**

Append to `docs/OWNER_ACCEPTANCE.md`:

```markdown
## FERB for everyone, phase B (L34): two-build TCC test — <pass|fail> (<date>)

Second macOS user "FERB Test". Build N <number>, build N+1 <number>, both signed with "FERB Release Signing"
(SHA-1 in scripts/release-cert.sha1); app and agent code hashes differed, designated requirements were identical.
- First open of N: <what macOS showed>. Of N+1: <what macOS showed>.
- Grants for N (doctor): <the permission lines>. For N+1: <SAME, or the differing lines>.
- System Settings after N+1: <what the owner saw>.
```

Fill each `<…>` with what the owner observed. Then commit only this file:

```bash
git add docs/OWNER_ACCEPTANCE.md
git commit -m "docs(L34): the two-build TCC test on a second macOS user: <pass|fail> (packaging B, task 6)

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

On fail, stop here and report.

---

### Task 7: Docs and the decision record

Only after Task 6 passed.

**Files:**
- Modify: `README.md` (disk image step 2, around line 23; the update paragraph, around lines 39-43; the build sentence, lines 50-53; Current limitations, line 599)
- Modify: `ARCHITECTURE.md` ("Packaged FERB.app": **Signing**, **Install steps** and **Residuals** bullets, around 2913-2929; the secret helper paragraph, around 1849-1852)
- Modify: `CHANGELOG.md`, `docs/0.27_DECISIONS.toml`

- [ ] **Step 1: README**

- Step 2 of "FERB.app (disk image)": replace "This build is ad-hoc signed, not yet signed with an Apple Developer ID, so macOS stops it the first time." with "FERB is signed with its own certificate, not an Apple Developer ID, so macOS stops it the first time."
- The update paragraph: replace "Until FERB is Developer ID signed, an update needs its macOS permissions granted again (the window says which; Set Up FERB… walks through them), and the Keychain may ask once before FERB's secret helper reads an item stored by an earlier build." with "Every release is signed with the same FERB certificate, so an update keeps FERB's macOS permissions and its secret helper's Keychain access."
- The build sentence: after the `SIGN_IDENTITY` Developer ID text, add "`SIGN_IDENTITY=release` signs with FERB's release certificate (creating and backing it up: the top of `scripts/signing.sh`); a disk image holds only a release-signed or Developer ID app (`FERB_DEV_DMG=1` makes a test image)."
- Current limitations: replace "FERB.app is ad-hoc signed: the first open needs **Open Anyway**, macOS permissions must be granted again after each update, and whether macOS runs the bundled Python from a downloaded (quarantined) disk image under launchd is untested." with "FERB.app is signed with FERB's own certificate, not a Developer ID: the first open needs **Open Anyway**". Then add what Task 6 recorded about a hand-downloaded update's first open, and that launchd ran the bundled Python from a quarantined download on the second account (Task 6 showed it).

- [ ] **Step 2: ARCHITECTURE.md**

- Replace the **Signing** bullet with:

  ```markdown
  - **Signing.** `scripts/signing.sh` is the one signing step, with the hardened runtime always. Ad-hoc is the
    default (development, tests). `SIGN_IDENTITY=release` signs every published build with FERB's self-made
    certificate "FERB Release Signing" (owner's login keychain; its public SHA-1 in `scripts/release-cert.sha1`,
    selected by that hash) and gives FERB.app, the agent, FERB Voice, the bundled `python3.12` and `ferb-secrets`
    the designated requirement `identifier "<id>" and certificate leaf = H"<sha1>"`, so TCC grants and Keychain
    access survive updates (two-build test on a second macOS user, `docs/OWNER_ACCEPTANCE.md`).
    `build-dmg.sh` packs only release-signed or Developer ID apps (`FERB_DEV_DMG=1`: test images). A Developer ID
    `SIGN_IDENTITY` with notary credentials adds timestamps, notarization and stapling; `build-app.sh --sign-only`
    re-signs without rebuilding. Release code has no Team ID, so disable-library-validation stays on the daemon
    and Python.
  ```

- **Install steps**: replace "Helpers are pinned to the exact code FERB.app's signature seals." with "Helpers are pinned only when they are the exact code FERB.app's signature seals, and then to their code hash (ad-hoc), to the release certificate FERB.app itself carries (`certificate_leaf`), or to its Developer ID team."
- Secret helper paragraph (around line 1851): replace "(the `cdhash` when ad-hoc, the Developer ID team later)" with "(the `cdhash` when ad-hoc; the release certificate when release-signed, so the pin survives updates; the Developer ID team later)".
- **Residuals**: replace "ad-hoc builds lose macOS grants and may prompt the Keychain on update; whether launchd runs the bundled Python from a quarantined download is untested;" with "ad-hoc (development) builds lose macOS grants on rebuild; the release key in the login keychain can be used by any process running as the owner through codesign;".

- [ ] **Step 3: CHANGELOG.md and the decision record**

Add under the phase A entry (or at the top if phase A's entry isn't there):

```markdown
## 0.27.0.dev1 FERB for everyone, phase B (L34) — 2026-10-25 — stable signing; two-build TCC test passed

Plan `docs/superpowers/plans/2026-10-25-packaging-b-stable-signing.md`.
- Published builds are signed with FERB's own long-lived certificate "FERB Release Signing" (`SIGN_IDENTITY=release`,
  selected by the SHA-1 in `scripts/release-cert.sha1`; created and backed up by the owner, the steps at the top of
  `scripts/signing.sh`). FERB.app, the agent, FERB Voice, the bundled Python and the secret helper pin that
  certificate in their designated requirement, so macOS permissions and Keychain access survive updates.
- A disk image holds only a release-signed (or Developer ID) app; `FERB_DEV_DMG=1` makes test images.
- Setup pins FERB.app's helpers to the certificate the app carries, so an update keeps the pins.
- Checkout validation: `uv run pytest -q` passed, mypy clean; real release-signed bundle tests passed. Installed on
  the owner's account: no. Owner live result: the two-build TCC test on a second macOS user passed
  (`docs/OWNER_ACCEPTANCE.md`).
```

In `docs/0.27_DECISIONS.toml`, add `"tests/test_install.py"` to `[L34]`'s `tests`. If `[L34]` doesn't exist yet, create it exactly as phase A's plan Task 7 Step 4 writes it, with `tests = ["tests/test_packaging.py", "tests/test_install.py"]`. Append to its `note`: "Phase B: release builds signed with the self-made certificate \"FERB Release Signing\" (SHA-1 in scripts/release-cert.sha1), designated requirement identifier + certificate leaf; two-build TCC test on a second macOS user passed (OWNER_ACCEPTANCE)."

Run: `uv run pytest tests/test_decision_coverage.py -q && uv run pytest -q && uv run mypy ferb/core && git diff --check`
Expected: PASS.

- [ ] **Step 4: Commit and push**

```bash
git add README.md ARCHITECTURE.md CHANGELOG.md docs/0.27_DECISIONS.toml
git diff --cached --stat
git commit -m "docs(L34): packaging phase B: FERB's release certificate, pinned requirements, permissions that survive updates

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
git push origin main
```

Report three things separately: checkout validation, installation status (nothing installed on the owner's account) and the owner's live result (Task 6).
