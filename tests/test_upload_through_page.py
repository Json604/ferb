"""HAND-500..502: uploads without the browser's file access. Live: no upload ever went through; every one ended with
DOM.setFileInputFiles answering "Not allowed" (task b1e8e376, 2026-11-11: four CSV imports into a Google Sheet refused,
then 9 minutes typing an array formula instead). Chromium answers that whenever the extension may not read local files
("Allow access to file URLs", off for a store install). From extension 0.19.9 FERB reads the file itself and hands its
bytes through the page: onto a file field (hidden, in a frame, several files) or as a drop.

The extension's own code under node (content.js's transfer and set; background.js's route), and the real daemon and
gateway over the fake extension: parts and their reassembly, the route each extension gets, the protection lists."""

import base64
import hashlib
import json
import subprocess
from pathlib import Path
from typing import Any

import pytest

from ferb.act.browser.ext_protocol import UPLOAD_PART
from tests.browserkit import bd, call  # noqa: F401  (the fixture and its helpers)
from tests.chromelab import EXT, SHOP, _node, _ready, worker
from tests.fake_extension import FakeExtension


# ---------- content.js: the frame keeps the parts, then gives the files to the element ----------

def _content(script: str) -> Any:
    """content.js's upload functions (describeFiles .. uploadSet) with a small DOM: elements, events, DataTransfer."""
    src = (EXT / "content.js").read_text()
    block = src[src.index("  // What a field (or a drop) now holds"):src.index("  function stamp(")]
    harness = r'''
class Event { constructor(type, init = {}) { Object.assign(this, init); this.type = type; this.defaultPrevented = false; }
              preventDefault() { if (this.cancelable) this.defaultPrevented = true; } }
class DragEvent extends Event {}
class DataTransfer { constructor() { this.files = []; this.items = {add: (f) => this.files.push(f)}; }
                     get types() { return this.files.length ? ["Files"] : []; } }
class Element {
  constructor(tag) { this.tagName = tag; this.listeners = {}; this.seen = []; }
  addEventListener(type, fn) { (this.listeners[type] = this.listeners[type] || []).push(fn); }
  dispatchEvent(e) { e.target = this; this.seen.push({type: e.type, bubbles: !!e.bubbles, composed: !!e.composed,
                                     files: e.dataTransfer ? e.dataTransfer.files.length : null});
                     for (const fn of this.listeners[e.type] || []) fn(e); return !e.defaultPrevented; }
  getBoundingClientRect() { return {x: 0, y: 0, width: 100, height: 40}; }
}
class HTMLInputElement extends Element {
  constructor(type, multiple = false) { super("INPUT"); this.type = type; this.multiple = multiple; this._files = []; }
  get files() { return this._files; } set files(v) { this._files = Array.from(v); }
}
const els = {};
const element = (gen, id) => (els[id] ? {el: els[id]} : {error: "stale"});
''' + block + r'''
const b64 = (s) => Buffer.from(s).toString("base64");
const T = "0123456789abcdef0123456789abcdef", U = "fedcba9876543210fedcba9876543210";
(async () => { const out = await (async () => {''' + script + r'''})();
  process.stdout.write(JSON.stringify(out) + "\n", () => process.exit(0)); })()
  .catch((e) => { console.error(e); process.exit(1); });
'''
    ran = subprocess.run([_node(), "-e", harness], capture_output=True, text=True, timeout=20)
    assert ran.returncode == 0, ran.stderr
    return json.loads(ran.stdout)


