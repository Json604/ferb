"""Lab (-m real): the web's input surfaces, driven through FERB's real browser tools as a worker would, to find and prove
where they fail (gap hunt 2026-11-11, after task 9e7c14ed spent 8 minutes failing to put text into Google Docs).

A throwaway Chrome (its own --user-data-dir), the checkout's unpacked extension, a test daemon with a temp home, and
pages from tests/browser_input_lab served on 127.0.0.1 (two ports: two origins). The pages load real open-source
editors and widgets from public CDNs at pinned versions (jsdelivr, esm.sh, cdnjs, unpkg, cdn.ckeditor.com): loading
them is the only traffic to third parties; nothing is signed in to or sent anywhere else. Never the owner's browsers,
profiles or ~/.ferb.

Each probe records what the worker saw (tool results, timings) and what the page's own model holds (window.labState),
and asserts TODAY's behaviour, so a probe marked GAP fails once FERB is fixed: update it then.

    uv run pytest -m real tests/test_browser_input_gaps_real.py -s
"""

import time
from collections.abc import Iterator
from typing import Any

import pytest

from ferb.act.browser.tools import OWNER_BROWSER as OWNER
from ferb.act.browser.tools import BrowserSnapshot
from tests.browser_input_gaps_kit import LabServer, Worker, report
from tests.chromelab import browser_lab

pytestmark = pytest.mark.real

PARA = ("First paragraph of the lab note, long enough to wrap: FERB types this like a person would.\n\n"
        "Second paragraph after a blank line.\nThird line, a single break.")


@pytest.fixture(scope="module")
def lab() -> Iterator[Any]:
    with browser_lab() as isolated:
        yield isolated


@pytest.fixture(scope="module")
def site() -> Iterator[LabServer]:
    s = LabServer()
    yield s
    s.close()


@pytest.fixture
def worker(lab: Any, tmp_path: Any, request: Any) -> Iterator[Worker]:
    d, chrome, _ = lab
    w = Worker(d, chrome, task=request.node.name.replace("[", "-").replace("]", "")[:60], workspace=str(tmp_path))
    yield w
    w.close()


EDITORS = ["plain", "quill", "prosemirror", "tiptap", "lexical", "slate", "draft", "ckeditor", "tinymce",
           "monaco", "cm6", "cm5", "ace"]


def _editor_target(w: Worker) -> Any:
    """The element a worker would pick: the editing surface the snapshot names (textbox/code), else anything editable."""
    for role in ("textbox", "code", "document", "generic"):
        for e in w.snap.elements:
            if e.role == role and (role == "textbox" or "editor" in e.label.lower()):
                return e
    return None


@pytest.mark.parametrize("editor", EDITORS)
def test_type_multiline_into_editor(worker: Worker, site: LabServer, editor: str) -> None:
    """Type a three-paragraph text into each editor, replace it, then press Enter and add a line."""
    w = worker
    w.open(f"{site.a}/editors.html?e={editor}", wait_ready=25)
    truth0 = w.truth()
    w.snapshot()
    target = _editor_target(w)
    out: dict[str, Any] = {"loaded": truth0 and truth0.get("ready"), "load_error": None,
                           "target": target and {"id": target.id, "role": target.role, "label": target.label,
                                                 "in_view": target.in_view, "occluded": target.occluded}}
    if not (truth0 and truth0.get("ready")):
        out["load_error"] = w.evaluate("() => window.labError || document.getElementById('status').textContent")
        report(f"editor:{editor}", w, out)
        pytest.skip(f"{editor} didn't load: {out['load_error']}")
    assert target is not None, [(e.role, e.label) for e in w.snap.elements][:40]
    r1 = w.act(target.id, "type", text=PARA)
    t1 = w.truth()
    out["after_type"] = {"text": t1["text"], "events": _summary(t1["events"])}
    w.snapshot()
    target2 = _editor_target(w)
    r2 = w.act(target2.id, "type", text="Replaced text.", mode="replace")
    t2 = w.truth()
    out["after_replace"] = t2["text"]
    w.snapshot()
    target3 = _editor_target(w)
    r3 = w.act(target3.id, "press", text="End") if target3 else None
    r4 = w.act(target3.id, "press", text="Enter") if target3 else None
    t4 = w.truth()
    out["after_enter"] = t4["text"]
    w.snapshot()
    target5 = _editor_target(w)
    r5 = w.act(target5.id, "type", text="Added line.") if target5 else None   # HAND-505: an editor's default: insert
    out["after_insert"] = w.truth()["text"]
    out["results"] = [r1, r2, r3, r4, r5]
    report(f"editor:{editor}", w, out)
    # every editor takes the three paragraphs; a code editor's hidden textarea, under its drawn text, is focused by
    # script and takes the keys there (HAND-504; it was refused as "target_occluded")
    assert r1.get("ok"), r1
    assert "Second paragraph after a blank line." in str(out["after_type"]["text"]), out["after_type"]
    assert "Replaced text." in str(out["after_replace"]) and "First paragraph" not in str(out["after_replace"])
    if editor in HIDDEN_INPUT_EDITORS:
        assert r1.get("unpointed") == "covered" and r3 and r3.get("ok") and r4 and r4.get("ok"), (r1, r3, r4)
    assert r5 and r5.get("mode") == "insert", r5          # typing into an editor keeps what is there
    assert "Replaced text." in str(out["after_insert"]) and "Added line." in str(out["after_insert"]), out["after_insert"]


def _summary(events: list[dict[str, Any]]) -> dict[str, Any]:
    kinds: dict[str, int] = {}
    for e in events:
        k = e["type"] + ("/" + e["inputType"] if e.get("inputType") else "") + ("" if e["trusted"] else "(untrusted)")
        kinds[k] = kinds.get(k, 0) + 1
    return kinds


