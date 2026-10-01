#!/bin/bash
# Spike 17, part 3 (20 seconds): the live microphone through the path FERB will use (built-in mic, on-device speech).
cd "$(dirname "$0")"
echo "Say clearly, right after GO:  \"FERB, what's the weather in Delhi tomorrow?\"  (6 seconds)"
read -p "Press Enter… " _
sleep 1; echo "GO"
open -W build/FERBAudioSpike.app --args mic2 6 builtin
tail -1 /tmp/ferb-audio-spike.jsonl
echo "Done. Tell Claude 'part 3 done'."