# decision: HAND-501
def test_a_hidden_multiple_field_gets_every_file_in_order_with_its_events() -> None:
    """Parts arrive in order and are joined per file; the field gets the files (a hidden one too: the snapshot's
    element), then input and change, bubbling and composed, as a person's pick in the chooser fires them."""
    out = _content(r'''
      els[1] = new HTMLInputElement("file", true);
      const sent = [uploadPart(T, 0, 0, b64("id,name\n")), uploadPart(T, 0, 8, b64("1,a\n")),
                    uploadPart(T, 1, 0, b64("%PDF-1.7"))];
      const r = await uploadSet(1, 1, T, [{name: "a.csv", type: "text/csv", size: 12, last_modified: 1000},
                                          {name: "b.pdf", type: "application/pdf", size: 8, last_modified: 2000}]);
      const texts = await Promise.all(els[1].files.map((f) => f.text()));
      return {sent, r, texts, names: els[1].files.map((f) => [f.name, f.type, f.lastModified]), seen: els[1].seen,
              again: await uploadSet(1, 1, T, [{name: "a.csv", size: 12}])};
    ''')
    assert [s["ok"] for s in out["sent"]] == [True, True, True] and out["sent"][1]["received"] == 12
    assert out["r"]["ok"] is True and out["r"]["route"] == "field" and out["r"]["files_set"] == 2
    assert out["texts"] == ["id,name\n1,a\n", "%PDF-1.7"]
    assert out["names"] == [["a.csv", "text/csv", 1000], ["b.pdf", "application/pdf", 2000]]
    assert [(e["type"], e["bubbles"], e["composed"]) for e in out["seen"]] == [("input", True, True),
                                                                              ("change", True, True)]
    assert [f["sha256"] for f in out["r"]["selected_files"]] == [hashlib.sha256(b"id,name\n1,a\n").hexdigest(),
                                                                 hashlib.sha256(b"%PDF-1.7").hexdigest()]
    assert out["again"]["reason"] == "upload_bytes_missing"       # a transfer is used once, then gone


# decision: HAND-501
def test_a_page_that_takes_the_files_and_empties_its_field_is_a_success_not_a_refusal() -> None:
    """Sites read the files in their change handler and then clear the field (so the same file can be picked again):
    what the field took is read before the page's handlers run, so that isn't reported as a failure and retried."""
    out = _content(r'''
      els[1] = new HTMLInputElement("file");
      let took = null;
      els[1].addEventListener("change", (e) => { took = e.target.files.map((f) => f.name); e.target.files = []; });
      uploadPart(T, 0, 0, b64("a,b\n"));
      const r = await uploadSet(1, 1, T, [{name: "a.csv", type: "text/csv", size: 4}]);
      return {r, took};
    ''')
    assert out["took"] == ["a.csv"]
    assert out["r"]["ok"] is True and out["r"]["files_set"] == 1 and out["r"]["files_after_change"] == 0
    assert out["r"]["selected_files"][0]["sha256"] == hashlib.sha256(b"a,b\n").hexdigest()


# decision: HAND-501
def test_a_part_out_of_order_an_incomplete_file_or_a_second_file_on_a_single_field_sets_nothing() -> None:
    out = _content(r'''
      els[1] = new HTMLInputElement("file");
      const skipped = [uploadPart(T, 0, 0, b64("abc")), uploadPart(T, 0, 9, b64("x"))];       // a part missing
      const late = uploadPart(T, 0, 3, b64("d"));                                             // the transfer went
      uploadPart(U, 0, 0, b64("abc"));
      const short = await uploadSet(1, 1, U, [{name: "a.txt", size: 4}]);
      uploadPart(T, 0, 0, b64("a")); uploadPart(T, 1, 0, b64("b"));
      const two = await uploadSet(1, 1, T, [{name: "a", size: 1}, {name: "b", size: 1}]);
      return {skipped, late, short, two, files: els[1].files.length, seen: els[1].seen.length};
    ''')
    assert out["skipped"][1] == {"ok": False, "detail": "part_out_of_order"}
    assert out["late"]["detail"] == "transfer_missing"
    assert out["short"]["reason"] == "upload_bytes_missing" and out["two"]["reason"] == "single_file_field"
    assert out["files"] == 0 and out["seen"] == 0                 # nothing reached the page


# decision: HAND-501
def test_a_drop_zone_gets_the_files_as_a_drop_and_one_that_ignores_it_is_a_known_failure() -> None:
    out = _content(r'''
      els[2] = new Element("DIV");
      let got = null;
      els[2].addEventListener("dragover", (e) => e.preventDefault());
      els[2].addEventListener("drop", (e) => { e.preventDefault(); got = e.dataTransfer.files.map((f) => f.name); });
      uploadPart(T, 0, 0, b64("hello"));
      const taken = await uploadSet(1, 2, T, [{name: "notes.txt", type: "text/plain", size: 5}]);
      els[3] = new Element("DIV");                                 // no listener takes it
      uploadPart(U, 0, 0, b64("hello"));
      const ignored = await uploadSet(1, 3, U, [{name: "notes.txt", size: 5}]);
      return {taken, got, seen: els[2].seen, ignored};
    ''')
    assert out["taken"]["ok"] is True and out["taken"]["route"] == "drop" and out["taken"]["drop_taken"] is True
    assert out["got"] == ["notes.txt"]
    assert [(e["type"], e["files"]) for e in out["seen"]] == [("dragenter", 1), ("dragover", 1), ("drop", 1)]
    assert out["ignored"]["ok"] is False and out["ignored"]["reason"] == "drop_not_taken"


