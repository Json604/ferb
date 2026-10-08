"""D35, REAL (pytest -m real): browser downloads in a headless lab Chrome (tests/chromelab.py: its own throwaway
--user-data-dir, the unpacked FERB extension, a test daemon) from a 127.0.0.1 server. No owner browser or profile is
touched and no window is shown. The lab profile's own download folder is a temp folder, so a download the browser makes
itself is visible as a file there (headless Chrome can't show a Save dialog: the on-screen check is the owner's).

Proves the chosen mechanism end to end: a download in a tab FERB drives (an attachment URL opened or navigated to, a
<a download> link, a page-made blob link) becomes a workspace file with facts and never a browser download; an owner tab
FERB doesn't drive downloads as before; the size ceiling refuses before bytes move; a detached script-made blob link,
which nothing tab-scoped can intercept, is reported as not taken (the measured residual).

    uv run pytest -m real tests/test_downloads_real.py -s
"""

import asyncio
import hashlib
import http.server
import os
import tempfile
import threading
import time
from pathlib import Path
from typing import Any, Iterator

import pytest

from ferb.core.types import BrowserAction
from tests.chromelab import browser_lab

pytestmark = pytest.mark.real

BLOB = os.urandom(300_000)
BIG = os.urandom(2 * 1024 * 1024 + 7)
PAGE = b"""<!doctype html><title>Downloads lab</title>
<a id=attr download href="/plain.txt?attr">Save plain</a>
<button onclick="exportAttached()">Export</button>
<button onclick="exportDetached()">Export detached</button>
<script>
function exportAttached() {          // a page-made file through a connected link, revoked at once
  const u = URL.createObjectURL(new Blob(['a,b\\n1,2\\n'], {type: 'text/csv'}));
  const a = document.createElement('a'); a.href = u; a.download = 'export.csv';
  document.body.appendChild(a); a.click(); URL.revokeObjectURL(u); a.remove();
}
function exportDetached() {          // FileSaver.js style: a link never in the document
  const a = document.createElement('a'); a.href = URL.createObjectURL(new Blob(['detached'], {type: 'text/plain'}));
  a.download = 'detached.txt'; a.dispatchEvent(new MouseEvent('click'));
}
</script>
"""