HIDDEN_INPUT_EDITORS = ["monaco", "cm5", "ace"]


@pytest.mark.parametrize("editor", HIDDEN_INPUT_EDITORS)
def test_hidden_input_editor_workarounds(worker: Worker, site: LabServer, editor: str) -> None:
    """GAP: code editors that take keys through a hidden textarea under their drawn text (Monaco, CodeMirror 5, Ace;
    Google Docs' text-event iframe is the same shape). What a worker can still try: click the drawn text by a
    screenshot point (the editor focuses its textarea), then type or press on the textarea, or page_mode."""
    w = worker
    w.open(f"{site.a}/editors.html?e={editor}", wait_ready=25)
    w.snapshot()
    ta = _editor_target(w)
    out: dict[str, Any] = {"textarea": ta and (ta.role, ta.label, ta.in_view, ta.occluded, ta.rect),
                           "elements": [(e.role, e.label) for e in w.snap.elements]}
    x, y, _, _ = w.rect_of("#host")
    out["point_click"] = w.act_at(x, y, "click")
    w.snapshot()
    ta = _editor_target(w)
    out["type_after_click"] = w.act(ta.id, "type", text="print(1)\nprint(2)")
    out["truth_after_type"] = w.truth()["text"]
    out["press_after_click"] = w.act(ta.id, "press", text="a")
    out["page_mode_type"] = w.act(ta.id, "type", text="print(3)\nprint(4)", page_mode=True)
    out["truth_after_page_mode"] = w.truth()["text"]
    out["page_mode_press"] = w.act(ta.id, "press", text="Enter", page_mode=True)
    out["truth"] = w.truth()["text"]
    report(f"hidden:{editor}", w, out)
    # HAND-504: real keys reach the covered textarea (focused by script); page_mode still works too
    assert out["type_after_click"].get("ok") and out["press_after_click"].get("ok"), out
    assert "print(1)" in str(out["truth_after_type"]) and "print(2)" in str(out["truth_after_type"]), out


# ---------- canvas-drawn editors ----------

def test_canvas_doc_standin(worker: Worker, site: LabServer) -> None:
    """The Docs-shaped stand-in (canvas + 1px text-input iframe under it). Mirrors task 9e7c14ed's route."""
    w = worker
    w.open(f"{site.a}/canvas.html?k=doc", wait_ready=10)
    w.snapshot()
    out: dict[str, Any] = {"elements": [(e.id, e.role, e.label, e.in_view, e.occluded) for e in w.snap.elements]}
    doc = w.el("Document content")
    out["type_on_textbox"] = w.act(doc.id, "type", text="Hello") if doc else None
    out["press_on_textbox"] = w.act(doc.id, "press", text="Enter") if doc else None
    out["page_mode_type"] = w.act(doc.id, "type", text="Hello", page_mode=True) if doc else None
    x, y, _, _ = w.rect_of("#page")
    out["click_canvas"] = w.act_at(x - 300, y - 200, "click")
    w.snapshot()
    doc = w.el("Document content")
    out["type_after_canvas_click"] = w.act(doc.id, "type", text="Line one") if doc else None
    out["truth"] = w.truth()
    report("canvas:doc", w, out)
    # HAND-504: task 9e7c14ed's route now works: the 1-px text-event frame under the canvas takes the keys
    assert out["type_on_textbox"].get("ok") and out["press_on_textbox"].get("ok"), out
    assert out["type_after_canvas_click"].get("ok") and "Line one" in str(out["truth"]["text"]), out


def test_canvas_grid(worker: Worker, site: LabServer) -> None:
    """x-data-spreadsheet, a canvas grid: put "Rank" in A1 and 42 in B2, as a worker would."""
    w = worker
    w.open(f"{site.a}/canvas.html?k=grid", wait_ready=15)
    time.sleep(1)
    w.snapshot()
    out: dict[str, Any] = {"elements": [(e.id, e.role, e.label, e.in_view, e.occluded) for e in w.snap.elements][:40]}
    # the grid's first cell sits right of the 60px row header and under the 25px column header (library defaults)
    left, top = w.evaluate("() => { const r = document.querySelector('.x-spreadsheet-table').getBoundingClientRect();"
                           " return [r.x, r.y]; }")
    out["click_A1"] = w.act_at(left + 60 + 50, top + 25 + 12, "click")
    w.snapshot()
    out["elements_after_click"] = [(e.id, e.role, e.label, e.in_view, e.occluded) for e in w.snap.elements][:40]
    editors = [e for e in w.snap.elements if e.role == "textbox"]
    out["type_A1"] = w.act(editors[0].id, "type", text="Rank") if editors else None
    out["enter"] = w.act(editors[0].id, "press", text="Enter") if editors else None
    out["dblclick_B2"] = w.act_at(left + 60 + 100 + 50, top + 25 + 25 + 12, "dblclick")
    w.snapshot()
    editors = [e for e in w.snap.elements if e.role == "textbox"]
    out["editor_after_dblclick"] = [(e.id, e.label, e.in_view, e.occluded) for e in editors]
    out["type_B2"] = w.act(editors[0].id, "type", text="42") if editors else None
    out["enter_B2"] = w.act(editors[0].id, "press", text="Enter") if editors else None
    out["truth"] = w.truth()
    report("canvas:grid", w, out)
    assert out["type_A1"] is None                     # nothing to type into after a single click on the canvas grid
    assert out["truth"]["cells"].get("B2") == "42"     # works after a double-click opens the cell editor


# ---------- the browser's own UI ----------