# ---------- background.js: which route, and nothing tried without file access ----------

# decision: HAND-500
def test_without_file_access_the_debugger_route_is_never_tried_and_the_bytes_go_to_the_frame() -> None:
    out = worker(r'''(async () => {
      gen = 1; const sent = [], cmds = [], scripts = [];
      send = async (type, body) => { if (type === "result") sent.push(body); };
      chrome.extension = {isAllowedFileSchemeAccess: async () => false};
      chrome.debugger = new Proxy({attach: async () => {}, detach: async () => {},
        sendCommand: async (t, method) => { cmds.push(method); return {}; },
        onEvent: chrome.debugger.onEvent, onDetach: chrome.debugger.onDetach}, {get: (o, k) => o[k]});
      chrome.tabs.get = async (id) => ({id, windowId: 9, url: "https://s.example/"});
      inFrame = async (tab, frame, func, args) => { scripts.push([frame, String(func).match(/__ferb\.(\w+)/)[1], args]);
        return String(func).includes("uploadPart") ? {ok: true, received: 3}
             : String(func).includes("uploadSet") ? {ok: true, route: "field", files_set: 1} : true; };
      const result = async (id) => sent.find((b) => b.id === id);
      await onCommand({gen: 1, id: "u1", op: "upload", args: {tab_id: 8, frame_id: 3, snap: 1, el: 2, paths: ["/x/a"]}});
      const off = await result("u1");
      const T = "0123456789abcdef0123456789abcdef";
      await onCommand({gen: 1, id: "p1", op: "upload_part", args: {tab_id: 8, frame_id: 3, transfer: T, index: 0,
                                                                  offset: 0, data: btoa("abc")}});
      await onCommand({gen: 1, id: "p2", op: "upload_part", args: {tab_id: 8, frame_id: 3, transfer: "../x", index: 0,
                                                                  offset: 0, data: "YQ=="}});
      await onCommand({gen: 1, id: "u2", op: "upload", args: {tab_id: 8, frame_id: 3, snap: 1, el: 2, transfer: T,
                                                             files: [{name: "a", size: 3}]}});
      return {off, part: await result("p1"), bad: await result("p2"), set: await result("u2"), cmds,
              scripts: scripts.filter(([, fn]) => fn === "uploadPart" || fn === "uploadSet").map(([f, fn]) => [f, fn])};
    })()''')
    assert out["off"]["value"]["reason"] == "file_access_off" and out["off"]["outcome"] == "failed"
    assert "DOM.setFileInputFiles" not in out["cmds"] and "DOM.performSearch" not in out["cmds"]
    assert out["part"]["value"] == {"received": 3}
    assert out["bad"]["status"] == "error" and out["bad"]["outcome"] == "failed"     # nothing reached the page
    assert out["set"]["value"]["route"] == "field" and out["set"]["value"]["transferred"] is True
    assert out["scripts"] == [[3, "uploadPart"], [3, "uploadSet"]]                    # the target frame only


# ---------- the daemon: parts, routes, limits, through the real gateway ----------

def _ext(monkeypatch: pytest.MonkeyPatch, **kw: Any) -> None:
    """The next paired fake extension's version or file-access answer (an older one, or one with access)."""
    monkeypatch.setattr("tests.browserkit.FakeExtension", lambda home: _Fake(home, **kw))


class _Fake(FakeExtension):
    def __init__(self, home: Path, version: str | None = None, file_access: bool | None = None) -> None:
        super().__init__(home, **({"version": version} if version else {}))
        self.file_access = file_access


def _cmds(ext: Any, op: str) -> list[dict[str, Any]]:
    out = [json.loads(json.loads(m)["body"]) for m in ext.wire if '"type": "cmd"' in m]
    return [c for c in out if c["op"] == op]


