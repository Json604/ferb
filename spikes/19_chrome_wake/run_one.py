"""Spike 19: run ONE wake method on "FERB Test" (Profile 25) only, then print the detector's events from that run.
Usage: python run_one.py <method> <detector.jsonl>"""
import json
import subprocess
import sys
import time

CHROME = "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"
PROFILE = "--profile-directory=Profile 25"                          # "FERB Test" — never FERB's own or the owner's
URL = "data:text/html,<title>ferb wake spike</title>spike"
METHODS = {
    "old": [CHROME, PROFILE, URL],                                    # FERB's current wake: known to steal focus
    "a": ["open", "-g", "-b", "com.google.Chrome", "--args", PROFILE, "--no-startup-window"],   # Chrome not running
    "b": ["open", "-g", "-b", "com.google.Chrome", "--args", PROFILE, "--no-startup-window"],   # Chrome running
}
method, log = sys.argv[1], sys.argv[2]
running = subprocess.run(["pgrep", "-x", "Google Chrome"], capture_output=True).returncode == 0
start = time.time()
subprocess.Popen(METHODS[method], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, start_new_session=True)
time.sleep(5)
events = [e for e in map(json.loads, open(log)) if e["t"] >= start - 0.2]
print(json.dumps({"method": method, "chrome_was_running": running, "events": events}, indent=1))
print("detector verdict:", "MOVED" if events else "nothing recorded")