def test_browser_ui(worker: Worker, site: LabServer, lab: Any) -> None:
    w = worker
    _, _, front = lab
    before = len(front.changes)
    w.open(f"{site.a}/chrome-ui.html", wait_ready=10)
    out: dict[str, Any] = {}
    out["popup"] = _try(w, "Sign in with Lab (popup)", "click")
    time.sleep(1)
    out["popup_state"] = w.truth()
    from ferb.act.browser.tools import (
        BrowserSnapshot,  # the tool itself: its result carries opened_tabs (news)
    )
    news = w.tool(BrowserSnapshot)
    out["popup_news"] = {k: news.get(k) for k in ("opened_tabs", "downloads") if k in news}
    out["task_tabs"] = [(a, t.url) for a, t in w.router._tabs.items() if t.owner == w.task and not t.closed]
    out["lab_pages"] = [t["url"] for t in w.chrome.targets() if t["type"] == "page"]
    out["notify"] = _try(w, "Allow notifications", "click")
    out["geo"] = _try(w, "Use my location", "click")
    time.sleep(2)
    out["permission_state"] = w.truth()
    view = w.snapshot()
    out["snapshot_mentions_prompt"] = {k: view[k] for k in view if k not in ("elements", "text", "tab", "url", "title")}
    out["pdf_click"] = _try(w, "Invoice PDF", "click")
    time.sleep(2)
    try:
        v = w.snapshot()
        out["pdf_snapshot"] = {"url": v.get("url"), "elements": len(w.snap.elements), "text": (w.snap.text or "")[:200]}
    except Exception as e:                                      # noqa: BLE001 - the failure is the finding
        out["pdf_snapshot"] = repr(e)[:300]
    try:
        out["pdf_find"] = w.find("4242")
    except Exception as e:                                      # noqa: BLE001
        out["pdf_find"] = repr(e)[:300]
    out["front_changes"] = front.changes[before:]
    report("browser-ui", w, out)
    # a window.open popup window (sign-in style, a named target and a size) becomes the task's (HAND-511; the lab's
    # task now counts as running, which it didn't before, so the earlier "never the task's" was the lab's own gap)
    assert "opened" in out["popup"] and len(out["task_tabs"]) == 2, (out["popup"], out["task_tabs"])
    assert any("name=popup" in u for u in out["lab_pages"])
    assert out["permission_state"].get("notify") == "asked"                    # GAP: a prompt waits; no fact says so
    assert isinstance(out["pdf_snapshot"], dict) and out["pdf_snapshot"]["elements"] == 0   # GAP: PDF viewer unreadable
    assert out["front_changes"] == []


def test_popups_and_tabs_that_close_themselves(worker: Worker, site: LabServer, lab: Any) -> None:
    """HAND-511: a sign-in window (window.open with a name and a size) that closes itself once done, and a download a
    page starts in a new tab (Drive's Download button: live 2026-11-12, task f68317e9, the browser closed that tab and
    FERB asked the owner "You closed a tab I was using"), and browser.open of an address that is a download."""
    from ferb.act.browser.tools import BrowserDownloads, BrowserOpen
    w = worker
    _, _, front = lab
    before = len(front.changes)
    w.open(f"{site.a}/chrome-ui.html", wait_ready=10)
    out: dict[str, Any] = {"signin": _try(w, "Sign in with Lab (closes itself)", "click")}
    time.sleep(1)
    tabs = lambda: [(a, t.url) for a, t in w.router._tabs.items() if t.owner == w.task and not t.closed]  # noqa: E731
    out["task_tabs_after_signin"] = tabs()
    popup = next((a for a, u in out["task_tabs_after_signin"] if "popup-close" in u), None)
    opener = w.tab
    if popup:
        w.tab = popup
        out["popup_continue"] = _try(w, "Continue as Lab user", "click")
        w.tab = opener
        time.sleep(1.5)
    out["signin_state"] = w.truth()
    out["owner_stopped_after_signin"] = list(w.published("browser.owner_stopped"))
    out["opener_snapshot_news"] = {k: v for k, v in w.tool(BrowserSnapshot).items()
                                   if k in ("opened_tabs", "closed_tabs", "downloads")}
    out["dltab"] = _try(w, "Download in a new tab", "click")
    time.sleep(3)
    out["downloads"] = w.tool(BrowserDownloads)
    out["owner_stopped_after_download_tab"] = list(w.published("browser.owner_stopped"))
    out["task_tabs_after_download_tab"] = tabs()
    tool = BrowserOpen(w.router, OWNER)
    res = w.run(tool.run(tool.Input(url=f"{site.a}/download?name=direct.pdf", profile="Lab"), w.ctx()))
    out["open_download"] = dict(res.content)
    time.sleep(1)
    out["owner_stopped_after_open"] = list(w.published("browser.owner_stopped"))
    out["task_tabs_after_open"] = tabs()
    out["front_changes"] = front.changes[before:]
    report("popups", w, out)
    assert popup and out["popup_continue"].get("ok") and out["signin_state"].get("signin") == "done", out
    # HAND-511: a window or tab that closed itself is a plain fact for the worker, never the owner stopping the task
    assert out["owner_stopped_after_open"] == [], out["owner_stopped_after_open"]
    assert [c["tab"] for c in out["opener_snapshot_news"].get("closed_tabs", [])] == [popup], out["opener_snapshot_news"]
    assert out["downloads"]["downloads"][0]["state"] == "taken"
    assert [a for a, _ in out["task_tabs_after_open"]] == [opener], out["task_tabs_after_open"]
    assert "closed" in out["open_download"] and out["open_download"]["downloads"][0]["state"] == "taken"
    assert out["front_changes"] == []


