"""Spike 19: wake a Chrome profile without taking focus. "FERB Test" (Profile 25) ONLY — never FERB's own profile or
the owner's. Each method is watched with a 50 ms poll of the front app (Accessibility), so a brief flash shows too."""
import json
import subprocess
import sys
import threading
import time

import Quartz

from ferb.act.mac import apps

CHROME = "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"
PROFILE = "Profile 25"                                    # "FERB Test"
URL = "data:text/html,<title>ferb wake spike</title>spike"


def chrome_windows() -> int:
    pid = [a.pid for a in apps.running() if a.bundle_id == "com.google.Chrome"]
    wins = Quartz.CGWindowListCopyWindowInfo(Quartz.kCGWindowListOptionOnScreenOnly, 0)
    return sum(1 for w in wins if w["kCGWindowOwnerName"] == "Google Chrome" and w["kCGWindowLayer"] == 0)


def watch(cmd: list[str], seconds: float = 4.0) -> dict:
    owner = apps.frontmost().bundle_id
    fronts: list[str] = []
    stop = threading.Event()

    def poll() -> None:
        while not stop.is_set():
            f = apps.frontmost()
            fronts.append(f.bundle_id if f else "?")
            time.sleep(0.05)
    t = threading.Thread(target=poll); t.start()
    subprocess.Popen(cmd, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, start_new_session=True)
    time.sleep(seconds); stop.set(); t.join()
    return {"cmd": " ".join(c if " " not in c else repr(c) for c in cmd)[:110],
            "chrome_took_focus": "com.google.Chrome" in fronts, "owner_app_back_at_end": fronts[-1] == owner,
            "chrome_windows_on_screen": chrome_windows()}


METHODS = {
    "open_g_args": ["open", "-g", "-b", "com.google.Chrome", "--args", f"--profile-directory={PROFILE}", URL],
    "binary_url": [CHROME, f"--profile-directory={PROFILE}", URL],
    "binary_no_window": [CHROME, f"--profile-directory={PROFILE}", "--no-startup-window"],
    "open_g_no_window": ["open", "-g", "-b", "com.google.Chrome", "--args", f"--profile-directory={PROFILE}",
                         "--no-startup-window"],
}
which = sys.argv[1]
print(json.dumps({which: watch(METHODS[which])}))
