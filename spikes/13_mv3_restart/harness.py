"""Spike 13 test equipment: a spare Chrome (temporary --user-data-dir) driven over the CDP pipe only to load the
extension and to kill its service worker on demand. FERB itself never debugs the owner's browsers; this is a lab.

The native-host manifest goes into the spare profile's own NativeMessagingHosts folder (Chrome reads
<user-data-dir>/NativeMessagingHosts, spike 4), so nothing is written into the owner's browser folders.
"""

import json
import shutil
import stat
import sys
import tempfile
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "03_cdp_identity"))
from cdp_pipe import ChromePipe  # noqa: E402

from daemon13 import Daemon  # noqa: E402

HERE = Path(__file__).resolve().parent
EXT_ID = "hdeghaalhjpjaannocdeikoiallfbnnc"
HOST_NAME = "com.ferb.spike13"


class Lab:
    def __init__(self) -> None:
        self.run = Path(tempfile.mkdtemp(prefix="ferb13-"))        # short path, outside TCC folders
        profile = self.run / "profile"
        (profile / "NativeMessagingHosts").mkdir(parents=True)
        shutil.copy(HERE / "host.py", self.run / "host.py")
        sock = self.run / "d.sock"
        launcher = self.run / "host.sh"
        launcher.write_text(f'#!/bin/sh\nexec "{sys.executable}" "{self.run / "host.py"}" "{sock}" "$@"\n')
        launcher.chmod(stat.S_IRWXU)
        (profile / "NativeMessagingHosts" / f"{HOST_NAME}.json").write_text(json.dumps({
            "name": HOST_NAME, "description": "FERB spike 13 host", "path": str(launcher), "type": "stdio",
            "allowed_origins": [f"chrome-extension://{EXT_ID}/"]}))
        self.daemon = Daemon(sock).start()
        self.chrome = ChromePipe(profile, headless=False, extra_args=["--enable-unsafe-extension-debugging"])
        assert self.chrome.send("Extensions.loadUnpacked", {"path": str(HERE / "ext")})["id"] == EXT_ID

    def worker_target(self) -> str | None:
        for t in self.chrome.send("Target.getTargets")["targetInfos"]:
            if t["type"] == "service_worker" and t["url"].startswith(f"chrome-extension://{EXT_ID}/"):
                return t["targetId"]
        return None

    def kill_worker(self) -> None:
        tid = self.worker_target()
        assert tid, "no worker running"
        self.chrome.send("Target.closeTarget", {"targetId": tid})

    def open_page(self, url: str) -> None:
        """A plain tab load in the spare browser (a wake-up event for the extension)."""
        self.chrome.send("Target.createTarget", {"url": url, "background": True})

    def page_urls(self) -> list[str]:
        return [t["url"] for t in self.chrome.send("Target.getTargets")["targetInfos"] if t["type"] == "page"]

    def close(self) -> None:
        self.chrome.close()
        self.daemon.close()


def wait_until(pred, timeout: float = 10, step: float = 0.1):
    end = time.monotonic() + timeout
    while time.monotonic() < end:
        v = pred()
        if v:
            return v
        time.sleep(step)
    return None