def test_basic_auth_and_print(worker: Worker, site: LabServer, lab: Any) -> None:
    """HTTP basic auth (the browser's own sign-in sheet) and window.print() (print preview), in a background tab."""
    w = worker
    _, _, front = lab
    before = len(front.changes)
    out: dict[str, Any] = {}
    t0 = time.monotonic()
    try:
        w.open(f"{site.a}/auth")
        out["auth_open"] = f"opened in {time.monotonic() - t0:.1f}s"
        v = w.snapshot()
        out["auth_snapshot"] = {"url": v.get("url"), "title": v.get("title"), "text": (w.snap.text or "")[:200],
                                "elements": [(e.role, e.label) for e in w.snap.elements]}
    except Exception as e:                                      # noqa: BLE001
        out["auth_open"] = f"{repr(e)[:300]} after {time.monotonic() - t0:.1f}s"
    w.close()
    w.open(f"{site.a}/chrome-ui.html", wait_ready=10)
    t0 = time.monotonic()
    try:
        out["print_click"] = _try(w, "Print this page", "click")
    except Exception as e:                                      # noqa: BLE001 - the failure is the finding
        out["print_click"] = f"{repr(e)[:300]} after {time.monotonic() - t0:.1f}s"
    time.sleep(2)
    try:
        out["print_state"] = w.truth()
        out["after_print_snapshot"] = len(w.snapshot().get("elements") or [])
    except Exception as e:                                      # noqa: BLE001
        out["after_print"] = repr(e)[:300]
    try:
        out["press_escape"] = _try(w, "Print this page", "press", text="Escape")
    except Exception as e:                                      # noqa: BLE001
        out["press_escape"] = repr(e)[:300]
    out["front_changes"] = front.changes[before:]
    report("auth-print", w, out)
    assert "error page" in out["auth_open"]                        # GAP: HTTP basic auth reads as an error page
    assert "OutcomeUnknown" in str(out["print_click"])             # GAP: window.print() hangs the act and the tab
    assert out["front_changes"] == []


# ---------- writing structured text: paste with html, lines into a covered input ----------

@pytest.mark.parametrize("editor", EDITORS)
def test_paste_with_html(worker: Worker, site: LabServer, editor: str) -> None:
    """HAND-506: browser.act paste, the page's own paste event with text/plain and text/html from the extension, never
    the system clipboard. The editors that handle paste keep the heading and the list; the browser's own editing and
    Slate (beforeinput) ignore a scripted paste, and the result says nothing was taken."""
    w = worker
    w.open(f"{site.a}/editors.html?e={editor}", wait_ready=25)
    html = "<h2>Lab heading</h2><p>First para with <b>bold</b>.</p><ul><li>one</li><li>two</li></ul><p>Last para.</p>"
    text = "Lab heading\nFirst para with bold.\none\ntwo\nLast para."
    w.snapshot()
    target = _editor_target(w)
    r = w.act(target.id, "paste", text=text, html=html)
    time.sleep(0.5)
    st = w.truth()
    report(f"paste:{editor}", w, {"result": r, "text": st and st["text"], "html": st and (st.get("html") or "")[:300]})
    if editor in ("plain", "slate"):
        assert "Lab heading" not in str(st["text"])
        assert r.get("ok") is False and r.get("reason") == "paste_not_taken", r
    else:
        assert r.get("ok"), r
        assert "Lab heading" in str(st["text"]) and "two" in str(st["text"]), st["text"]


@pytest.mark.parametrize("editor", HIDDEN_INPUT_EDITORS + ["doc"])
def test_lines_into_a_field_under_the_pages_own_view(worker: Worker, site: LabServer, editor: str) -> None:
    """HAND-505: FERB's own type into a covered input (code editors, the Docs-shaped stand-in): focused by script, each
    line break pressed as Enter, at the caret by default. (The lab's earlier candidate run through its own pipe showed
    the browser mechanism; this is FERB's code path.)"""
    w = worker
    url = f"{site.a}/canvas.html?k=doc" if editor == "doc" else f"{site.a}/editors.html?e={editor}"
    w.open(url, wait_ready=25)
    w.snapshot()
    target = w.el("Document content") if editor == "doc" else _editor_target(w)
    out: dict[str, Any] = {"type": w.act(target.id, "type", text="def f():\n    return 1")}
    time.sleep(0.3)
    out["text"] = w.truth()["text"]
    report(f"lines:{editor}", w, out)
    assert out["type"].get("ok") and out["type"].get("mode") == "insert", out["type"]
    lines = str(out["text"]).split("\n")
    assert lines[0].endswith("def f():") and any(x.strip() == "return 1" for x in lines[1:]), out["text"]


# ---------- real public pages, as an anonymous visitor (nothing typed is sent anywhere: client-side playgrounds) ----

PUBLIC_DOC = "https://docs.google.com/document/d/1_jnAifzWv0llivFWolSXxdcFJ4ksjG8eM3MdNjTqAcY/edit"


