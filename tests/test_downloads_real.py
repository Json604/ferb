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
<a target=_blank href="/attach.bin?name=newtab.bin">New tab file</a>
<button onclick="window.open('/attach.bin?name=popup.bin')">Popup file</button>
<button onclick="window.open('/oauth')">Sign in</button>
<p id=status>not signed in</p>
<script>
addEventListener('message', (e) => { if (e.data === 'signed-in') document.getElementById('status').textContent = 'signed in via popup'; });
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
            p, _, query = self.path.partition("?")
            head, body = {"content-type": "text/html"}, PAGE
            if p == "/attach.bin":
                name = dict(q.split("=", 1) for q in query.split("&") if "=" in q).get("name", "report.bin")
                head, body = {"content-type": "application/octet-stream",
                              "content-disposition": f'attachment; filename="{name}"'}, BLOB
            elif p == "/data.csv":
                head, body = {"content-type": "text/csv; charset=utf-8"}, "é,😀\n".encode() * 50_000
            elif p == "/plain.txt":
                head, body = {"content-type": "text/plain"}, b"plain words"
            elif p == "/oauth":                                    # an OAuth-style popup: it answers its opener
                body = (b"<title>Sign in</title><script>window.opener && window.opener.postMessage('signed-in', '*');"
                        b"</script>signed in")
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


async def _live_task(d: Any, task_id: str) -> None:
    """A running task (only a live task's tab holds downloads)."""
    from datetime import timedelta
    from ferb.core.types import Identity, Task
    from ferb.orchestrator.tree import TaskRecord
    t = Task(task_id, None, "lab", "dev", Identity.OWNER, {"browser:read", "browser:act"}, "worker", f"req-{task_id}",
             [], d.daemon.clock.now() + timedelta(minutes=30))
    root = d.daemon.spans.start("task", task_id=task_id, input={})
    d.daemon.tasks.add(TaskRecord(t, d.daemon.clock.now(), root.id))


def test_downloads_in_a_task_tab_become_workspace_facts_and_never_a_browser_download(site: str) -> None:
    folder = Path(tempfile.mkdtemp(prefix="ferb-lab-dl-"))
    prefs = {"download": {"default_directory": str(folder), "prompt_for_download": False, "directory_upgrade": True}}
    with browser_lab(headless=True, prefs=prefs) as (d, chrome, front):
        window = int(chrome.evaluate(chrome.sw, "ferbWindow(true)"))
        driver = d.daemon.router
        root = d.daemon.home / "workspace" / "downloads"
        newtab_owner = chrome.owner_tab("chrome://newtab/")             # the owner's new-tab page (no debugger)
        d.run(_live_task(d, "lab-task"))

        async def click(tab: Any, label: str) -> None:
            snap = await driver.snapshot(tab)
            el = next(e for e in snap.elements if e.label == label)
            assert (await driver.act(driver.element_ref(tab, el.id), BrowserAction("click"), "")).ok
            await asyncio.sleep(2.5)

        async def opened(tab: Any, seconds: float = 10) -> list[dict[str, Any]]:
            for _ in range(int(seconds * 10)):
                if found := [o for o in driver.opened_for(tab) if o.get("tab")]:
                    return found
                await asyncio.sleep(0.1)
            return []

        async def run() -> dict[str, Any]:
            out: dict[str, Any] = {}
            tab = await driver.open_for(f"{site}/attach.bin", "lab-task", "Lab", window_id=window)
            out["opened"] = await driver.downloads_for(tab, 30)
            out["opened_url"] = driver.tab_url(tab.tab_id)
            await driver.navigate(tab, f"{site}/data.csv")
            out["navigated"] = await driver.downloads_for(tab, 30)
            await driver.navigate(tab, f"{site}/")
            for label in ("Save plain", "Export"):
                await click(tab, label)
                out[label] = await driver.downloads_for(tab, 30)
            d.daemon.config.set("browser", "download_max_mb", 1)
            await driver.navigate(tab, f"{site}/big")
            out["big"] = await driver.downloads_for(tab, 30)
            d.daemon.config.set("browser", "download_max_mb", 100)
            await driver.navigate(tab, f"{site}/")
            await click(tab, "Export detached")
            out["detached"] = await driver.downloads_for(tab, 30)
            await click(tab, "New tab file")                            # target=_blank: FERB opens it itself
            out["new_tabs"] = await opened(tab)
            new = driver.owned_by(out["new_tabs"][0]["tab"], "lab-task")
            out["in_new_tab"] = await driver.downloads_for(new, 30)
            await click(tab, "Popup file")                              # window.open: the page hook opens it blank
            out["popups"] = await opened(tab, 5)
            pop = driver.owned_by(out["popups"][0]["tab"], "lab-task") if out["popups"] else None
            out["in_popup"] = await driver.downloads_for(pop, 30) if pop else []
            await click(tab, "Sign in")                                 # its opener relationship must survive
            await asyncio.sleep(2)
            out["oauth_popup"] = await opened(tab, 5)
            out["oauth"] = "signed in via popup" in (await driver.snapshot(tab)).text
            borrowed = await driver.attach(f"chrome:Default:{newtab_owner}", "lab-task")
            await driver.navigate(borrowed, f"{site}/attach.bin?name=fromnewtab.bin")   # through a blank page
            out["from_new_tab_page"] = await driver.downloads_for(borrowed, 30)
            await driver.release("lab-task")
            return out

        out = d.run(run(), timeout=240)
        mid = _browser_files(folder)
        # after the task let go of the borrowed tab: its downloads are the owner's again
        chrome.evaluate(chrome.sw, f"chrome.tabs.update({newtab_owner}, {{url: '{site}/attach.bin?name=after.bin'}})"
                                   ".then(() => true)")
        owner = chrome.owner_tab("about:blank")                        # a tab FERB never drove
        chrome.evaluate(chrome.sw, f"chrome.tabs.update({owner}, {{url: '{site}/attach.bin?name=owner.bin'}}).then(() => true)")
        time.sleep(4)
        end = _browser_files(folder)
        print("\nD35 lab:", {k: [(f.get("state"), f.get("name"), f.get("bytes") or f.get("size")) for f in v]
                             if isinstance(v, list) and v and "state" in v[0] else v
                             for k, v in out.items()}, mid, end)

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
        [detached] = out["detached"]                                   # FileSaver style: the page hook brings it in
        assert detached["state"] == "taken" and detached["name"] == "detached.txt"
        assert Path(detached["path"]).read_bytes() == b"detached"
        [newtab] = out["in_new_tab"]
        assert newtab["state"] == "taken" and newtab["name"] == "newtab.bin"
        [fromnewtab] = out["from_new_tab_page"]
        assert fromnewtab["state"] == "taken" and fromnewtab["name"] == "fromnewtab.bin"
        [popup] = out["in_popup"]                                      # window.open: blank, armed, then loaded by FERB
        assert popup["state"] == "taken" and popup["name"] == "popup.bin"
        assert out["oauth"] is True and out["oauth_popup"]             # the OAuth-style popup still answered its opener
    assert out["opened_url"] in ("about:blank", "")                    # the page never became the download
    assert mid == [], mid                                              # nothing of the task's reached the browser's flow
    assert set(end) - set(mid) == {"after.bin", "owner.bin"}           # the owner's own downloads: untouched