def _page(d: Any, g: Any, path: str = "/uploads") -> tuple[str, dict[str, str]]:
    tab = call(d, g, "browser.open", url=f"{SHOP}{path}")["content"]["tab"]
    snap = call(d, g, "browser.snapshot", tab=tab)["content"]
    return str(tab), {e["label"]: e["id"] for e in snap["elements"]}


def _spans(d: Any) -> str:
    async def go() -> str:
        rows = d.daemon.db.conn.execute("SELECT input_json, output_json, error FROM spans").fetchall()
        return json.dumps([list(r) for r in rows]) + json.dumps([e.data for e in d.daemon._journal().entries()])
    return str(d.run(go()))


# decision: HAND-501, HAND-502
def test_a_file_travels_in_parts_to_the_task_tab_and_is_set_on_a_hidden_field_in_its_frame(  # noqa: F811
        bd: Any, tmp_path: Path) -> None:
    d, launched = bd
    ext, g = _ready(d, launched)
    data = bytes(range(256)) * (UPLOAD_PART * 2 // 256) + b"tail of the csv"          # three parts
    a, b = tmp_path / "laptops.csv", tmp_path / "notes.txt"
    a.write_bytes(data)
    b.write_bytes(b"")                                                               # an empty file is one empty part
    tab, ids = _page(d, g)
    r = call(d, g, "browser.act", tab=tab, element=ids["Attachments"], action="upload", files=[str(a), str(b)])
    assert not r["is_error"], r
    c = r["content"]
    assert c["route"] == "field" and c["files_set"] == 2 and c["bytes_match"] is True and c["transferred"] is True
    assert "whether the site accepted" in c["detail"] and c["next"]
    parts = ext.upload_parts
    assert [(p["index"], p["offset"], p["size"]) for p in parts] == [
        (0, 0, UPLOAD_PART), (0, UPLOAD_PART, UPLOAD_PART), (0, 2 * UPLOAD_PART, len(b"tail of the csv")), (1, 0, 0)]
    chrome_id = d.daemon.router._tabs[tab].chrome_id
    assert {(p["tab_id"], p["frame_id"]) for p in parts} == {(chrome_id, 3)}       # the task's tab, the field's frame
    assert len({p["transfer"] for p in parts}) == 1
    [up] = _cmds(ext, "upload")
    assert "paths" not in up["args"] and up["args"]["frame_id"] == 3
    assert [f["name"] for f in up["args"]["files"]] == ["laptops.csv", "notes.txt"]
    assert up["args"]["files"][0]["type"] == "text/csv"
    held = ext.page_files[(chrome_id, 0)]
    assert held[0]["sha256"] == hashlib.sha256(data).hexdigest() and held[1]["size"] == 0
    assert not any((d.daemon.home / "workspace" / "uploads").rglob("*.csv"))       # no copy on disk for this route
    trace = _spans(d)
    assert base64.b64encode(data[:UPLOAD_PART]).decode()[:64] not in trace       # no content in spans or journal


# decision: HAND-501
def test_a_drop_zone_takes_the_files_and_a_page_that_ignores_the_drop_is_a_known_failure(  # noqa: F811
        bd: Any, tmp_path: Path) -> None:
    d, launched = bd
    ext, g = _ready(d, launched)
    f = tmp_path / "report.pdf"
    f.write_bytes(b"%PDF-1.7 fixture")
    tab, ids = _page(d, g)
    taken = call(d, g, "browser.act", tab=tab, element=ids["Drop files here"], action="upload", files=[str(f)])
    assert not taken["is_error"] and taken["content"]["route"] == "drop", taken
    assert taken["content"]["bytes_match"] is True
    ignored = call(d, g, "browser.act", tab=tab, element=ids["Ignores drops"], action="upload", files=[str(f)])
    assert ignored["is_error"] and ignored["content"]["reason"] == "drop_not_taken"
    assert ignored["content"]["transferred"] is False and "file field" in ignored["content"]["next"]
    two = call(d, g, "browser.act", tab=tab, element=ids["One file"], action="upload", files=[str(f), str(f)])
    assert two["is_error"] and two["content"]["reason"] == "single_file_field"


# decision: HAND-500
def test_with_file_access_the_browser_sets_ferbs_copy_and_an_older_extension_keeps_that_route(  # noqa: F811
        bd: Any, tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    d, launched = bd
    _ext(monkeypatch, file_access=True)
    ext, g = _ready(d, launched)
    f = tmp_path / "cv.pdf"
    f.write_bytes(b"%PDF cv")
    tab, ids = _page(d, g, "/upload")
    r = call(d, g, "browser.act", tab=tab, element=ids["Choose file"], action="upload", files=[str(f)])
    assert not r["is_error"] and r["content"]["route"] == "debugger", r
    assert not ext.upload_parts and _cmds(ext, "upload")[0]["args"]["paths"]
    ext.file_access = False                       # the switch went off since the hello: nothing tried, a known failure
    r = call(d, g, "browser.act", tab=tab, element=ids["Choose file"], action="upload", files=[str(f)])
    assert r["is_error"] and r["content"]["reason"] == "file_access_off" and "again" in r["content"]["detail"], r
    r = call(d, g, "browser.act", tab=tab, element=ids["Choose file"], action="upload", files=[str(f)])
    assert not r["is_error"] and r["content"]["route"] == "field" and len(ext.upload_parts) == 1, r


# decision: HAND-500
def test_an_older_extension_never_gets_parts(  # noqa: F811
        bd: Any, tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    d, launched = bd
    _ext(monkeypatch, version="0.19.8")
    ext, g = _ready(d, launched)
    f = tmp_path / "cv.pdf"
    f.write_bytes(b"%PDF cv")
    tab, ids = _page(d, g, "/upload")
    r = call(d, g, "browser.act", tab=tab, element=ids["Choose file"], action="upload", files=[str(f)])
    assert not r["is_error"], r
    assert not ext.upload_parts and not _cmds(ext, "upload_part") and _cmds(ext, "upload")[0]["args"]["paths"]


# decision: HAND-500
def test_a_file_the_protection_lists_close_is_never_read_or_sent(bd: Any) -> None:  # noqa: F811
    d, launched = bd
    ext, g = _ready(d, launched)
    own = d.daemon.home / "config.toml"                    # FERB's own state: never read by its tools
    if not own.exists():
        own.write_text("[x]\n")
    tab, ids = _page(d, g)
    r = call(d, g, "browser.act", tab=tab, element=ids["Attachments"], action="upload", files=[str(own)])
    assert r["is_error"] and "FERB's own files" in json.dumps(r["content"]), r
    assert not ext.upload_parts and not _cmds(ext, "upload")


# decision: HAND-500
def test_the_open_handle_is_checked_again_so_a_path_swapped_after_the_gateway_is_never_sent(  # noqa: F811
        bd: Any, tmp_path: Path) -> None:
    """Rule 10: the file FERB really opened is checked against the lists (F_GETPATH) before a byte is read, on this
    route as on the debugger's: here the check says the opened file is closed."""
    import asyncio

    from ferb.core.errors import Refused
    d, launched = bd
    ext, g = _ready(d, launched)
    f = tmp_path / "a.txt"
    f.write_bytes(b"secret-ish")
    tab, _ = _page(d, g)
    router = d.daemon.router
    t = router._tabs[tab]
    with pytest.raises(Refused, match="closed by the lists"):
        asyncio.run(router._upload(t, 3, t.snap_gen, 0, (str(f),), "i-1",
                                   lambda opened, how: "closed by the lists", file_input=True))
    assert not ext.upload_parts and not _cmds(ext, "upload")


# decision: HAND-502
def test_files_over_the_stand_in_limit_are_refused_before_a_byte_is_sent(  # noqa: F811
        bd: Any, tmp_path: Path) -> None:
    d, launched = bd
    ext, g = _ready(d, launched)
    d.daemon.config.set("browser", "download_task_max_gb", 0.000001)    # 1,000 bytes, as the owner could set it
    f = tmp_path / "big.bin"
    f.write_bytes(b"x" * 1500)
    tab, ids = _page(d, g)
    r = call(d, g, "browser.act", tab=tab, element=ids["Attachments"], action="upload", files=[str(f)])
    assert r["is_error"] and "download_task_max_gb" in json.dumps(r["content"]), r
    assert not ext.upload_parts and not _cmds(ext, "upload")
