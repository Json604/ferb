#!/bin/bash
# Spikes 9 + 17: the guided session (about 3 minutes). Run it in your own terminal from the FERB folder:
#   spikes/17_voice_audio/session.sh
# It plays a few short system sounds, asks you to allow permissions when macOS asks, to say one sentence, and to
# press Right Option a few times. Results: /tmp/ferb-audio-spike.jsonl and /tmp/ferb-talk-key.log.
cd "$(dirname "$0")"
APP=build/FERBAudioSpike.app
SOUND=/System/Library/Sounds/Submarine.aiff
loop_sound() { for i in $(seq 1 12); do afplay "$SOUND"; done; }
step() { echo; echo "── $1"; }

step "1/5 System audio. A sound plays for a few seconds. If macOS asks to let 'FERB Audio Spike' record system audio, click Allow."
read -p "   Press Enter to start… " _
loop_sound & SP=$!
sleep 1; open -W "$APP" --args tap-system 6
AFPID=$(pgrep -n afplay); [ -n "$AFPID" ] && open -W "$APP" --args tap-pid "$AFPID" 3
wait $SP 2>/dev/null

step "2/5 Silence check: nothing playing now."
open -W "$APP" --args tap-system 3

step "3/5 Microphone. Allow the microphone and speech recognition if asked."
echo "   After you press Enter, say clearly:  \"FERB, what's the weather in Delhi tomorrow?\"  (6 seconds)"
read -p "   Press Enter, then speak… " _
open -W "$APP" --args mic 6

step "4/5 Talk key (20 seconds). Do these three, slowly:"
echo "   a) hold Right Option for about 1 second, then release"
echo "   b) tap Right Option very quickly"
echo "   c) hold Right Option and press C, then release both"
read -p "   Press Enter to start… " _
rm -f /tmp/ferb-talk-key.log
../../.venv/bin/python talk_key.py 20

step "5/5 Optional — skip with Enter if you don't have them handy."
read -p "   AirPods: connect them, then press Enter (or type s to skip)… " a
if [ "$a" != "s" ]; then loop_sound & SP=$!; sleep 1; open -W "$APP" --args tap-system 5; wait $SP 2>/dev/null; fi
read -p "   DRM: start a song in Apple Music, then press Enter (or type s to skip)… " m
if [ "$m" != "s" ]; then MPID=$(pgrep -x Music); [ -n "$MPID" ] && open -W "$APP" --args tap-pid "$MPID" 5; open -W "$APP" --args tap-system 5; fi

echo; echo "Done. Tell Claude 'session done'."
