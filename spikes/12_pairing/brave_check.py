"""Pre-step-6 Brave pairing check (owner-scheduled): one temporary Brave profile, never the owner's own profile.

1. Brave must already be running (never started, quit or killed here).
2. Hands `--profile-directory="FERB Brave Check"` + a blank local page to the running Brave (creates the profile).
3. Native-host manifest: Brave's own NativeMessagingHosts folder first; only if no hello arrives, Chrome's (one at a
   time, to measure which Brave reads). Both removed at the end.
4. Waits for the owner to load spike 12's extension (seen in the temp profile's own Preferences), then pairs it via
   the extension's pairing page and checks a signed tab event is attributed to (brave, that folder).
The owner deletes the temporary profile in Brave afterwards (FERB never deletes folders under a running browser).
"""

import http.server
import json
import secrets
import subprocess
import threading
import time
from pathlib import Path

from daemon_stub import EXT_ID, HOST_NAME, RUN, SOCK, Daemon, stop_stale_hosts

BRAVE = "/Applications/Brave Browser.app/Contents/MacOS/Brave Browser"
BRAVE_DATA = Path.home() / "Library/Application Support/BraveSoftware/Brave-Browser"
FOLDER = "FERB Brave Check"
LOCATIONS = {"brave": BRAVE_DATA / "NativeMessagingHosts",
             "chrome": Path.home() / "Library/Application Support/Google/Chrome/NativeMessagingHosts"}
LOG = Path(__file__).parent / "brave_check.log"


def log(msg: str) -> None:
    line = f"{time.strftime('%H:%M:%S')} {msg}"
    print(line, flush=True)
    with open(LOG, "a") as f:
        f.write(line + "\n")


def brave_running() -> bool:
    return subprocess.run(["pgrep", "-x", "Brave Browser"], capture_output=True).returncode == 0


def hand_to_brave(url: str) -> None:
    """Only ever hands a URL to the owner's running Brave (the launcher exits after handing off)."""
    if not brave_running():
        raise SystemExit("Brave is not running: start it yourself, then rerun")
    subprocess.Popen([BRAVE, f"--profile-directory={FOLDER}", url], stdout=subprocess.DEVNULL,
                     stderr=subprocess.DEVNULL, start_new_session=True)


def extension_loaded() -> bool:
    for name in ("Secure Preferences", "Preferences"):
        try:
            prefs = json.loads((BRAVE_DATA / FOLDER / name).read_text())
        except (OSError, ValueError):
            continue
        if EXT_ID in prefs.get("extensions", {}).get("settings", {}):
            return True
    return False


def write_manifest(where: str) -> Path:
    stop_stale_hosts()
    RUN.mkdir(mode=0o700, exist_ok=True)
    import shutil
    import sys
    shutil.copy(Path(__file__).parent / "host.py", RUN / "host.py")
    launcher = RUN / "host.sh"
    launcher.write_text(f'#!/bin/sh\nexec "{sys.executable}" "{RUN / "host.py"}" "{SOCK}" "$@" 2>>"{RUN / "host.err"}"\n')
    launcher.chmod(0o700)
    folder = LOCATIONS[where]
    created = not folder.exists()
    folder.mkdir(parents=True, exist_ok=True)
    path = folder / f"{HOST_NAME}.json"
    path.write_text(json.dumps({"name": HOST_NAME, "description": "FERB Brave check host", "path": str(launcher),
                                "type": "stdio", "allowed_origins": [f"chrome-extension://{EXT_ID}/"]}))
    log(f"manifest in {where} folder ({'created the folder' if created else 'folder existed'})")
    return path


def main() -> None:
    LOG.unlink(missing_ok=True)
    if not brave_running():
        log("Brave is not running — stopping (the check never starts Brave).")
        return

    class H(http.server.BaseHTTPRequestHandler):
        def do_GET(self) -> None:  # noqa: N802
            self.send_response(200)
            self.send_header("content-type", "text/html")
            self.end_headers()
            self.wfile.write(b"<title>FERB Brave check</title><p>Temporary profile for FERB's Brave check.</p>")

        def log_message(self, *a: object) -> None:
            pass
    srv = http.server.ThreadingHTTPServer(("127.0.0.1", 0), H)
    threading.Thread(target=srv.serve_forever, daemon=True).start()
    page = f"http://127.0.0.1:{srv.server_port}/"

    existed = (BRAVE_DATA / FOLDER).exists()
    hand_to_brave(page)
    for _ in range(60):
        if (BRAVE_DATA / FOLDER).exists():
            break
        time.sleep(0.5)
    log(f"temporary profile folder: {'already existed' if existed else 'created' if (BRAVE_DATA / FOLDER).exists() else 'NOT created'}")

    d = Daemon()
    manifests: list[Path] = []
    created_dirs = [p for p in LOCATIONS.values() if not p.exists()]
    try:
        manifests.append(write_manifest("brave"))
        log("READY: load spike 12's extension (spikes/12_pairing/ext) unpacked in the FERB Brave Check window")
        while not extension_loaded():
            time.sleep(1)
        log("extension seen in the temporary profile")
        read_from = None
        for where in ("brave", "chrome"):
            if where == "chrome":
                manifests.pop().unlink(missing_ok=True)
                manifests.append(write_manifest("chrome"))
            try:
                hello = d.wait(lambda e: e.msg.get("type") == "hello", 150)
                read_from = where
                log(f"hello received with the manifest in the {where} folder: {hello.msg['body_json']}")
                break
            except TimeoutError:
                log(f"no hello in 150 s with the manifest in the {where} folder")
        if read_from is None:
            log("RESULT: Brave never reached the native host")
            return
        token = d.pairing.issue_token("brave", FOLDER)
        hand_to_brave(f"chrome-extension://{EXT_ID}/pair.html#token={token}")
        ev = d.wait(lambda e: e.verdict.reason == "paired" and e.verdict.binding == ("brave", FOLDER), 90)
        install = ev.msg["install_id"]
        ack = d.wait(lambda e: e.msg["type"] == "paired_ack" and e.msg["install_id"] == install, 30)
        log(f"paired: binding {ack.verdict.binding}, signed ack accepted={ack.verdict.accepted}")
        marker = page + "?m=" + secrets.token_hex(4)
        d.act(install, {"type": "open", "id": 1, "url": marker})
        tev = d.wait(lambda e: e.msg["type"] == "tab_event" and marker in e.msg["body_json"], 30)
        log(f"signed tab event accepted={tev.verdict.accepted}, attributed to {tev.verdict.binding}")
        ok = ack.verdict.accepted and tev.verdict.accepted and tev.verdict.binding == ("brave", FOLDER)
        log(f"RESULT: {'PASS' if ok else 'FAIL'} (Brave reads the native-host manifest from the {read_from} folder)")
    finally:
        for m in manifests:
            m.unlink(missing_ok=True)
        for p in created_dirs:
            try:
                p.rmdir()
            except OSError:
                pass
        d.close()
        stop_stale_hosts()
        srv.shutdown()
        log("cleanup: manifest removed, relay stopped; the temporary profile is left for the owner to delete in Brave")


if __name__ == "__main__":
    main()
