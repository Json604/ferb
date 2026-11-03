#!/bin/zsh -f
# The push gate's Swift tests (kb/gate.toml): `swift test` for FERB.app, its output as usual, and on a failure a closing
# list of the failing tests' lines. The gate shows only a check's last lines, which otherwise end in suite totals and
# never name the test that failed (2026-11-02).
. scripts/swiftpm-anonymous.sh
log=$(mktemp -t ferb-swift-test) || exit 1
trap 'rm -f "$log"' EXIT
swift test --package-path ui/mac/FERBNotch >"$log" 2>&1
result=$?
cat "$log"
if (( result != 0 )); then
  print -- "--- failing tests ---"
  grep -E "\.swift:[0-9]+: error: |' failed \(|error: " "$log" | grep -v '^CoreData' | tail -30
fi
exit $result