def test_public_google_doc_read(worker: Worker) -> None:
    """A public, view-only Google Doc (canvas-drawn), signed out: what FERB's snapshot, find and html read of it, and
    whether the export URL (the route workers use) gives the text."""
    w = worker
    w.open(PUBLIC_DOC)
    time.sleep(6)
    view = w.snapshot()
    words = [x for x in (w.snap.text or "").split() if len(x) > 3][:400]
    out: dict[str, Any] = {"title": view.get("title"), "elements": len(w.snap.elements),
                           "snapshot_text_chars": len(w.snap.text or ""), "snapshot_text_head": (w.snap.text or "")[:500],
                           "roles": sorted({e.role for e in w.snap.elements})}
    html = w.html(selector=".kix-page, .kix-canvas-tile-content, canvas, [role=document]")
    out["html_matches"] = {k: html.get(k) for k in ("matches", "total_chars")}
    out["canvas_count"] = w.evaluate("() => document.querySelectorAll('canvas').length")
    out["dom_text_chars"] = w.evaluate("() => document.body.innerText.length")
    out["annotated_text"] = w.evaluate("() => typeof window._docs_annotate_getAnnotatedText")
    out["words_sample"] = words[:30]
    out["annotated_call"] = w.evaluate("() => { try { const a = window._docs_annotate_getAnnotatedText('x');"
                                       " return a && typeof a.getText === 'function' ? a.getText().slice(0, 200) : String(a); }"
                                       " catch (e) { return 'error: ' + e.message; } }")
    out["find_body_word"] = w.find("Publish").get("matches")
    from ferb.act.browser.tools import BrowserOpen
    tool = BrowserOpen(w.router, OWNER)
    res = w.run(tool.run(tool.Input(url=PUBLIC_DOC.replace("/edit", "/export?format=txt"), profile="Lab"), w.ctx()))
    out["export_open"] = {k: v for k, v in res.content.items() if k in ("downloads", "url", "error")}
    report("public:gdoc", w, out)


PLAYGROUNDS = {
    "monaco": "https://microsoft.github.io/monaco-editor/playground.html",
    "codemirror": "https://codemirror.net/try/",
    "lexical": "https://playground.lexical.dev/",
    "prosemirror": "https://prosemirror.net/examples/basic/",
    "excalidraw": "https://excalidraw.com/",
}


@pytest.mark.parametrize("name", list(PLAYGROUNDS))
def test_public_playgrounds(worker: Worker, name: str) -> None:
    """The real playground pages: find the editing surface a worker would pick and type a two-line text."""
    w = worker
    w.open(PLAYGROUNDS[name])
    time.sleep(8)
    w.snapshot()
    boxes = [(e.role, e.label) for e in w.snap.elements if e.role == "textbox"]
    out: dict[str, Any] = {"textboxes": [(e.id, e.role, e.label[:60], e.frame_id, e.in_view, e.occluded)
                                         for e in w.snap.elements if e.role == "textbox"][:12],
                           "elements": len(w.snap.elements), "frames": len({e.frame_id for e in w.snap.elements})}
    for role, label in boxes[:2]:
        w.snapshot()
        e = w.el(label, role)
        if e is not None:
            out[f"type:{label[:30]}"] = w.act(e.id, "type", text="lab line one\nlab line two")
    if name == "excalidraw":
        # a canvas app's own keyboard: the text tool is the key "t", then a click on the canvas opens a textarea
        w.snapshot()
        out["roles"] = sorted({e.role for e in w.snap.elements})
        tool_btn = next((e for e in w.snap.elements if e.label.lower().startswith("text")), None)
        out["text_tool"] = tool_btn and w.act(tool_btn.id, "click")
        p = w.point_at(640, 500)
        out["canvas_point"] = p
        if "id" in p:
            out["canvas_click"] = w.act(p["id"], "click")
        w.snapshot()
        ta = next((e for e in w.snap.elements if e.role == "textbox"), None)
        out["textarea_after_click"] = ta and (ta.id, ta.label, ta.in_view, ta.occluded)
        if ta:
            out["type_text"] = w.act(ta.id, "type", text="lab line one\nlab line two")
            out["escape"] = w.act(ta.id, "press", text="Escape")
        out["scene_text"] = w.evaluate("() => { try { return JSON.parse(localStorage.getItem('excalidraw') || '[]')"
                                       ".filter(e => e.type === 'text').map(e => e.text); } catch (e) { return String(e); } }")
    w.snapshot()
    out["page_text_has_typed"] = "lab line" in (w.snap.text or "")
    out["find_typed"] = w.find("lab line two").get("matches")
    report(f"public:{name}", w, out)


def test_public_map_pan(worker: Worker) -> None:
    """Google Maps (a WebGL canvas) signed out: pan by dragging two screenshot points; does the view move?"""
    w = worker
    w.open("https://www.google.com/maps/@12.9716,77.5946,14z")
    time.sleep(8)
    w.snapshot()
    url0 = w.router.tab_url(w.tab)
    out: dict[str, Any] = {"elements": len(w.snap.elements), "url0": url0,
                           "consent": [e.label for e in w.snap.elements if "accept" in e.label.lower()][:3]}
    shot = w.screenshot()
    vw, vh = w.evaluate("() => [innerWidth, innerHeight]")
    sx, sy = shot["width"] / vw, shot["height"] / vh
    p1, p2 = w.point(shot["shot"], 900 * sx, 450 * sy), w.point(shot["shot"], 700 * sx, 350 * sy)
    out["points"] = (p1, p2)
    if "id" in p1 and "id" in p2:
        out["drag"] = w.act(p1["id"], "drag", end_element=p2["id"])
    time.sleep(2)
    out["url1"] = w.router.tab_url(w.tab)
    w.snapshot()
    out["scroll_zoom"] = w.scroll("down")
    time.sleep(2)
    out["url2"] = w.router.tab_url(w.tab)
    report("public:maps", w, out)


# ---------- form widgets ----------

def _try(w: Worker, label: str, action: str, role: str | None = None, contains: bool = False, **args: Any) -> Any:
    """Snapshot, find the element by label, act; None when the snapshot has no such element (itself a finding)."""
    w.snapshot()
    e = w.el(label, role, contains)
    if e is None:
        w.log.append({"tool": "lookup", "args": {"label": label, "role": role}, "s": 0,
                      "out": "NOT IN SNAPSHOT: " + str([(x.role, x.label) for x in w.snap.elements][:60])})
        return None
    return w.act(e.id, action, **args)


