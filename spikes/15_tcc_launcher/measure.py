"""Spike 15 harness: build/sign the launcher into ~/.ferb/spike15, run launcher -> python probe under a temporary
LaunchAgent (so the launcher, not the terminal, is the responsible process), print what the probe saw."""
import json, os, plistlib, shutil, subprocess, sys, time
from pathlib import Path

HERE = Path(__file__).resolve().parent
DIR = Path.home() / ".ferb" / "spike15"
LAUNCHER = DIR / "ferb-launcher"
LABEL = "com.ferb.spike15"
PLIST = Path.home() / "Library" / "LaunchAgents" / f"{LABEL}.plist"
DOMAIN = f"gui/{os.getuid()}"


def build() -> str:
    DIR.mkdir(parents=True, exist_ok=True)
    tmp = DIR / "ferb-launcher.new"
    subprocess.run(["cc", "-O2", "-o", str(tmp), str(HERE / "launcher.c")], check=True)
    subprocess.run(["codesign", "-s", "-", "-f", str(tmp)], check=True, capture_output=True)
    os.replace(tmp, LAUNCHER)                                  # same path, new binary
    shutil.copy(HERE / "probe.py", DIR / "probe.py")
    cd = subprocess.run(["codesign", "-dvvv", str(LAUNCHER)], capture_output=True, text=True).stderr
    return next(l for l in cd.splitlines() if l.startswith("CDHash="))


def run(python: str) -> dict:
    out = DIR / "result.json"
    out.unlink(missing_ok=True)
    PLIST.write_bytes(plistlib.dumps({"Label": LABEL, "ProgramArguments": [str(LAUNCHER), python, str(DIR / "probe.py"),
                                      str(out)], "RunAtLoad": True, "StandardErrorPath": str(DIR / "err.log")}))
    subprocess.run(["launchctl", "bootout", f"{DOMAIN}/{LABEL}"], capture_output=True)
    subprocess.run(["launchctl", "bootstrap", DOMAIN, str(PLIST)], check=True, capture_output=True)
    try:
        for _ in range(100):
            if out.exists() and out.stat().st_size:
                return json.loads(out.read_text())
            time.sleep(0.1)
        return {"error": (DIR / "err.log").read_text()[-300:] if (DIR / "err.log").exists() else "no result"}
    finally:
        subprocess.run(["launchctl", "bootout", f"{DOMAIN}/{LABEL}"], capture_output=True)
        PLIST.unlink(missing_ok=True)


if __name__ == "__main__":
    cmd = sys.argv[1]
    if cmd == "build":
        print("built", LAUNCHER, build())
    elif cmd == "run":
        print(json.dumps(run(sys.argv[2])))
