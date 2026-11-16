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
- **Screenshots** (PHN-565): iOS can't blank screenshots as Android's FLAG_SECURE does. FERB covers itself in the app
  switcher always, and hides while the screen is recorded or mirrored (always for Watch live and a private answer, by
  the owner's switch for the rest); a screenshot shows what is on screen. The owner's question: kb T-1240.
- **Background** (PHN-566): no Stay connected; see below (the words are the owner's question, kb T-1241).
- **Tests**: FerbMobile 4 (the library on the simulator, the real link started), FerbCore 14 (keys, vault, FerbPhone;
  one skipped: an unhosted test bundle has no Keychain), FerbDesign 39 (tokens, sky, words, 45 goldens), FerbApp 67
  (every fixture chat through the real library, the app model, notifications' plan, the app's screens against the
  design's goldens, the privacy cover, the catalog), UI 4 (launch, the lock, pairing page, a wrong typed code through
  the real link, no camera, coming back).
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
   - Watch live: hidden while recording the screen (Control Center → Screen Recording).
   - This phone → Forget this Mac: the chat goes, pairing starts over.
   - Settings → Face ID → add an appearance: the strong key reads as changed (checks need pairing again); turn the
     passcode off: FERB asks to pair again.
3. The live checks are LIVE-1100..1111 with the iPhone in place of the Android phone.
