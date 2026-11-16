#!/bin/zsh
# The iPhone app, from the command line, for the simulator: no team, no provisioning profile, no certificate. The
# simulator build is signed ad hoc ("Sign to Run Locally", CODE_SIGN_IDENTITY=-): without it the app carries no
# entitlements and the simulator's Keychain refuses it (errSecMissingEntitlement), so FERB could keep nothing.
#   apps/ios/tools/build.sh            the phone library (build-rust.sh, skipped when unchanged), then FERB.app and
#                                      FERB Gallery.app for the iOS simulator into apps/ios/build/dd/Build/Products
#   apps/ios/tools/build.sh test       the same, then every test on a simulator: the four packages' unit and snapshot
#                                      tests and the app's UI tests (FERB scheme)
# FERB_SIM names the simulator (default: "FERB iOS main", made on first use from an iPhone 16 Pro and the newest iOS
# runtime installed). SwiftPM runs anonymously (scripts/swiftpm-anonymous.sh): it never touches the owner's keychain.
set -eu
HERE="${0:A:h}"
IOS="${HERE:h}"
ROOT="${IOS:h:h}"
SIM="${FERB_SIM:-FERB iOS main}"
source "$ROOT/scripts/swiftpm-anonymous.sh"

"$HERE/build-rust.sh"
python3 -I "$HERE/make_project.py" --check
python3 -I "$HERE/strings.py" --check

if ! xcrun simctl list devices available | grep -q "  $SIM ("; then
  RUNTIME=$(xcrun simctl list runtimes available | grep -o 'com.apple.CoreSimulator.SimRuntime.iOS-[0-9-]*' | tail -1)
  xcrun simctl create "$SIM" com.apple.CoreSimulator.SimDeviceType.iPhone-16-Pro "$RUNTIME" >/dev/null
fi
DEST="platform=iOS Simulator,name=$SIM"
COMMON=(-project "$IOS/FERB.xcodeproj" -destination "$DEST" -derivedDataPath "$IOS/build/dd" CODE_SIGN_IDENTITY=- DEVELOPMENT_TEAM=)

if [[ "${1:-}" == "test" ]]; then
  nice -n 10 xcodebuild test -scheme FERB "${COMMON[@]}"
else
  nice -n 10 xcodebuild build -scheme FERB "${COMMON[@]}"
  nice -n 10 xcodebuild build -scheme "FERB Gallery" "${COMMON[@]}"
  echo "built: $IOS/build/dd/Build/Products/Debug-iphonesimulator/FERB.app and FERB Gallery.app"
fi
