"""Lab (-m real): uploads with and without the browser's file access (HAND-500..502), in a throwaway headless Chrome.

Only a disposable --user-data-dir, the checkout's unpacked extension, a test daemon with a temporary home and a page
served from 127.0.0.1 are touched; never the owner's browsers, profiles or ~/.ferb. The lab profile's developer mode
is turned on (an unpacked extension stays on through the reload that the file-access switch causes) and the switch
is flipped through the extensions page's own API, as the owner's click on it would.

What it shows: the unpacked lab extension has file access, and the debugger's route sets the file; with the switch off
the browser answers DOM.setFileInputFiles with "Not allowed" (the live failure) and FERB hands the bytes through the
page instead: a hidden multiple field in a frame, a visible single field and a drop zone, each as the page's own
listeners see them.
"""

import asyncio
import http.server
import json
import tempfile
import threading
import time
from pathlib import Path
from typing import Any

import pytest

from ferb.act.browser.ext_protocol import FERB_EXTENSION_ID, UPLOAD_PART
from ferb.core.types import BrowserAction
from tests.chromelab import browser_lab, wait_for

pytestmark = pytest.mark.real

TOP = """<!doctype html><meta charset=utf-8><title>Upload lab</title>
<input type=file aria-label="One file" id=one>
<div role=button tabindex=0 aria-label="Drop files here" id=zone style="width:300px;height:80px;border:1px dashed">
  Drop files here</div>
<p id=log></p>
<iframe src="/child" style="width:400px;height:100px"></iframe>
<script>
const log = (s) => { document.getElementById('log').textContent += s + ';'; fetch('/seen?' + encodeURIComponent(s)); };
document.getElementById('one').addEventListener('change', (e) =>
  log('one:' + [...e.target.files].map(f => f.name + '/' + f.size).join(',') + ':trusted=' + e.isTrusted));
const zone = document.getElementById('zone');
zone.addEventListener('dragover', (e) => e.preventDefault());
zone.addEventListener('drop', (e) => { e.preventDefault();
  log('drop:' + [...e.dataTransfer.files].map(f => f.name + '/' + f.size).join(',')); });
</script>"""
CHILD = """<!doctype html><meta charset=utf-8><title>child</title>
<input type=file multiple aria-label="Hidden attach" id=hidden style="display:none">
<script>
document.getElementById('hidden').addEventListener('change', async (e) => {
  const parts = [];
  for (const f of e.target.files) parts.push(f.name + '/' + f.size + '/' + (await f.text()).length);
  fetch('/seen?' + encodeURIComponent('hidden:' + parts.join(',')));
});
</script>"""


def _serve() -> tuple[http.server.ThreadingHTTPServer, list[str]]:
    seen: list[str] = []

    class H(http.server.BaseHTTPRequestHandler):
        def do_GET(self) -> None:  # noqa: N802
            from urllib.parse import unquote, urlsplit
            u = urlsplit(self.path)
            if u.path == "/seen":
                seen.append(unquote(u.query))
            body = CHILD if u.path == "/child" else TOP
            self.send_response(200)
            self.send_header("content-type", "text/html; charset=utf-8")
            self.end_headers()
            self.wfile.write(body.encode())

        def log_message(self, *a: Any) -> None:
            pass
    srv = http.server.ThreadingHTTPServer(("127.0.0.1", 0), H)
    threading.Thread(target=srv.serve_forever, daemon=True).start()
    return srv, seen


def _switch(chrome: Any, on: bool, poke: str) -> None:
    """The owner's "Allow access to file URLs" switch, through chrome://extensions' own API (lab profile only)."""
    target = chrome.send("Target.createTarget", {"url": f"chrome://extensions/?id={FERB_EXTENSION_ID}"})["targetId"]
    sid = chrome.send("Target.attachToTarget", {"targetId": target, "flatten": True})["sessionId"]
    wait_for(lambda: _ready_api(chrome, sid), 10, "the extensions page never loaded")
    chrome.evaluate(sid, "chrome.developerPrivate.updateProfileConfiguration({inDeveloperMode: true}).then(() => true)")
    chrome.evaluate(sid, f"chrome.developerPrivate.updateExtensionConfiguration({{extensionId: '{FERB_EXTENSION_ID}', "
                         f"fileAccess: {str(on).lower()}}}).then(() => true)")
    chrome.send("Target.closeTarget", {"targetId": target})
    time.sleep(1)
    # the switch reloads the extension; its worker starts again on the next tab load, as in real use
    loaded = chrome.send("Target.createTarget", {"url": poke})["targetId"]
    time.sleep(2)
    chrome.send("Target.closeTarget", {"targetId": loaded})


