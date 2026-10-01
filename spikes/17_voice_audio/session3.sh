#!/bin/bash
# Spike 17, part 3 re-run (about 30 seconds): the live microphone through the path FERB will use — built-in mic only
# (never a Bluetooth headset's mic), the speech model loaded first, on-device speech — with a live level meter.
cd "$(dirname "$0")"
APP=build/FERBAudioSpike.app
R=/tmp/ferb-audio-spike.jsonl
echo "Say clearly when you see GO:  \"FERB, what's the weather in Delhi tomorrow?\"  (8 seconds to speak)"
read -p "Press Enter… " _
START=$(wc -l < "$R" 2>/dev/null || echo 0)
open -W "$APP" --args mic2 8 builtin &
APPPID=$!
python3 - "$R" "$START" <<'PY'
import json, sys, time
path, start = sys.argv[1], int(sys.argv[2])
seen, go = start, False
end = time.time() + 40
while time.time() < end:
    try:
        lines = open(path).read().splitlines()
    except FileNotFoundError:
        lines = []
    for line in lines[seen:]:
        seen += 1
        d = json.loads(line)
        m = d.get("mode")
        if m == "mic2-start" and not go:
            go = True
            print(f"   (speech model ready in {d.get('warm_up_s', 0):.2f} s, mic: {d['input_device']})")
            print("   GO — speak now")
        elif m == "mic2-level":
            bar = "█" * min(40, int(d["rms"] * 400))
            print(f"   frames {d['frames_total']:>7}  level {d['rms']:.3f} {bar}")
        elif m == "mic2":
            print(f"\n   heard: {d.get('text') or '(nothing)'}  {('— ' + d['error']) if d.get('error') else ''}")
            print(f"   audio frames received: {d.get('frames', '?')}  | peak level: {d.get('peak', 0):.2f}"
                  f"  | text ready {d.get('after_release_s', 0):.2f} s after recording stopped")
            sys.exit(0)
    time.sleep(0.2)
print("   no result from the app")
PY
wait $APPPID
echo "Done. Tell Claude 'part 3 done' with what it printed."
