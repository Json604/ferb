# FERB for iPhone

The iPhone app of stage 4 of the Rust rewrite (ENG-1; `engine/PLAN.md` "Stage 4"), the twin of `apps/android`
feature for feature, on the same Rust phone library (`engine/crates/ferb-mobile-ffi`) and the same design (the glass
lab, `~/Desktop/ferb-glass-lab`, PHN-416). `engine/JOURNAL.md` has what was built and why (stage 4's entry).

| Folder | What |
|---|---|
| `Packages/FerbMobile` | The phone library: `FerbMobileFFI.xcframework` (the static archives for iPhone and the Apple-silicon simulator) and its generated Swift (module `FerbMobile`). Both are made by `tools/build-rust.sh` and git-ignored. |
| `Packages/FerbDesign` | The lab's phone design as a SwiftUI library (tokens, the sky, the season layer, the pieces, the twelve screens, the home screen's words) and `FerbGallery`. Its own README says what was copied from where. |
| `Packages/FerbCore` | Android's `:core` for iPhone: `FerbPhone`, the `Vault` (Keychain items and an excluded folder), the three signing keys (the Secure Enclave), the prompt, the phone's signals. |
| `Packages/FerbApp` | The app: `FerbAppModel` (the app model, the chat's rows, questions, tasks, files, Watch live, settings, the lock, notifications' plan; the one string catalog) and `FerbAppUI` (the screens, the scanner, notifications, the privacy cover, the root). |
| `App`, `Gallery`, `UITests` | The Xcode project's three thin targets: `@main` for FERB, `@main` for FERB Gallery, the UI tests. |
| `FERB.xcodeproj` | Written by `tools/make_project.py` (never by hand; `--check` compares). |
| `tools` | `build.sh` (the one command), `build-rust.sh`, `make_project.py`, `strings.py`. |

## Build and test (simulator, no signing)

Xcode 27 with an iOS simulator runtime, Rust from `engine/rust-toolchain.toml` with the `aarch64-apple-ios` and
`aarch64-apple-ios-sim` targets (`rustup target add aarch64-apple-ios-sim` in `engine/` once: the toolchain file lists
only the phone target the gate builds). From the repository's root:

```sh
apps/ios/tools/build.sh          # the phone library (skipped when unchanged), FERB.app and FERB Gallery.app for the simulator
apps/ios/tools/build.sh test     # then every test: the packages' unit and snapshot tests and the UI tests
```

