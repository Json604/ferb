"""Spike 16: does FERB's Google session survive 30+ minutes when driven through an extension in the ORIGINAL "FERB"
profile of the owner's normal Chrome (no copy, no CDP launch)?

  python runner.py install     # host + native-host manifest (temporary; Chrome's default NativeMessagingHosts)
  python runner.py run         # stand-in daemon: a round every 5 min for 35 min, results to log.jsonl
  python runner.py uninstall   # remove both

Each round opens background tabs (feed, myaccount, Gmail inbox) in that profile, reads them, closes them. Only
booleans are logged: signed in as FERB's address, navigator.webdriver. Never starts, quits or focuses Chrome.
"""

import json
import os
import shutil
import socket
import stat
import sys
import threading
import time
from pathlib import Path

HOME = Path.home() / ".ferb" / "spike16"
SOCK = HOME / "d.sock"
LOG = HOME / "log.jsonl"
HOSTS = Path.home() / "Library/Application Support/Google/Chrome/NativeMessagingHosts"
MANIFEST = HOSTS / "com.ferb.spike16.json"
PY = next((Path.home() / ".ferb/python").glob("cpython-3.12*/bin/python3.12"))
HERE = Path(__file__).parent
ADDRESS = "ferb.test@example.com"   # set to the FERB profile's own Google account
URLS = {"feed": "https://mail.google.com/mail/feed/atom", "myaccount": "https://myaccount.google.com/",
        "inbox": "https://mail.google.com/mail/u/0/#inbox"}


def install() -> None:
    HOME.mkdir(parents=True, exist_ok=True, mode=0o700)
    shutil.copy(HERE / "host.py", HOME / "host.py")
    launcher = HOME / "host.sh"
    launcher.write_text(f'#!/bin/sh\nexec "{PY}" "{HOME / "host.py"}" "{SOCK}" 2>>"{HOME / "host.err"}"\n')
    launcher.chmod(0o700)
    ext_id = (HERE / "ext_id.txt").read_text().strip()
    HOSTS.mkdir(parents=True, exist_ok=True)
    MANIFEST.write_text(json.dumps({"name": "com.ferb.spike16", "description": "FERB spike 16 relay",
                                    "path": str(launcher), "type": "stdio",
                                    "allowed_origins": [f"chrome-extension://{ext_id}/"]}, indent=2))
    print(f"installed; extension id {ext_id}")


def uninstall() -> None:
    MANIFEST.unlink(missing_ok=True)
    shutil.rmtree(HOME, ignore_errors=True)
    print("removed native-host manifest and ~/.ferb/spike16")


class Conn:
    def __init__(self) -> None:
        self.f = None
        self.lock = threading.Lock()
        self.waiters: dict[int, list] = {}
        self.n = 0
        self.ready = threading.Event()

    def serve(self) -> None:
        SOCK.unlink(missing_ok=True)
        srv = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        srv.bind(str(SOCK))
        os.chmod(SOCK, stat.S_IRUSR | stat.S_IWUSR)
        srv.listen(1)
        while True:
            c, _ = srv.accept()
            f = c.makefile("rw")
            for line in f:
                msg = json.loads(line)
                if msg.get("type") == "hello":
                    self.f = f
                    self.ready.set()
                    log({"event": "hello", "version": msg.get("version"), "last_disconnect": msg.get("lastDisconnect")})
                elif "id" in msg and msg["id"] in self.waiters:
                    box = self.waiters.pop(msg["id"])
                    box.append(msg)
            self.ready.clear()
            self.f = None
            for box in list(self.waiters.values()):
                box.append({"type": "error", "error": "connection dropped"})
            self.waiters.clear()
            log({"event": "extension disconnected"})

    def call(self, typ: str, timeout: float = 60, **args: object) -> dict:
        if not self.ready.wait(600):
            raise RuntimeError("extension not connected")
        with self.lock:
            self.n += 1
            mid, box = self.n, []
            self.waiters[mid] = box
            self.f.write(json.dumps({"id": mid, "type": typ, "args": args}) + "\n")
            self.f.flush()
        end = time.monotonic() + timeout
        while not box and time.monotonic() < end:
            time.sleep(0.05)
        if not box:
            raise TimeoutError(typ)
        if box[0]["type"] == "error":
            raise RuntimeError(box[0]["error"])
        return box[0]["result"]


def log(entry: dict) -> None:
    entry = {"t": time.strftime("%H:%M:%S"), **entry}
    with open(LOG, "a") as f:
        f.write(json.dumps(entry) + "\n")
    print(json.dumps(entry), flush=True)


def one_round(conn: Conn, n: int) -> dict:
    out: dict = {"round": n}
    for name, url in URLS.items():
        try:
            tab = conn.call("open", url=url)["tabId"]
            time.sleep(3)
            r = conn.call("read", tabId=tab)
            conn.call("close", tabId=tab)
        except Exception as e:
            out[name] = f"error: {e}"
            continue
        if name == "feed":
            ok = "<feed" in r["source"] and f"inbox for {ADDRESS}" in r["source"].lower()
        elif name == "myaccount":
            ok = r["url"].startswith("https://myaccount.google.com") and "/account/about" not in r["url"]
        else:
            ok = "mail.google.com/mail" in r["url"] and ("inbox" in r["title"].lower() or ADDRESS in r["title"].lower())
        out[name] = ok
        out["webdriver"] = out.get("webdriver", False) or bool(r["webdriver"])
    return out


def run(minutes: float = 35, every: float = 300) -> None:
    HOME.mkdir(parents=True, exist_ok=True, mode=0o700)
    conn = Conn()
    threading.Thread(target=conn.serve, daemon=True).start()
    log({"event": "waiting for the extension (load or reload it in the FERB profile)"})
    conn.ready.wait()
    start, n = time.monotonic(), 0
    while True:
        log(one_round(conn, n))
        n += 1
        if time.monotonic() - start >= minutes * 60:
            break
        time.sleep(max(0, start + n * every - time.monotonic()))
    log({"event": "done", "elapsed_min": round((time.monotonic() - start) / 60, 1)})


if __name__ == "__main__":
    {"install": install, "uninstall": uninstall, "run": run}[sys.argv[1]]()