def test_widgets(worker: Worker, site: LabServer) -> None:
    w = worker
    w.open(f"{site.a}/widgets.html", wait_ready=15)
    time.sleep(1.5)                                          # the libraries build their widgets after load
    out: dict[str, Any] = {}
    _try(w, "Country", "select", text="Japan")
    _try(w, "Birth date", "type", text="2026-12-25")
    out["date_trusted"] = w.truth()["date"]
    _try(w, "Birth date", "type", text="2026-12-25", page_mode=True)
    out["date_page"] = w.truth()["date"]
    _try(w, "Meeting time", "type", text="14:30")
    out["time_trusted"] = w.truth()["time"]
    _try(w, "Local date-time", "type", text="2026-12-25T14:30", page_mode=True)
    _try(w, "Volume", "press", text="ArrowRight")
    out["range_after_arrow"] = w.truth()["range"]
    _try(w, "Volume", "type", text="70", page_mode=True)
    _try(w, "Colour", "type", text="#ff0000", page_mode=True)
    _try(w, "Quantity", "type", text="5")
    w.snapshot()
    out["all_elements"] = [(e.role, e.label, e.value) for e in w.snap.elements]
    # Choices.js: its original <select> is hidden; what shows is a role-less div that opens a listbox
    _point_act(w, ".choices", "click")
    w.snapshot()
    out["choices_open_elements"] = [(e.role, e.label) for e in w.snap.elements if "Mumbai" in e.label or "Bengaluru" in e.label]
    _try(w, "Mumbai", "click", contains=True)
    out["choices_after_click"] = w.truth()["choices"]
    tom_input = w.evaluate("() => document.querySelector('#tom-ts-control') ? 'present' : 'absent'")
    out["tom_input"] = tom_input
    _try(w, "Language", "type", text="Tam", contains=True)
    w.snapshot()
    out["tom_options_after_type"] = [(e.role, e.label) for e in w.snap.elements if e.role == "option"]
    _try(w, "Tamil", "click", role="option")
    out["tom"] = w.truth()["tom"]
    _try(w, "Check-in date", "type", text="2026-12-25")
    out["flat_typed"] = w.truth()["flat"]
    _try(w, "Check-in date", "click")
    w.snapshot()
    out["flat_days_in_snapshot"] = [(e.role, e.label) for e in w.snap.elements if "2026" in e.label][:5]
    _try(w, "November 20, 2026", "click", contains=True)
    out["flat_clicked"] = w.truth()["flat"]
    w.snapshot()
    handles = [e for e in w.snap.elements if e.role == "slider" and e.label != "Volume"]
    out["slider_handles"] = [(e.id, e.label, e.value) for e in handles]
    if handles:
        w.act(handles[0].id, "press", text="ArrowRight")
    out["slider"] = w.truth()["slider"]
    w.snapshot()
    stars = [e for e in w.snap.elements if e.label == "★"]
    out["stars_in_snapshot"] = [(e.id, e.role) for e in stars]
    if len(stars) >= 4:
        w.act(stars[3].id, "click")
    out["switch_type"] = _try(w, "Notifications", "type", text="on")   # HAND-509: a button isn't a field
    _try(w, "Notifications", "click")
    out["phone_type"] = _try(w, "Phone", "type", text="9876543210")
    out["phone"] = (w.truth()["phone"], w.truth()["phoneShown"])
    _try(w, "Gift card number", "type", text="4111111111111111")
    out["card"] = w.truth()["card"]
    _try(w, "Fruit", "press", text="ArrowDown")
    _try(w, "Assignee", "type", text="Ar")
    _try(w, "Arjun Mehta", "click")
    _try(w, "I agree to the terms", "click", contains=True)
    out["final"] = w.truth()
    report("widgets", w, out)
    f = out["final"]
    # HAND-509: a date or time field is set through its value (real keys set nothing); a button refuses typing; a mask
    # that reformats what was typed says what the field reads now
    assert out["date_trusted"] == "2026-12-25" and out["time_trusted"] == "14:30"
    assert out["switch_type"].get("ok") is False and out["switch_type"].get("reason") == "not_editable", out["switch_type"]
    assert out["phone_type"].get("field_value") == out["phone"][1] != "9876543210", out["phone_type"]
    assert f["date"] == "2026-12-25" and f["dtl"] == "2026-12-25T14:30"   # page_mode sets them
    assert all(v is None for _, _, v in out["slider_handles"])             # GAP: an ARIA slider's value isn't shown
    assert f["native"] == "Japan" and f["choices"] == "bom" and f["flat"] == "2026-11-20" and f["stars"] == 4
    assert f["switch"] == "true" and f["fruit"] == "Banana" and f["picked"] == "Arjun Mehta" and f["terms"] is True


def _point_act(w: Worker, selector: str, action: str, **args: Any) -> Any:
    """A worker's route to something the snapshot doesn't list: a screenshot, a point on it, then act on that id."""
    x, y, _, _ = w.rect_of(selector)
    p = w.point_at(x, y)
    return w.act(p["id"], action, **args) if "id" in p else p