`build.sh` makes the simulator "FERB iOS main" (an iPhone 16 Pro) when it is missing (`FERB_SIM` picks another) and
signs the simulator build ad hoc (`CODE_SIGN_IDENTITY=-`, Xcode's "Sign to Run Locally": no team, no certificate;
an unsigned simulator app has no entitlements and the simulator's Keychain refuses it). SwiftPM runs anonymously (`scripts/swiftpm-anonymous.sh`): nothing touches
the owner's keychain or signing identities. The first build of the phone library takes about 4 minutes; later ones
skip it when no engine source, lock file, patch or the toolchain changed (`apps/ios/build/rust/inputs.sha256`).

Run it by hand: `xcrun simctl install "FERB iOS main" apps/ios/build/dd/Build/Products/Debug-iphonesimulator/FERB.app`
then `xcrun simctl launch "FERB iOS main" com.ferb.phone`.

A package alone: `cd apps/ios/Packages/<Package> && source ../../../../scripts/swiftpm-anonymous.sh && xcodebuild test
-scheme <Package> -destination 'platform=iOS Simulator,name=FERB iOS main' CODE_SIGNING_ALLOWED=NO`. The design's
screenshots are re-recorded with `TEST_RUNNER_FERB_RECORD_SNAPSHOTS=1` (its README).

## Choices (kb `phone` area, the stage 4 rulings)

- **Minimum iOS 17** (PHN-560): the design is the lab's SwiftUI, written for iOS 17 (`@Observable`, `UnevenRoundedRectangle`,
  `TimelineView`, the lab's own `.iOS(.v17)`), and iOS 17 runs on every iPhone iOS 18 does (XS and later). The phone
  library itself needs only iOS 13 (`ios_api_check.py`).
- **The project** (PHN-562): every line of code is in the four Swift packages; the project only makes the bundles from
  three synchronized folders, and is written by `tools/make_project.py` (deterministic, no network, no generator to
  install), so it never drifts by hand. The bundle id `com.ferb.phone` (as Android's applicationId) is a placeholder
  the owner may change in one place: `BUNDLE_ID` in `tools/make_project.py` (the gallery's and the UI tests' follow).
  No team: set one in Xcode's Signing for a device build.
- **Keys and storage** (PHN-561): the Secure Enclave's P-256 keys with Android's semantics (device: no prompt; lock:
  Face ID, Touch ID or the passcode each use, kept across new faces; strong: Face ID or Touch ID only, unusable once
  they change), the link state and the store key as Keychain items (AfterFirstUnlockThisDeviceOnly), the store and the
  record files in an excluded folder. The simulator has no Secure Enclave: software keys through the same encoding.
  No DNS context is needed on iOS: n0-dns-resolver reads the nameservers through libresolv (PHN-478), so there is no
  `NativeInit` twin.
- **The scanner** (PHN-564): AVFoundation's QR reader, as PHN-447 named.
- **FERB's account** (PHN-827..829, the owner's "required. google and apple."): after the lock and before anything else,
  signed out shows only the sign-in (FerbAppModel `AccountModel`, FerbAppUI `AccountViews.swift`). Google and Apple both
  go through the site's web flow in ASWebAuthenticationSession (callback `ferb-auth://callback`, PKCE S256 and our own
  state; no URL scheme registered); a native Sign in with Apple would need a second audience on the server and is not
  built. The account and its tokens are one Keychain item of the service `com.ferb.phone.account`
  (AfterFirstUnlockThisDeviceOnly), apart from the vault's so Forget this Mac never signs out; the device id is in
  UserDefaults, and its absence (a new install) deletes what an earlier install left in that service. Signed in with
  no Mac paired: "FERB runs on your Mac" (the download address, then pairing). Pairing first asks the site for
  the account's assertion for this phone's link id and carries it to the Mac (`pair(code:account:)`); the Mac's refusal
  is shown in its own words. Sign out, Delete account (This phone → the account's row) and a sign-in the site ended
  (`invalid_grant`, or a call refused after a refresh) all leave the Mac too. A pairing made before sign-in existed is bound to the account
  (`phone.bind_account` with the assertion) at sign-in or the next unlock, once per unlock until the Mac says bound;
  `account_mismatch` shows the Mac's words on the removed page, anything else waits for the next unlock. The UI
  tests never reach the site: `-FERBUITestSignedIn YES` (Debug, simulator) signs in an invented account in memory.
- **Screenshots** (PHN-810, PHN-811, the owner's PHN-598): iOS has no public FLAG_SECURE. FERB follows the owner's one
  Block screenshots switch, set once at the root (`ferbScreens` in FerbAppUI `Privacy.swift`) and carried into every page
  and every window it presents. With it on, the pictures of the Mac (Watch live, a task's short view, the Mac's camera)
  are drawn inside a secure text field's canvas (FerbAppUI `ScreenshotShield`, the banking apps'
  technique: public calls, undocumented behaviour), which iOS leaves out of screenshots, and FERB hides while the screen
  is recorded or mirrored (`isSceneCaptured`); with it off they show. Where an iOS doesn't build that canvas the pictures
  are drawn plainly and This phone says screenshots can show FERB. The rest of the app shows in screenshots either way.
  A private answer never shows its value and hides from captures whatever the switch says (PHN-110). FERB covers itself
  in the app switcher always, presented windows included. The simulator doesn't honour the secure layer: a blank
  screenshot is checked on an iPhone. `-FERBUITestShield YES` (Debug, simulator) shows a plain and a shielded half.
- **Speaking** (PHN-603..605): hold the microphone; Apple's recogniser with `requiresOnDeviceRecognition` where the
  iPhone supports the language, else Apple's service after FERB's words; the words land in the message box.
- **Background** (PHN-566): no Stay connected; see below (the words are the owner's question, kb T-1241).
- **Tests**: FerbMobile 4 (the library on the simulator, the real link started), FerbCore 14 (keys, vault, FerbPhone;
  one skipped: an unhosted test bundle has no Keychain), FerbDesign 40 (tokens, sky, words, 45 goldens), FerbApp 81
  (every fixture chat through the real library, the app model, notifications' plan, the app's screens against the
  design's goldens, the privacy cover, the catalog, speaking, load more, the screenshot shield), UI 5 (launch, the
  lock, pairing page, a wrong typed code through the real link, no camera, coming back, the shield drawn in place).
- **Words** (PHN-567): one catalog (`FerbAppModel/Resources/Localizable.xcstrings`), written by `tools/strings.py` from
  Android's `strings.xml` under the same names; `python3 -I apps/ios/tools/strings.py --diff` lists every string that
  differs and why.

## What reaches the phone when (iOS has no Stay connected)

iOS gives an app no foreground service. FERB on iPhone receives:
- **open on screen:** everything, as on Android;
- **just left (the background):** everything for as long as iOS lets it run, usually under 30 seconds (a background
  task keeps the link up and posts FERB's notifications locally); then running transfers are paused where they are
  (they go on from there at the next open) and the link pauses;
- **closed or suspended:** nothing reaches the phone until FERB is opened again; what waited is fetched then. Stage 5's
  push (PHN-421) is what brings questions to a closed app.
The This phone sheet says so where Android shows Stay connected.

## On an iPhone (by hand, when the owner has one and a provisioning profile)

1. In Xcode open `apps/ios/FERB.xcodeproj`, target FERB → Signing & Capabilities: choose the team; change the bundle id
   if it is taken (then also in `tools/make_project.py`). Build `apps/ios/tools/build-rust.sh` first.
2. Run on the iPhone (Face ID set up, a passcode on). Check, in order:
   - FERB opens locked; Face ID opens it; leaving for more than 5 minutes locks it again; the app switcher shows the cover.
   - Pair with the camera (the QR on the Mac) and with Type code; the six digits match; the Mac's Yes pairs.
   - Chat, a question answered (a routine one from its notification while FERB is in the background; a check with
     Face ID), Stop, Stop everything.
   - Files: open a folder, fetch a file, Open (QuickLook) and Share; a key place asks Face ID each time; send a photo.
   - Watch live with Block screenshots on: hidden while recording the screen (Control Center → Screen Recording); a
     screenshot (side button + volume up) taken while Watch live shows the Mac comes out with the picture blank and the
     buttons drawn; This phone's screenshot row says "Pictures of your Mac stay out of screenshots, …". With the switch
     off, the same screenshot shows the picture.
   - Speaking: hold the microphone in the message box; FERB's words first, then iOS's two questions; the words land in
     the box, nothing is sent until Send; with Settings → Accessibility → VoiceOver on, double-tap and hold works.
   - This phone → Forget this Mac: the chat goes, pairing starts over.
   - Settings → Face ID → add an appearance: the strong key reads as changed (checks need pairing again); turn the
     passcode off: FERB asks to pair again.
3. The live checks are LIVE-1100..1111 with the iPhone in place of the Android phone.
