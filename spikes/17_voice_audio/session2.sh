#!/bin/bash
# Spike 17, part 2 (about 2 minutes): the microphone with the certificate-signed build, and the talk key, slowly.
cd "$(dirname "$0")"
APP=build/FERBAudioSpike.app
echo "── 1/3 Allow the microphone and speech recognition when macOS asks (nothing is recorded in this step)."
read -p "   Press Enter… " _
open -W "$APP" --args request
echo "── 2/3 Say clearly, right after GO:  \"FERB, what's the weather in Delhi tomorrow?\"  (8 seconds)"
read -p "   Press Enter… " _
sleep 1; echo "   GO"; open -W "$APP" --args mic 8
rm -f /tmp/ferb-talk-key.log
echo "── 3/3 Talk key: three short rounds, one action each."
for what in "HOLD Right Option for about 1 second, then release" "TAP Right Option very quickly once" "HOLD Right Option, press C, release both"; do
  read -p "   Next: $what. Press Enter, then do it within 6 seconds… " _
  ../../.venv/bin/python talk_key.py 6 >/dev/null
done
echo; echo "Done. Tell Claude 'part 2 done'."