@pytest.fixture(scope="module")
def site() -> Iterator[str]:
    class H(http.server.BaseHTTPRequestHandler):
        def do_GET(self) -> None:  # noqa: N802
            p = self.path.split("?")[0]
            head, body = {"content-type": "text/html"}, PAGE
            if p == "/attach.bin":
                head, body = {"content-type": "application/octet-stream",
                              "content-disposition": 'attachment; filename="report.bin"'}, BLOB
            elif p == "/data.csv":
                head, body = {"content-type": "text/csv; charset=utf-8"}, "é,😀\n".encode() * 50_000
            elif p == "/plain.txt":
                head, body = {"content-type": "text/plain"}, b"plain words"
            elif p == "/big":
                head, body = {"content-type": "application/zip", "content-disposition": "attachment; filename=big.zip"}, BIG
            self.send_response(200)
            for k, v in head.items():
                self.send_header(k, v)
            self.send_header("content-length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)

        def log_message(self, *a: Any) -> None:
            pass
    srv = http.server.ThreadingHTTPServer(("127.0.0.1", 0), H)
    threading.Thread(target=srv.serve_forever, daemon=True).start()
    yield f"http://127.0.0.1:{srv.server_port}"
    srv.shutdown()


def _browser_files(folder: Path) -> list[str]:
    # Chrome's component updater may stage its own CRX files ("downloads.html…") here; they aren't page downloads.
    return sorted(p.name for p in folder.iterdir() if not p.name.startswith("downloads.html"))


def test_downloads_in_a_task_tab_become_workspace_facts_and_never_a_browser_download(site: str) -> None:
    folder = Path(tempfile.mkdtemp(prefix="ferb-lab-dl-"))
    prefs = {"download": {"default_directory": str(folder), "prompt_for_download": False, "directory_upgrade": True}}
    with browser_lab(headless=True, prefs=prefs) as (d, chrome, front):
        window = int(chrome.evaluate(chrome.sw, "ferbWindow(true)"))
        driver = d.daemon.router
        root = d.daemon.home / "workspace" / "downloads"

        async def run() -> dict[str, Any]:
            out: dict[str, Any] = {}
            tab = await driver.open_for(f"{site}/attach.bin", "lab-task", "Lab", window_id=window)
            out["opened"] = await driver.downloads_for(tab, 30)
            out["opened_url"] = driver.tab_url(tab.tab_id)
            await driver.navigate(tab, f"{site}/data.csv")
            out["navigated"] = await driver.downloads_for(tab, 30)
            await driver.navigate(tab, f"{site}/")
            snap = await driver.snapshot(tab)
            for label in ("Save plain", "Export"):
                el = next(e for e in snap.elements if e.label == label)
                assert (await driver.act(driver.element_ref(tab, el.id), BrowserAction("click"), "")).ok
                await asyncio.sleep(2)
                out[label] = await driver.downloads_for(tab, 30)
                snap = await driver.snapshot(tab)
            d.daemon.config.set("browser", "download_max_mb", 1)
            await driver.navigate(tab, f"{site}/big")
            out["big"] = await driver.downloads_for(tab, 30)
            d.daemon.config.set("browser", "download_max_mb", 100)
            el = next(e for e in snap.elements if e.label == "Export detached")
            assert (await driver.act(driver.element_ref(tab, el.id), BrowserAction("click"), "")).ok
            await asyncio.sleep(2)
            out["detached"] = await driver.downloads_for(tab, 30)
            await driver.close(tab)
            return out

        out = d.run(run(), timeout=180)
        before_owner = _browser_files(folder)
        owner = chrome.owner_tab("about:blank")                        # a tab FERB doesn't drive
        chrome.evaluate(chrome.sw, f"chrome.tabs.update({owner}, {{url: '{site}/attach.bin?owner'}}).then(() => true)")
        time.sleep(4)
        after_owner = _browser_files(folder)
        print("\nD35 lab:", {k: [(f["state"], f.get("name"), f.get("bytes") or f.get("size")) for f in v]
                             for k, v in out.items() if isinstance(v, list)}, before_owner, after_owner)

        [opened] = out["opened"]                                       # files checked while the lab home exists
        assert opened["state"] == "taken" and opened["name"] == "report.bin" and opened["bytes"] == len(BLOB)
        assert opened["sha256"] == hashlib.sha256(BLOB).hexdigest() and Path(opened["path"]).read_bytes() == BLOB
        assert Path(opened["path"]).parent == root and opened["origin"] == "external:download"
        [csv] = out["navigated"]
        assert csv["state"] == "taken" and Path(csv["path"]).read_bytes() == "é,😀\n".encode() * 50_000
        [plain] = out["Save plain"]
        assert plain["state"] == "taken" and Path(plain["path"]).read_bytes() == b"plain words"
        [blob] = out["Export"]
        assert blob["state"] == "taken" and blob["name"] == "export.csv"
        assert Path(blob["path"]).read_bytes() == b"a,b\n1,2\n"
        [big] = out["big"]
        assert big["state"] == "refused" and "path" not in big
        assert sorted(Path(f["path"]).name for f in (opened, csv, plain, blob)) == sorted(p.name for p in root.iterdir())
        [detached] = out["detached"]
        assert detached["state"] == "not_taken" and detached["name"] == "detached.txt"   # the measured residual
    # None of FERB's downloads reached the browser's own download flow; only the residual and the owner's tab did.
    assert out["opened_url"] in ("about:blank", "")                    # the page never became the download
    assert before_owner == ["detached.txt"], before_owner
    assert after_owner == ["detached.txt", "report.bin"], after_owner                # the owner's tab: untouched