def test_everyday_fields_read_as_secret(worker: Worker, site: LabServer) -> None:
    """GAP: a delivery pin code (India's postal code) and a voucher's "security code" are classified as login-code
    fields by their words, so FERB refuses to type into them (live: Flipkart, task 97e27b93)."""
    w = worker
    w.open(f"{site.a}/widgets.html", wait_ready=15)
    out: dict[str, Any] = {}
    for label, text in (("Delivery pin code", "560034"), ("Coupon security code (printed on the voucher)", "SAVE10")):
        w.snapshot()
        e = w.el(label)
        out[label] = {"secret": e and e.secret, "field_kind": e and e.field_kind}
        try:
            out[label]["type"] = w.act(e.id, "type", text=text) if e else None
        except Exception as ex:                                  # noqa: BLE001 - the refusal is the finding
            out[label]["type"] = repr(ex)[:300]
    out["truth"] = {k: w.truth()[k] for k in ("pincode", "voucher")}
    report("secret-words", w, out)
    assert out["Delivery pin code"]["field_kind"] == "one_time_code"      # GAP today; flips when fixed
    assert out["truth"]["pincode"] == ""


# ---------- drag and drop, hold ----------

def _drag(w: Worker, a: str, b: str) -> dict[str, Any]:
    """A worker's drag: one screenshot, two points (centres of the two CSS selectors), act drag."""
    ax, ay, vw, vh = w.rect_of(a)
    bx, by, _, _ = w.rect_of(b)
    shot = w.screenshot()
    sx, sy = shot["width"] / vw, shot["height"] / vh
    p1 = w.point(shot["shot"], ax * sx, ay * sy)
    p2 = w.point(shot["shot"], bx * sx, (by + 8) * sy)
    if "id" not in p1 or "id" not in p2:
        return {"points": [p1, p2]}
    return w.act(p1["id"], "drag", end_element=p2["id"])


def test_drag_and_hold(worker: Worker, site: LabServer) -> None:
    w = worker
    w.open(f"{site.a}/dnd.html", wait_ready=15)
    out: dict[str, Any] = {"before": w.truth()}
    w.snapshot()
    out["native_sortable"] = (_drag(w, "#native > div:nth-child(1)", "#native > div:nth-child(3)"), w.truth()["native"])
    out["pointer_sortable"] = (_drag(w, "#pointer > div:nth-child(1)", "#pointer > div:nth-child(3)"), w.truth()["pointer"])
    out["kanban_native"] = (_drag(w, "#c1", "#done"), w.truth()["done"])
    out["resize"] = (_drag(w, "#grip", "#map"), w.truth()["boxWidth"])
    out["pan"] = (_drag(w, "#map", "#hold"), w.truth()["pan"])
    _try(w, "Hold to confirm", "click")
    out["final"] = w.truth()
    report("drag", w, out)
    f = out["final"]
    assert f["native"] == ["N2", "N3", "N1", "N4"] and f["pointer"] == ["P2", "P3", "P1", "P4"]   # both DnD kinds work
    assert f["done"] == ["c1"] and f["boxWidth"] != 202 and f["pan"] != [0, 0]
    assert f["confirmed"] is False and f["holdMs"] < 200      # GAP: no press-and-hold


# ---------- scrolling ----------

def test_scrolling(worker: Worker, site: LabServer) -> None:
    """HAND-510: every scroll pane has an id (a feed of text, a carousel), sideways scrolling, screens at once, to the
    end. Before: the feed had no id, "right" scrolled the page down, and row 500 took about 58 one-screen calls."""
    w = worker
    w.open(f"{site.a}/scroll.html", wait_ready=10)
    view = w.snapshot()
    out: dict[str, Any] = {"scrollers": view.get("scrollers"), "find_500_first": w.find("Message 500")}
    scrollers = {s.get("label"): s.get("id") for s in view.get("scrollers") or []}
    out["scroller_labels"] = scrollers
    cid = next((v for k, v in scrollers.items() if k and "Messages" in k), None)
    calls, t0 = 0, time.monotonic()
    while cid and calls < 80:
        r = w.scroll("down", cid, screens=29)            # a worker's sum: row 500 is ~59 screens of 300 px down
        calls += 1
        if "Message 500" in w.truth()["virtualShown"] or "Message 500" in str(w.find("Message 500").get("text")):
            break
        if r.get("at_end"):
            break
    out["virtual_scrolls_to_500"] = {"calls": calls, "seconds": round(time.monotonic() - t0, 1),
                                     "shown": w.truth()["virtualShown"]}
    out["virtual_to_end"] = w.scroll("down", cid, to="end") if cid else None
    out["virtual_end_shown"] = w.truth()["virtualShown"]
    w.log.clear()
    fid = next((v for k, v in scrollers.items() if k and "Feed" in k), None)
    n, r, had = 0, {}, -1
    while fid and n < 40 and w.truth()["feedLoaded"] > had:     # to the end until the feed stops loading more
        had = w.truth()["feedLoaded"]
        r = w.scroll("down", fid, to="end")
        n += 1
        time.sleep(0.6)
    out["feed"] = {"calls": n, "loaded": w.truth()["feedLoaded"], "last": r}
    car = next((v for k, v in scrollers.items() if k and "Carousel" in k), None)
    out["carousel_scroll_right"] = w.scroll("right", car) if car else None
    out["carousel_left"] = w.truth()["carouselLeft"]
    out["carousel_back"] = w.scroll("left", car, to="start") if car else None
    out["carousel_left_after_start"] = w.truth()["carouselLeft"]
    out["deep_button"] = _try(w, "Deep button", "click")
    out["bottom"] = _try(w, "Bottom button", "click")
    out["final"] = w.truth()
    report("scroll", w, out)
    assert "Feed" in out["scroller_labels"] and "Carousel" in out["scroller_labels"], out["scroller_labels"]
    assert out["carousel_left"] > 0 and out["carousel_scroll_right"]["x"] > 0, out["carousel_scroll_right"]
    assert out["carousel_left_after_start"] == 0
    assert out["virtual_scrolls_to_500"]["calls"] <= 4, out["virtual_scrolls_to_500"]
    assert "Message 10000" in str(out["virtual_end_shown"]) or out["virtual_to_end"]["at_end"], out["virtual_to_end"]
    assert out["feed"]["loaded"] >= 200, out["feed"]


