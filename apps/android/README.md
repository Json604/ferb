# FERB for Android

The Android app of stage 3 of the Rust rewrite (ENG-1; `engine/PLAN.md` "Stage 3"). It is built in slices:

| Module | Slice | What |
|---|---|---|
| `:design` | S10 (T-1210) | The glass lab's phone design in Jetpack Compose (PHN-416, PHN-449): tokens, components, the twelve screens as stateless composables, and the live sky (PHN-450). No networking, no engine. |
| `:gallery` | S10 | A debug-only app that shows every token, component and screen with invented data, at any place and time of day. Asks for no permission. |
| `:core` | S9 (T-1209) | The phone library (`engine/crates/ferb-mobile-ffi`) built by cargo-ndk with its UniFFI Kotlin; the Vault (Keystore keys, the sealed store key and link state); `FerbPhone`, the one object the app uses; the Android signals. |
| `:app` | S11 (T-1211, T-1221) | The app itself (`com.ferb.phone`): the lock (the Mac told when it locks), FERB's account (kb PHN-827..829: the sign-in page right after the lock, Google or Apple through the site's web flow in a Custom Tab with PKCE, the answer back as the verified App Link `https://ferb.kartikey.xyz/app/callback`; tokens sealed under their own Keystore key `ferb.key.account` in `noBackupFilesDir/account`; the phone-first page while no Mac is paired; pairing carries the server's account statement; Sign out and Delete account in This phone), pairing with the scanner, chat (read marks, files with a message), tasks from the Mac, files both ways (places, folders, key places behind the fingerprint, pause and resume, Open with and Share on the owner's tap), Watch live (always out of screenshots), Stop everything, FERB's settings, the Mac's name, questions and notifications by what the Mac says each needs (every question that waits, the planner's too, notifies unless the app is on screen; what keeps them from arriving is said with its fix, kb PHN-760..766), Stay connected; search, recent files and whole folders either way; the phone's own facts for FERB (its city only when the owner turns it on) and one exact fix when the Mac asks for a task; replying to a message by swiping it to the right (its quote above the message box and above the sent message, kb APP-1146); speaking to FERB by holding the microphone (the words land in the message box, never sent by themselves: on-device recognition where the phone has it, else its own speech service after FERB says so, PHN-603..605); "Load more" for long folders and searches; where a sent file landed. |

## Build

JDK 21 and the Android SDK (`~/Library/Android/sdk`, platform 37 installed) are needed; `local.properties` points at
the SDK and is not committed. From `apps/android`:

```sh
./gradlew :design:test :gallery:assembleDebug   # the sky and token tests, and every screenshot against its golden
./gradlew :core:testDebugUnitTest               # :core's JVM tests against the host library (builds it first)
./gradlew :core:connectedDebugAndroidTest       # :core on a running emulator or phone
./gradlew :design:recordRoborazziDebug          # rewrite the goldens after a deliberate change (look at them first)
./gradlew :app:testDebugUnitTest                # the app's JVM tests: every fixture chat through the real library, the
                                                # lock, answers, words, and its screenshots (the chat against :design's)
./gradlew :app:recordRoborazziDebug             # rewrite the app's own goldens (app/src/test/screenshots)
./gradlew :app:connectedDebugAndroidTest        # on a running emulator or phone: manifest, lock gate, PIN prompt,
                                                # notifications by needs, Watch live's FLAG_SECURE, files through the
                                                # FileProvider, Stay connected (sets and clears a PIN 1111 on it)
                                                # and MainThreadStormTest: 150 s of the app on a link that moves, drops
                                                # and answers slowly; fails on a main-thread stall over 250 ms (kb
                                                # PHN-683; alone, for N seconds: adb shell am instrument -w -e class
                                                # com.ferb.app.MainThreadStormTest -e storm_s N
                                                # com.ferb.phone.test/androidx.test.runner.AndroidJUnitRunner)
./gradlew :app:assembleDebug                    # app/build/outputs/apk/debug/app-debug.apk (adb install -r it)
./gradlew :app:assembleBenchmark                # starts as a release does (R8, not debuggable, the baseline profile)
                                                # but debug-signed: app/build/outputs/apk/benchmark/app-benchmark.apk
                                                # installs over the debug app and keeps its pairing (kb PHN-633)
./gradlew --stop                                # stop the Gradle daemon when done
```

The first run downloads Gradle's dependencies and Robolectric's Android 15 runtime; later runs work offline.
Versions: Gradle 8.14.5, Android Gradle Plugin 8.13.2, Kotlin 2.2.21, Compose BOM 2024.12.01 (Compose 1.7.6),
Robolectric 4.16, Roborazzi 1.52.0; minSdk 30, compileSdk and targetSdk 37.

The gallery's APK is `gallery/build/outputs/apk/debug/gallery-debug.apk` (`adb install -r` it).

## The Rust build (:core)

`./gradlew :core:assembleDebug` (or any build of a module that uses `:core`) runs, each only when its inputs changed:

1. `:core:cargoNdk`: `engine/patches/apply.sh`, then `cargo ndk -t arm64-v8a -P 30 build --release --lib --locked` in
   `engine/crates/ferb-mobile-ffi` (cargo-ndk 4.1.2, `cargo install cargo-ndk --locked`; NDK 28.2.13676358, pinned in
   `core/build.gradle.kts`), with 16 kB page alignment, and copies only `libferb_mobile_ffi.so` into
   `core/build/rust-jni/<abi>`. The crate's release profile (opt-level s, LTO, stripped) is used for debug APKs too
   (kb PHN-495). Add x86_64 (an emulator on an Intel Mac) with `-Pferb.abis=arm64-v8a,x86_64`, after
   `rustup target add x86_64-linux-android` in `engine/`.
2. `:core:cargoHost`: the host library and the crate's own UniFFI generator (`--features bindgen`, dev profile).
3. `:core:uniffiKotlin`: the generator writes `com.ferb.mobile` into `core/build/generated/uniffi/kotlin` from the
   host library (the stripped phone library keeps no UniFFI metadata); the bindings check every function's checksum
   against the loaded library, so the two cannot drift silently. No Python.

All cargo output is in `core/build/rust` (one folder, so cargo's own fingerprints make a repeat build cost seconds).
The inputs are every engine crate's sources and manifests, the lock files, `engine/patches` and the toolchain file:
broad on purpose (a new path dependency is covered); cargo decides what really rebuilds. A first build needs the
network once (crates, Gradle's dependencies); later builds work offline. The JVM tests (`:core:testDebugUnitTest`)
load the host library through JNA's jar (`jna.library.path`). JNA's Android aar (5.17.0) carries its dispatcher,
which is 16 kB-aligned too.

## Keys and storage (:core)

`Vault` keeps everything in `noBackupFilesDir/ferb` (app-private, credential-encrypted storage, never in a backup):
`link.state` (the link's state with the phone's private link key, as `LinkListener.persist` hands it) and
`store.key` (32 random bytes that encrypt the SQLCipher store) are sealed with AES-256-GCM under `ferb.key.wrap`, a
non-exportable Android Keystore key (StrongBox when the phone has one, else the TEE); `phone.db` is the store;
`client.id` a random UUID. The three signing keys (`ferb.key.device`, `ferb.key.lock`, `ferb.key.strong`, P-256) never
leave the Keystore. `:core`'s manifest turns off backup (`allowBackup=false`, `fullBackupContent=false`) and, through
`dataExtractionRules`, Android 12+'s cloud backup and device-to-device transfer for every domain. kb PHN-448,
PHN-496, PHN-497; `engine/JOURNAL.md` entry 114 has the cases (fingerprints changed, lock screen removed, data
cleared, a new phone, root).

## Release signing

A release APK is signed with the owner's own key, which never enters this repository (kb PHN-406, PHN-499); debug
builds need nothing. Once, outside the checkout (keep a copy of the file somewhere safe: an update must be signed
with the same key):

```sh
mkdir -p ~/FERB-signing && keytool -genkeypair -v -keystore ~/FERB-signing/ferb-android-release.p12 \
  -storetype PKCS12 -alias ferb -keyalg RSA -keysize 4096 -validity 10000 -dname "CN=FERB"
security add-generic-password -s ferb-android-release -a "$USER" -w     # the same password, into the login Keychain
echo "ferb.release.storeFile=$HOME/FERB-signing/ferb-android-release.p12" >> ~/.gradle/gradle.properties
```

Then `./gradlew :app:assembleRelease` (R8 minifies it: about 22 MB, 16 MB of it the phone library) signs with it (`ferb.release.keyAlias` if not `ferb`; `ferb.release.password`
or `FERB_RELEASE_PASSWORD` instead of the Keychain; `FERB_RELEASE_STORE_FILE` instead of the properties file). A
release build without them stops at its first task and says what is missing. `apksigner verify --print-certs` shows
the signer. No Play Store yet.

## Where the design numbers come from

The design is `~/Desktop/ferb-glass-lab` (read only; never built from here). On the phone its `Ink` colours
(`Sources/FerbGlassLab/Mobile.swift`) are the authority over `tokens.json`'s warm ramp, which is the Mac's and is kept
for the glass pieces.

- **Tokens** (`design/src/main/kotlin/com/ferb/design/*.kt`) are transcribed by hand, each naming the lab file and
  constant it comes from. `tools/lab_numbers.py` reads the lab's Swift and `tokens.json` into
  `design/src/test/resources/lab/lab-numbers.json` (with the lab commit); `LabNumbersTest` fails when a Kotlin value
  differs, or when a colour the lab's phone draws has no Kotlin counterpart and no stated reason. When the lab
  changes: `python3 -I tools/lab_numbers.py --check`, then rewrite the file and follow the failures.
- **The sky** (`sky/`) ports `Sky.swift`, `AstraSky.swift` and the season layer of `Weather.swift` (weather itself is
  paused, PHN-412). `tools/sky_golden.swift` re-evaluates the lab's formulas, copied by hand and importing nothing
  from the lab, into `sky-golden.json`; `SkyMathTest` holds the Kotlin to those numbers (1e-9).
  `swift tools/sky_golden.swift > design/src/test/resources/lab/sky-golden.json` rewrites it.
- **The time-zone place** (`sky/ZonePlaces.kt`) is generated by `tools/zone_places.py` from the IANA tz database
  (public domain) that macOS ships: the sky's place when the owner gave no location (PHN-420).
- **Screens and components** are held by Roborazzi screenshots of Robolectric's native renderer
  (`design/src/test/screenshots`), at the lab's Android size of 412 by 915 dp.

## Dependencies to name in THIRD_PARTY_NOTICES.md (owner-governed, not edited here)

Shipped in the app: AndroidX Compose (ui, foundation, animation, material-icons-extended) and AndroidX Activity,
Apache-2.0. From `:app` (S11): AndroidX Browser 1.8.0 (Custom Tabs for the sign-in, Apache-2.0), AndroidX CameraX 1.4.1 (camera2, lifecycle, view) and lifecycle viewmodel-savedstate
and service 2.8.3 (Apache-2.0), zxing-cpp's Android reader 2.3.0 (`io.github.zxing-cpp:android`, Apache-2.0, with
its native `libzxingcpp_android.so`), and the glass lab's `words/corpus.json` (the owner's own words). Tests only:
kotlinx-coroutines-test 1.9.0 (Apache-2.0), AndroidX UiAutomator 2.3.0 (Apache-2.0). From `:core` (S9): JNA 5.17.0 (Apache-2.0, chosen of its Apache-2.0/LGPL-2.1 dual licence), kotlinx
coroutines 1.9.0, AndroidX lifecycle-process 2.8.3 and annotation 1.9.1 (Apache-2.0); inside
`libferb_mobile_ffi.so`: UniFFI's runtime and the generated Kotlin's templates (MPL-2.0, source at
github.com/mozilla/uniffi-rs), SQLCipher (BSD-3-Clause), OpenSSL (Apache-2.0), and the Rust crates of ferb-link and
ferb-mobile (their notices as the Mac's). Tests only (not shipped): Robolectric (MIT), Roborazzi (Apache-2.0), AndroidX Test (Apache-2.0),
JUnit 4 (EPL-1.0). The tz table is public domain.
