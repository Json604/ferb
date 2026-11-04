# Plan: switching FERB.app to the Apple Developer ID (T-27)

This plan is for T-27 (area release). It says what changes, what could break and how existing installs move over when
FERB.app stops using the self-made "FERB Release Signing" certificate (REL-403) and starts using an Apple Developer ID
(Individual). It comes after T-29 (DMN-513: the daemon goes back to a user LaunchAgent). Only the owner's decisions are
authoritative. Mechanisms named here are agent proposals, and "(inferred)" marks anything not checked against code,
Apple's documents or a live run. Line numbers are from `20386b7b` on `ui/ux`.

## 0. What depends on the signer today (inventory)

| Where | What it pins or assumes | With a Team ID |
|---|---|---|
| `scripts/signing.sh:6-11,93,159` | `release` mode sets each piece's designated requirement (DR) to `identifier "<id>" and certificate leaf = H"<SHA-1>"` | Developer ID mode uses codesign's default DR: `identifier "<id>" and anchor apple generic and certificate 1[field.1.2.840.113635.100.6.2.6] and certificate leaf[field.1.2.840.113635.100.6.1.13] and certificate leaf[subject.OU] = <TEAMID>` ([TN3127](https://developer.apple.com/documentation/technotes/tn3127-inside-code-signing-requirements)) |
| `scripts/release-cert.sha1` (read at `signing.sh:48-49`; `tests/test_packaging.py:536-545,887`) | the self-made leaf | retired after the switch; a public team ID file takes its place (step 2) |
| `signing.sh:99-110` `ferb_check_release_app` | a DMG holds only a release-signed app; a Developer ID app **skips every check** (`:101`) | needs its own check: team DR, `spctl`, stapled ticket (step 3) |
| `signing.sh:131` | `disable-library-validation` on daemon, python and app unless Developer ID | library validation is **on** in the Developer ID build (new behavior, step 4) |
| `~/.ferb/bin/ferb-secrets.requirement` (written `ferb/device/install.py:771-775`, checked `ferb/identity/helper.py:12-18`) | the helper's DR when it was installed | rewritten by the next `setup --service --update` via `sealed_requirement`, which already has a team branch (`install.py:765-767`; test `tests/test_install.py:467-474`) |
| config `[hearing] requirement` / `[voice] requirement` (`ferb/cli/setup.py:1080-1090`, read `ferb/daemon/areas/voice.py:52-54`, checked `ferb/sense/transcribe.py:107-109`) | FERB.app's DR from `Installer.own_requirement` (`install.py:698-707`, DMN-400) | re-pinned by the same update; `trusted_requirement` accepts any DR containing `certificate leaf` (`install.py:125-130`), so the team DR passes |
| `ferb/daemon/app_peer.py:142-153` (APP_ONLY peers) | no stored pin: the DR is read again from the bundle whenever its signature stamp changes (`:177-189`) | unaffected. **DMN-34/T-9 (open)** would add a pin to the release leaf, which needs an owner decision (§7) |
| macOS TCC (8 services: Accessibility, AllFiles, ScreenCapture, ListenEvent, Microphone, SpeechRecognition, Pasteboard, Automation) | each grant stores the client's DR as `csreq` | old grants don't match the new DR, so everything is granted once more (step 6) |
| Login keychain items the **helper** created (`ferb/identity/secret_helper.swift:17-28`: services `ferb`, `ferb.owner-secrets`) | partition list `cdhash:<helper hash>` (REL-404 why) | one prompt per item, then `teamid:<TEAMID>` holds across builds (step 5) |
| Keychain items written through `/usr/bin/security` (API keys `ui/mac/.../SetupServices.swift:264-290`, `ferb/runtimes/keys.py`, `ferb/identity/secrets.py`; Claude Code's own item `ferb/runtimes/claude_login.py:8`) | trusted to Apple's `security` tool, not FERB's signer | unaffected (inferred from the ACL owner; worth checking in the live run) |
| Background service: SMAppService plus APP-599 (`SetupModel.swift:255-258`), and after T-29 a user LaunchAgent | without a team, the launch constraint (LWCR) names one exact build (DMN-513 why) | becomes team-based; SMAppService comes back (DMN-513 rule; step 7) |
| Login item: `SMAppService.mainApp`, with the fallback LaunchAgent `com.ferb.FERB.login` (`LoginItemController.swift:25-46,96-110`; `LoginItemPlan.swift`) | fallback used when macOS refuses the self-signed login item | the fallback can go, but Macs that have it need moving (step 8) |
| Sparkle 2.10.0 (`ui/mac/FERBNotch/Package.swift:10`), EdDSA key `scripts/sparkle-public-key.txt`, `build-app.sh:109-124` | `signing.sh:11` and `NotchCore/UpdatePlan.swift:4` say updates pass a "same-certificate check" | **wrong for Sparkle 2**: EdDSA alone is enough (§1). Fix the comments |
| `build-dmg.sh:17,40-43`, `README.md:22,150`, site (T-28) | "Open Anyway" first-open instructions | notarized: the normal "downloaded from the Internet" prompt |
| APP-601 `NotchCore/OwnQuarantine.swift` | strips quarantine because nested code ran outside the Open Anyway approval | not needed for notarized builds (the ticket covers the nested code hashes) (inferred) |
| REL-404 `assets/secret-helper/*`, `scripts/secret_helper.py`, `build-app.sh:213-218`, `tests/test_secret_helper_pin.py` | fixed helper bytes so its cdhash survives releases | removable once team partitions are proven (step 5) |
| `extensions/`, `relay/`, the native-messaging manifests | no signature dependency found (grep for codesign/requirement/certificate) | none |

## 1. Sparkle: does an installed self-signed build accept a Developer ID update? Yes.

Evidence: Sparkle 2.10.0 `Sparkle/SUUpdateValidator.m`, `validateUpdateForHost:` (fetched with `gh api` at tag 2.10.0).
Its doc comment says a bundle update must meet one of two conditions: "old and new Ed(DSA) public keys are the same and
valid (it allows change of Code Signing identity), or old and new Code Signing identity are the same and valid". The
code ends with `if (passedDSACheck || passedCodeSigning) return YES;` ("We allow failure of one of them, because this
allows key rotation"). One extra check applies: when EdDSA passes but the identities differ, the new app's signature must
still be valid (`codeSignatureIsValidAtBundleURL`). The public documentation says the same:
<https://sparkle-project.org/documentation/#rotating-signing-keys>. It allows "a new update that changes either your
Apple code signing certificate or your EdDSA keys (but not both)."

So:
- The switch build must keep **the same `SUPublicEDKey`** and be EdDSA-signed by `release-appcast.sh` with the same
  private key. Rotate nothing else in that release.
- `release-appcast.sh:44` signs the DMG **after** `build-dmg.sh` staples it (stapling changes the DMG's bytes). Keep that
  order; signing before stapling breaks EdDSA.
- Builds with no `SUFeedURL` (anything before L34 Phase G, and `FERB_UPDATES=off` builds) never see the update, so those
  users download the new DMG by hand (§9).
- Once on the Developer ID, a lost EdDSA key can be rotated through a Developer ID-signed DMG (same Sparkle page).

## 2. Prerequisites (owner and setup, before any code)

1. The Developer ID Application certificate goes in the owner's login keychain, and its `.p12` backup goes where the
   release `.p12` lives (`signing.sh:30-31`). Notary credentials are stored with `xcrun notarytool store-credentials`
   (profile name only in `NOTARY_KEYCHAIN_PROFILE`; `signing.sh:15-17,112-118`). Nothing enters the repo.
2. **Personal data first:** add the owner's legal name (the certificate's common name is
   `Developer ID Application: <legal name> (<TEAMID>)`) to the untracked `kb/local/personal.txt`, so the `personal`
   gate (`kb/gate.toml:31-33`, `tools/kb/gate.py:205-228`) refuses it in any tracked file, in tests, README, kb and
   scripts alike. Select the identity by team ID or SHA-1, never by name. Note: every shipped binary carries the name in
   its signature (`codesign -dv` shows `Authority=...`). That is public by design and can't be avoided with an
   Individual account.
3. A public `scripts/developer-team.txt` (the 10-character team ID, which is visible in every signed binary anyway) is
   the counterpart of `release-cert.sha1`. Owner's OK needed (§10).
4. Keep `SIGN_IDENTITY=release`, the release certificate and its `.p12` until the first Developer ID release passes its
   live checks (rollback, §9).

## 3. Build and signing changes (`scripts/`)

- `signing.sh`: add a `devid` mode next to `release`. It reads the team from `scripts/developer-team.txt`, picks the one
  `Developer ID Application:` identity whose name ends `(<TEAMID>)` by its SHA-1 (as `release` does at `:67-81`), and
  sets `FERB_SIGN_ID` to the hash. This keeps the legal name out of env files and command lines. `ferb_developer_id`
  (`:89`) stays the switch for timestamp, notarization and library validation.
- Add `ferb_team_requirement ID` = `identifier "<ID>" and anchor apple generic and certificate leaf[subject.OU] = "<TEAMID>"`
  and verify every piece at `:231-240` against it in `devid` mode. Today that verification runs only for `release`.
- `ferb_check_release_app` (`:99-110`): in `devid` mode, require the team DR on the app, `spctl -a -t exec -vv` saying
  `source=Notarized Developer ID`, and `xcrun stapler validate`. Today `:101` returns success unchecked.
- `build-dmg.sh:60` signs the DMG with `$SIGN_IDENTITY`. Use `$FERB_SIGN_ID` so `devid` works. The text file is
  gone (REL-408): the window's first-open note goes away by itself, since `dmg_variant` picks `after-devid` whenever
  `ferb_developer_id` is true; keep that true in `devid` mode. After stapling, check the image with
  `spctl -a -t open --context context:primary-signature -vv`.
- `build-app.sh:213-218`: unchanged in the switch build (§5 explains why the pinned helper stays for one release).
- `release-appcast.sh`: before `sign_update`, refuse a DMG whose app isn't team-signed and stapled (one `spctl` line).
  `release_checks.py` checks versions and the extension only. It stays as it is.

What could break: a `devid` mode resolving to the wrong identity, for example an "Apple Development" certificate of the
same team, which notarization refuses ([resolving common notarization issues](https://developer.apple.com/documentation/security/resolving-common-notarization-issues),
"Use a valid Developer ID certificate"). Test it with the stub-keychain pattern of `tests/test_packaging.py:400-441`.

## 4. Notarization: what each nested executable needs

Apple's requirements ([notarizing macOS software](https://developer.apple.com/documentation/security/notarizing-macos-software-before-distribution)):
a Developer ID certificate, the hardened runtime, a secure timestamp, no `get-task-allow`, and code linked against the
macOS 10.9 SDK or later. Every Mach-O must be signed, or notarization fails with an error.

| Piece | Today (`signing.sh`) | Developer ID notes |
|---|---|---|
| Main app `FERBNotch` (`com.ferb.FERB`) | runtime, `audio-input` (`:126`) | fine. Library validation turns on; Sparkle is re-signed by FERB's team (`:183-194`), so it loads |
| Agent stub `Helpers/FERB.app` (`com.ferb.daemon`) | runtime, `apple-events` (`:124`) | loads libpython and every `.so`, all re-signed by the team: fine under library validation |
| `python3.12` (`com.ferb.python`) and `libpython3.12.dylib`, `lib-dynload/*.so`, site-packages `*.so`/`*.dylib` | re-signed (`:205-219`; libpython ad-hoc first at `build-app.sh:141`, then re-signed) | **Risk:** the `find` at `:205` signs only `*.so`, `*.dylib` and files with the user-execute bit. A Mach-O with another name and no x bit stays unsigned and notarization rejects it. Before notarizing, add a scan of every file with `file -b` = `Mach-O*` |
| `ferb-secrets` (`com.ferb.secrets`) | runtime, no entitlements (`:214-215`) | fine |
| Codex (`Resources/node/.../codex`) | kept as OpenAI signed it when it carries their Developer ID (`ferb_vendor_signed`, `:171-179`) | other teams' Developer ID code, hardened and timestamped, is accepted inside a notarized app (inferred: Apple's page says "all executables" need a Developer ID signature, not "your own team's"). `lipo -thin` (`app-parts.sh:79-84`) keeps each slice's own signature. If Codex dlopens any library FERB re-signed, its library validation refuses it (the reason voice was trimmed, `app-parts.sh:67-70`) |
| Claude Code (`claude_agent_sdk/_bundled/claude`) | same vendor rule | same as Codex (inferred: Anthropic Developer ID signed; check `codesign -dv` on the built app) |
| Sparkle `Autoupdate`, `Updater.app`, `Installer.xpc`, `Downloader.xpc` | inside-out, Downloader keeps its entitlements (`:181-194`) | matches Sparkle's guide for apps signed outside Xcode; `--timestamp` comes from `ferb_sign` (`:148`) |
| Symlinks | `codesign --verify --deep --strict` already passes (`:230`) | a link pointing outside the bundle fails `--strict` and notarization; the DMG's `/Applications` link is outside any bundle and fine |
| DMG | signed and notarized only with a Developer ID (`build-dmg.sh:59-62`) | the app is notarized and stapled first (`build-app.sh:233`), then the image. Two submissions per architecture (~720 MB each), so plan for the time |

**JIT and the Intel build (inferred, needs a test).** The hardened runtime is already on in every build (`--options runtime`
at `signing.sh:162`), so W^X rules already apply to today's builds. The Developer ID build adds only library validation.
The audit's PyObjC/JIT note isn't in the repo (I searched kb, scripts and git log); I didn't verify it. The concern is
this: on x86_64, libffi closures (ctypes `CFUNCTYPE` callbacks, PyObjC callbacks and blocks) may need writable and
executable memory. That would crash the Intel daemon under the hardened runtime unless the python and daemon
entitlements add `com.apple.security.cs.allow-unsigned-executable-memory` (or `allow-jit`). Notarization accepts both
([hardened runtime](https://developer.apple.com/documentation/security/hardened-runtime)). The Intel build has never run
on an Intel Mac (`README.md:150`). Run it there before release (LIVE in §8) and add the entitlement only for x86_64 if it
crashes.

## 5. Keychain: the pinned helper (REL-404)

- Items the old helper created have partition `cdhash:<old hash>` (REL-404 why). The Developer ID helper has a new
  cdhash and a team, so macOS asks **once per such item**: the login password plus Always Allow. After that the item
  carries `teamid:<TEAMID>`, and new items get only the team partition. Later rebuilds of the helper, even other bytes,
  no longer ask (inferred from the `security set-key-partition-list` partition kinds `apple-tool:`, `teamid:`,
  `cdhash:`; LIVE-679 measures it). The prompt comes from a background helper, so the owner must be at the Mac. Copy in
  the first-run screen should say so.
- No lockout: the items themselves stay. If the owner denies, that secret's operation fails (`helper.py:21-22`) until
  allowed.
- **Can the committed binary go?** Yes, after the switch. Proposal: keep the pinned bytes for the switch release, so the
  only change is the signer and one measured prompt per item. In the next release, compile the helper per build again,
  delete `assets/secret-helper/`, `scripts/secret_helper.py` and `tests/test_secret_helper_pin.py`, and supersede
  REL-404 (agent ruling). Do this only after the live check shows no prompt between two Developer ID builds with
  different helper bytes.
- The data-protection keychain (`kSecUseDataProtectionKeychain`) needs a `keychain-access-groups` entitlement and, on
  macOS, a provisioning profile (inferred). It isn't needed for this plan.
- Small fix while there: `install_secret_helper` swaps the binary (`install.py:724`) before it writes the requirement
  (`:725`). A crash in between leaves the new binary checked against the old pin, so secrets are refused until the next
  setup. Write the `.requirement.new` first and swap both.

## 6. TCC: every permission once more

- TCC compares the running code with the `csreq` recorded at grant time. Self-signed grants name the old leaf, so after
  the switch build all eight services read "not granted". System Settings can still show the old FERB entries switched
  on but inert (inferred: common behavior after a signer change). With a team DR, later builds keep their grants. That
  is the lasting gain.
- The existing path: `setup --service --update` runs because the build changed (`NotchCore/AppSetup.swift:58-66`), and
  the intro shows grant beats "only if a grant is missing" (`:20`). Add a fact to the setup record: the signer (team or
  leaf), written by `write_record` (`install.py:317-324`). Then the first Developer ID launch knows it's a signer change.
  It can say "macOS asks for FERB's permissions once more because FERB is now signed by its developer", and handle the
  login item and service moves (steps 7-8).
- Stale entries: the owner removes them with "−", or FERB runs `tccutil reset <service> com.ferb.FERB` (and
  `com.ferb.daemon`) once on that signer change, so macOS prompts cleanly. That's a product choice touching macOS
  permissions (§10).
- Automation is granted per target app: every app FERB scripts asks once more, the first time it's used.

## 7. The background service: SMAppService back (DMN-513 says so), or stay on the LaunchAgent

DMN-513's rule: "SMAppService returns with the Developer ID (T-27)." DMN-2's reasons hold again: the definition is inside
the signed app, nothing can edit it (rule 6) and deleting the app removes it.

- **Launch constraint:** with a Team ID, the registration's constraint is team plus signing identifier, not one build, so
  updates no longer get SIGKILLed (inferred from APP-599 and DMN-513's "without a Team ID … one exact build". Verify
  with `launchctl print gui/$UID/com.ferb.daemon` across two Developer ID builds).
- **Move from T-29's LaunchAgent** (the first Developer ID build's setup): bring back the parts of DMN-2 that T-29
  removes (as of `20386b7b`: `Installer.retire_legacy_agent` `install.py:495-504`, `BUNDLE_SERVICE` `:58`,
  `build-app.sh:180-185`, `ui/mac/FERBAgent/com.ferb.daemon.plist`, `SetupServices.swift:57-82`, `AppSetupPlan.decide`).
  The order is: bootout `gui/$UID/com.ferb.daemon`, wait for exit (`_check_exited`), delete the plist, `register()`, wait
  for `ping`. The same label can't be loaded twice (`install.py:496-498`).
- **Fallback, no lockout:** if `register()` throws, or the status stays `.notRegistered`, write the LaunchAgent again with
  T-29's code and say why. `.requiresApproval` is new friction for existing users: FERB has no daemon until they allow it
  in Login Items. The LaunchAgent needed no approval. Keep the T-29 path as that fallback, not dead code.
- **APP-599** (unregister before every register, `SetupModel.swift:255-258`): with team constraints it isn't needed per
  update, and it re-posts "background item added" each time (inferred). Keep it only for the one signer change (a
  registration left by a self-signed 2026-11 build on a Mac that skipped T-29's builds). Then supersede APP-599 (agent).
- **AssociatedBundleIdentifiers** (`com.ferb.daemon.plist:20`, `LoginItemPlan.swift:27`): Login Items groups a legacy
  agent under FERB only when the team IDs match (inferred; `man launchd.plist` says only that the key names the
  associated app). With SMAppService the grouping is automatic.
- **Staying on the LaunchAgent** is the alternative. It's simpler, with no approval step and no migration, but it loses
  rule 6's "nothing can edit it" property. It's the owner's call only if DMN-513's plan changes (§10).

## 8. Login item, quarantine, copy

- `LoginItemController.applyDefaultAfterSetup` runs once ever (`:117-124`), so a Mac on the fallback
  `com.ferb.FERB.login` agent stays on it. On the signer change: if the fallback is on, try `SMAppService.mainApp`
  again. On success, remove the fallback (`turnOn` already does, `:99-100`). Re-register `mainApp` too if it was enabled
  under the old signer (inferred: it carries an LWCR like the agent).
- APP-601: harmless in a notarized build. Remove it (supersede, agent) after LIVE-684's Developer ID twin shows nothing
  nested is blocked with the attribute left in place.
- Copy: `README.md:22,150`, the DMG note, the site (T-28, the owner's UI agent), and `signing.sh:10-11,36` (the
  "same-certificate check" claim). The certificate-creation recipe moves to an appendix once `release` mode retires.

## 9. Tests, live checks, migration and rollback

**Tests that pin the old identity and what changes (`tests/`):**
- `test_packaging.py:405-441` (release pins) and `:513-533` (a DMG holds only a release-signed app): add `devid` twins,
  with a stub keychain listing `Developer ID Application: Test Owner (TEAM123456)` (`:29`; never the real name), team DR
  verification and the DMG check. Keep the release ones while `release` mode exists.
- `:344-398` already cover timestamp, notarization and credentials. Add: no `disable-library-validation` in `devid`
  entitlements, the Mach-O scan refusing an unsigned nested file, and `build-dmg.sh` using `FERB_SIGN_ID`.
- `:497` (the DMG says how to open it): it stays for `release`; in `devid` the note is absent.
- `:882-897` (real release bundle: `Authority=FERB Release Signing`, `TeamIdentifier=not set`): add a
  `FERB_DEVID_BUNDLE=1` twin that checks the team DR on each piece, `sealed_requirement` taking the team branch, and
  `spctl` notarized.
- `:536-545` (`release-cert.sha1` recorded): add the same for `developer-team.txt`. Delete with `release` mode later.
- `test_install.py:460-500`: the team branch of `sealed_requirement` is tested already. Add the signer in the setup
  record and the atomic requirement swap (§5).
- `test_secret_helper_pin.py`: deleted with REL-404 (§5).
- Swift `SetupModelTests.swift:44` (APP-599): it becomes "unregister only on a signer change". Login-item move test.
- No test names the real team, name or certificate hash.

**Live checks (new ones in `kb/live.toml`, on a Developer ID build the owner installs):**
1. A notarized DMG downloaded in a browser opens with the normal prompt: no Open Anyway, no "damaged", nested code
   starts (twin of LIVE-636, LIVE-684).
2. A self-signed release build in /Applications updates through Sparkle to the Developer ID build (LIVE-637's path).
   Then: the service moves to SMAppService (or the LaunchAgent fallback), the permission beats appear, every grant works
   after one re-grant, and the old entries are handled.
3. Keychain: one prompt per helper-made secret, then none across a relaunch and across a second Developer ID build whose
   helper bytes differ (LIVE-679's twin; it decides §5's deletion). API keys and the Claude sign-in never ask.
4. Two Developer ID builds in a row: the daemon keeps running after update (no launch-constraint kill), and permissions
   stay (LIVE-16 and LIVE-17 updated for DMN-513's successor).
5. The Intel build on an Intel Mac: daemon start, PyObjC paths (AX, capture), hearing (§4 JIT note).
6. Login Items lists one FERB with its service grouped under it, and no `com.ferb.FERB.login` left.

**Migration summary (first Developer ID launch, no user step beyond macOS's own prompts):**
1. Sparkle (or a manual drag) replaces the app.
2. The app sees a new build and runs `setup --service --update`, which re-copies the helper with its team requirement
   and re-pins hearing.
3. On the signer change the app moves the service (LaunchAgent to SMAppService, with fallback), moves the login item,
   and shows the permission beats.
4. Keychain asks once per helper item when first used. The old daemon process checks APP_ONLY peers against the new
   bundle's DR right away (`app_peer.py:146-152`), so the notch is never locked out mid-update.

**Friends:**
- A friend on an old LaunchAgent build with Sparkle updates on its own and sees macOS's prompts.
- A friend on a build without `SUFeedURL`, or on an ad-hoc build without Sparkle, downloads the new DMG and drags it
  over the old app. Nothing needs deleting: `~/.ferb` and the Keychain items stay. The old plists are retired by setup.
  The only manual step is removing inert duplicate entries in Privacy & Security if they want a tidy list.
- Source installs (FERB Local Code Signing, `install.py:732`) are unaffected.

**Rollback:** don't roll back to the self-made certificate. Sparkle would accept it through EdDSA, but TCC, the Keychain
and the launch constraint would churn again. Ship a fixed build signed with the Developer ID instead. Keep `release`
mode and the old `.p12` only until the switch release passes the live checks above. Then retire `release-cert.sha1`,
`ferb_requirement`, the release tests and the DMN-34 leaf (with the owner's new decision).

## 10. Needs the owner's decision

1. **DMN-34 / T-9:** the app-only pin names "FERB's release certificate". With the Developer ID it should name the team
   (`anchor apple generic and certificate leaf[subject.OU] = "<TEAMID>"`). For the switch release it should accept both
   the leaf and the team. That needs a new owner decision that supersedes DMN-34. If T-9 is built before T-27, build it
   as a set of accepted signers, so only the set changes.
2. **REL-403** ("signed without a Developer ID or notarization for now") is superseded by the switch; the owner
   records it.
3. **SMAppService back or stay on the LaunchAgent** (§7). DMN-513 already says it comes back. Confirm, given the new
   Login Items approval step for existing users.
4. **Clearing stale TCC entries:** FERB runs `tccutil reset` for its own bundle IDs once on the signer change, or tells
   the owner to remove them.
5. **Silent auto-update for the switch release** (`SUAutomaticallyUpdate` true, `build-app.sh:122`): FERB would update
   while the owner is away and then lack every permission until they come back, or this one release asks first.
6. **The public team ID in tracked files** (`scripts/developer-team.txt`, the T-9 pin). It's visible in every binary,
   but it identifies the account.