# ---------- pointer-only affordances ----------

def test_hover_context_dblclick_modifiers(worker: Worker, site: LabServer) -> None:
    w = worker
    w.open(f"{site.a}/menus.html", wait_ready=10)
    out: dict[str, Any] = {}
    w.snapshot()
    out["laptops_before_hover"] = bool(w.el("Laptops"))
    _try(w, "Products", "hover", contains=True)
    w.snapshot()
    out["laptops_after_hover"] = bool(w.el("Laptops"))
    _try(w, "Laptops", "click")
    out["css_menu"] = w.truth()["picked"]
    w.snapshot()
    out["account_in_snapshot"] = bool(w.el("Account"))       # a plain span with a mouseenter menu
    _point_act(w, "#account", "hover")
    _try(w, "Sign out", "click")
    out["js_menu"] = w.truth()["picked"]
    _point_act(w, "#area", "rightclick")                     # a plain <p>: not in the snapshot, so a point
    _try(w, "Rename", "click", role="menuitem")
    out["context_menu"] = w.truth()["ctx"]
    _point_act(w, "#cell", "dblclick")
    _try(w, "Cell value", "type", text="Edited")
    _try(w, "Cell value", "press", text="Enter")
    out["cell"] = w.truth()["cell"]
    w.snapshot()
    out["archive_buttons_before_hover"] = [(e.id, e.label) for e in w.elements("button") if e.label == "Archive"]
    _point_act(w, "#row2", "hover")
    w.snapshot()
    arch = [e for e in w.elements("button") if e.label == "Archive"]
    out["archive_buttons_after_hover"] = [(e.id, e.in_view) for e in arch]
    if arch:
        w.act(arch[-1].id, "click")
    out["row_action"] = w.truth()["rowAction"]
    # HAND-508: a hover that brings up something under the pointer (Gmail's row actions over the row) did what it was for
    out["hover_reveals"] = _point_act(w, "#row3", "hover")
    _try(w, "Archive 1003", "click")
    out["row_action_over"] = w.truth()["rowAction"]
    _try(w, "b.txt", "click", role="option")
    _try(w, "d.txt", "click", role="option")       # a person shift-clicks here: FERB's act has no modifier
    out["selected_after_two_clicks"] = w.truth()["selected"]
    w.snapshot()
    out["tooltip_in_snapshot_text"] = "Locker 12" in (w.snap.text or "")
    out["tooltip_find"] = w.find("Locker 12").get("matches")
    out["tooltip_html"] = "Locker 12" in str(w.html(selector="#tip").get("html"))
    report("pointer", w, out)
    assert out["css_menu"] == "Laptops" and out["js_menu"] == "Sign out" and out["context_menu"] == "Rename"
    assert out["cell"] == "Edited" and out["row_action"] == "archive 1002"
    assert out["hover_reveals"].get("ok") and "under the pointer" in str(out["hover_reveals"].get("observed")), out
    assert out["row_action_over"] == "archive 1003"
    assert out["selected_after_two_clicks"] == ["d.txt"]      # GAP: no shift-click, so no range selection


# ---------- shadow DOM and web components ----------

def test_shadow_dom(worker: Worker, site: LabServer) -> None:
    w = worker
    w.open(f"{site.a}/shadow.html", wait_ready=20)
    time.sleep(1)
    out: dict[str, Any] = {}
    for mode in ("open", "closed"):
        _try(w, f"{mode} email", "type", text=f"{mode}@example.com")
        _try(w, f"{mode} save", "click")
    _try(w, "Slotted action", "click")
    _try(w, "Deep leaf", "click")
    _try(w, "Full name", "type", text="Asha Rao")
    _try(w, "Plan", "select", text="Pro")
    w.snapshot()
    out["plan_elements"] = [(e.id, e.role, e.label) for e in w.snap.elements
                            if "Plan" in e.label or e.label in ("Pro", "Team")]
    _try(w, "Plan", "click", role="combobox")
    _try(w, "Pro", "click", role="option")
    _try(w, "Subscribe", "click")
    out["final"] = w.truth()
    report("shadow", w, out)
    f = out["final"]
    assert f["open"]["saved"] == "open@example.com" and f["closed"]["saved"] == "closed@example.com"
    assert f["slotted"] and f["leaf"] and f["slin"] == "Asha Rao" and f["slsel"] == "pro"


# ---------- frames ----------

def test_frames(worker: Worker, site: LabServer) -> None:
    w = worker
    w.open(f"{site.a}/frames.html?b={site.b}", wait_ready=10)
    time.sleep(1)
    view = w.snapshot()
    out: dict[str, Any] = {"labels": [(e.frame_id, e.role, e.label, e.in_view) for e in w.snap.elements][:60],
                           "frames_note": {k: view.get(k) for k in view if "frame" in k}}
    for name in ("same", "cross", "sandboxed", "scrolled", "nested", "nested-inner"):
        _try(w, f"{name} field", "type", text=f"{name}-value")
        _try(w, f"{name} pay", "click")
    _try(w, "Srcdoc button", "click")
    _try(w, "Framed editor", "type", text="Hello from a frame\nsecond line")
    time.sleep(0.5)                                           # the frames' postMessage reports arrive afterwards
    out["final"] = w.truth()
    report("frames", w, out)
    f = out["final"]
    for name in ("same", "cross", "sandboxed", "scrolled", "nested", "nested-inner"):
        assert f.get(name) == {"value": f"{name}-value", "trusted": True}, (name, f.get(name))
    assert f.get("srcdoc") is True