def _ready_api(chrome: Any, sid: str) -> bool:
    try:
        return bool(chrome.evaluate(sid, "!!(chrome.developerPrivate && chrome.developerPrivate.updateExtensionConfiguration)"))
    except Exception:
        return False


def test_uploads_with_and_without_file_access_in_a_lab_chrome() -> None:
    srv, seen = _serve()
    base = f"http://127.0.0.1:{srv.server_port}"
    out: dict[str, Any] = {}
    try:
        with tempfile.TemporaryDirectory(prefix="ferb-upload-lab-") as tmp:
            csv = Path(tmp) / "laptops.csv"
            csv.write_bytes(b"model,price\n" + b"x" * (UPLOAD_PART + 1000))          # two parts
            note = Path(tmp) / "note.txt"
            note.write_bytes(b"hello from the lab")
            with browser_lab(headless=True) as (d, chrome, front):
                driver = d.daemon.router
                window = int(chrome.evaluate(chrome.sw, "ferbWindow(true)"))
                br = d.daemon.hub.bridges[("chrome", "Default")]

                async def upload(label: str, files: list[Path]) -> dict[str, Any]:
                    tab = await driver.open_for(f"{base}/", "lab-upload", "Lab", window_id=window)
                    try:
                        snap = await driver.snapshot(tab)
                        [el] = [e for e in snap.elements if e.label == label]
                        try:
                            r = await driver.act(driver.element_ref(tab, el.id),
                                                 BrowserAction("upload", files=tuple(str(f) for f in files)), "")
                            return {"ok": r.ok, "detail": r.detail, **{k: r.facts.get(k) for k in (
                                "route", "files_set", "bytes_match", "drop_taken", "reason", "observed")}}
                        except Exception as e:
                            return {"error": f"{type(e).__name__}: {e}"}
                    finally:
                        await driver.close(tab)

                async def forced_debugger() -> Any:
                    """The debugger's route as 0.19.8 always tried it, asked for directly (lab only)."""
                    tab = await driver.open_for(f"{base}/", "lab-upload", "Lab", window_id=window)
                    try:
                        snap = await driver.snapshot(tab)
                        [el] = [e for e in snap.elements if e.label == "One file"]
                        t, frame, gen, local, _ = driver._resolve(driver.element_ref(tab, el.id))
                        return await asyncio.to_thread(chrome.evaluate, chrome.sw, (
                            f"commands.uploadPaths({{tab_id: {t.chrome_id}, frame_id: {frame}, snap: {gen}, el: {local},"
                            f" paths: [{json.dumps(str(note))}]}}).then(r => r, e => 'error: ' + e.message)"))
                    finally:
                        await driver.close(tab)

                out["access_at_start"] = br.file_access
                out["with_access"] = d.run(upload("One file", [note]), timeout=90)
                gen = br.gen
                _switch(chrome, False, f"{base}/poke")
                chrome.sw = chrome.worker_session(FERB_EXTENSION_ID)
                wait_for(lambda: br.gen != gen and br.conn is not None, 30, "the extension never came back")
                out["access_after_off"] = br.file_access
                out["debugger_without_access"] = d.run(forced_debugger(), timeout=90)
                out["field"] = d.run(upload("One file", [note]), timeout=90)
                out["hidden_in_frame"] = d.run(upload("Hidden attach", [csv, note]), timeout=90)
                out["drop"] = d.run(upload("Drop files here", [note]), timeout=90)
                time.sleep(0.5)
                out["page_saw"] = list(seen)
                out["focus_changes"] = front.stop()
    finally:
        srv.shutdown()
    print(json.dumps(out, indent=2))
    assert out["access_at_start"] is True and out["with_access"]["route"] == "debugger" and out["with_access"]["ok"]
    assert out["access_after_off"] is False
    assert "Not allowed" in str(out["debugger_without_access"])                 # the live failure, reproduced
    assert out["field"]["ok"] and out["field"]["route"] == "field" and out["field"]["bytes_match"] is True
    assert out["hidden_in_frame"]["ok"] and out["hidden_in_frame"]["files_set"] == 2
    assert out["drop"]["ok"] and out["drop"]["route"] == "drop" and out["drop"]["drop_taken"] is True
    size = len(b"model,price\n") + UPLOAD_PART + 1000
    assert f"hidden:laptops.csv/{size}/{size},note.txt/18/18" in out["page_saw"]
    assert "one:note.txt/18:trusted=false" in out["page_saw"] and "drop:note.txt/18" in out["page_saw"]
    assert not front.took_focus(chrome.pid)
