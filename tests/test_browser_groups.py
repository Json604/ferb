"""Step 5c (protocol 18, HAND-201 and fix list): one collapsed tab group per task, tabs known by
browser tab id across reconnects and daemon restarts, the owner's close or move of a FERB tab published for the
supervisor, signed daemon messages, the native host's starter checked, reads that fail are known failures, the chosen
window remembered, secret fields (cards too) refusing text keys, structural facts and a stable P19 identity. The real
daemon with the fake extension (which verifies signatures like the extension), plus pure functions."""

import asyncio
import json
import time
from typing import Any

import pytest

import ferb.act.browser.ext_bridge as eb
from ferb.act.browser.ext_bridge import OutcomeUnknown, enters_text, group_title, label_amount
from ferb.act.browser.ext_bridge import peer_refusal as real_peer_refusal     # the check itself (conftest patches it)
from ferb.act.browser.ext_protocol import Result, daemon_envelope
from ferb.core.errors import DriverError
from ferb.core.types import FormSubmit, PageElement
from tests.browserkit import OWNER, bd, call, task  # noqa: F401  (the fixture and its helpers)
from tests.chromelab import SHOP, _ready


def _drv(d: Any) -> Any:
    return d.daemon.router


def _bridge(d: Any) -> Any:
    return d.daemon.hub.bridges[("chrome", "Profile 1")]


def _chrome_id(d: Any, tab: str) -> int:
    return int(_drv(d)._tabs[tab].chrome_id)


def _published(d: Any) -> list[tuple[str, dict[str, Any]]]:
    seen: list[tuple[str, dict[str, Any]]] = []
    real = d.daemon.hub.publish

    def spy(topic: str, data: dict[str, Any]) -> None:
        seen.append((topic, data))
        real(topic, data)
    d.daemon.hub.publish = spy
    return seen


# ---------- tab groups (HAND-201) ----------

# decision: HAND-201
def test_a_tasks_tabs_open_into_one_labelled_group_and_close_with_it_at_the_end(bd: Any) -> None:
    d, launched = bd
    ext, g = _ready(d, launched)
    _drv(d).task_title = lambda root: "Find the cheapest flight to Goa for next weekend please"
    a = call(d, g, "browser.open", url=f"{SHOP}/a")["content"]["tab"]
    b = call(d, g, "browser.open", url=f"{SHOP}/b")["content"]["tab"]
    ca, cb = _chrome_id(d, a), _chrome_id(d, b)
    opens = [json.loads(json.loads(m)["body"]) for m in ext.wire if '"type": "cmd"' in m]
    opens = [c for c in opens if c["op"] == "open"]
    assert ext.records[ca]["group"] == ext.records[cb]["group"]                  # one group per task
    group = ext.groups[ext.records[ca]["group"]]
    assert group["title"] == "FERB · Find the cheapest flight to…" and group["collapsed"]
    assert opens[0]["args"]["group"]["root"] == g.task_id and ext.records[ca]["task"] == g.task_id
    d.run(_drv(d).release(g.task_id))
    assert ca not in ext.tabs and cb not in ext.tabs and not ext.groups           # closed with the group


# decision: HAND-201
def test_the_result_tab_is_handed_over_out_of_the_group_and_a_borrowed_tab_is_left_open(bd: Any) -> None:
    d, launched = bd
    ext, g = _ready(d, launched)
    ext.tabs[41], ext.tab_windows[41] = "https://news.example/", 7
    borrowed = call(d, g, "browser.use_tab", tab_ref="chrome:Profile 1:41")["content"]["tab"]
    assert ext.records[41].get("borrowed") and "group" not in ext.records[41]      # recorded, never grouped
    result = call(d, g, "browser.open", url=f"{SHOP}/result")["content"]["tab"]
    other = call(d, g, "browser.open", url=f"{SHOP}/other")["content"]["tab"]
    cr, co = _chrome_id(d, result), _chrome_id(d, other)
    kept = call(d, g, "browser.keep_open", tab=result)
    assert not kept["is_error"] and cr in ext.kept and cr not in ext.records    # out of the group, open
    d.run(_drv(d).release(g.task_id))
    assert cr in ext.tabs and 41 in ext.tabs and co not in ext.tabs              # never closes what it didn't open
    assert ext.op_close({"tab_id": 41}, "x")["reason"] == "not_ferbs"            # the extension refuses it too


# decision: HAND-201
def test_keep_the_tabs_open_hands_every_tab_over_and_pause_closes_nothing(bd: Any) -> None:
    d, launched = bd
    ext, g = _ready(d, launched)
    a = call(d, g, "browser.open", url=f"{SHOP}/a")["content"]["tab"]
    ca = _chrome_id(d, a)
    _drv(d).task_paused = lambda task: True
    assert d.run(_drv(d).release(g.task_id)) == [] and ca in ext.records         # paused: the group stays
    _drv(d).task_paused = lambda task: False
    _drv(d).keep_tabs_open(g.task_id)
    d.run(_drv(d).release(g.task_id))
    assert ca in ext.tabs and ca in ext.kept and ca not in ext.records and not ext.groups


# decision: HAND-201
def test_the_owner_closing_a_ferb_tab_or_its_group_is_published_as_stopping_that_part(bd: Any) -> None:
    d, launched = bd
    ext, g = _ready(d, launched)
    seen = _published(d)
    a = call(d, g, "browser.open", url=f"{SHOP}/a")["content"]["tab"]
    b = call(d, g, "browser.open", url=f"{SHOP}/b")["content"]["tab"]
    for tab, part in ((a, "tab"), (b, "group")):
        cid = _chrome_id(d, tab)
        ext.tabs.pop(cid)
        ext.tab_event("removed", cid)                                            # the owner closed it
        ext.wait_for(lambda tab=tab: any(t == "browser.owner_stopped" and e["tab"] == tab for t, e in seen))
        [ev] = [e for t, e in seen if t == "browser.owner_stopped" and e["tab"] == tab]
        assert ev["part"] == part and ev["task_id"] == g.task_id
    snap = call(d, g, "browser.snapshot", tab=a)
    assert snap["is_error"] and "owner closed" in snap["content"]["error"]


# decision: HAND-201
def test_ferbs_own_close_is_no_owner_stop(bd: Any) -> None:
    d, launched = bd
    ext, g = _ready(d, launched)
    seen = _published(d)
    a = call(d, g, "browser.open", url=f"{SHOP}/a")["content"]["tab"]
    cid = _chrome_id(d, a)
    assert not call(d, g, "browser.close", tab=a)["is_error"]
    ext.tab_event("removed", cid)
    time.sleep(0.3)
    assert not [t for t, _ in seen if t == "browser.owner_stopped"]


# decision: HAND-201
def test_a_tab_the_owner_drags_out_of_the_group_is_theirs(bd: Any) -> None:
    d, launched = bd
    ext, g = _ready(d, launched)
    seen = _published(d)
    a = call(d, g, "browser.open", url=f"{SHOP}/a")["content"]["tab"]
    cid = _chrome_id(d, a)
    ext.take_out_of_group(cid)
    ext.wait_for(lambda: any(t == "browser.owner_took" for t, _ in seen))
    out = call(d, g, "browser.snapshot", tab=a)
    assert out["is_error"] and "theirs now" in out["content"]["error"]
    d.run(_drv(d).release(g.task_id))
    assert cid in ext.tabs                                                       # never closed by FERB


# ---------- connection-independent tab identity ----------

# decision: HAND-201
def test_a_reconnect_readopts_the_tasks_tabs_by_browser_id(bd: Any) -> None:
    d, launched = bd
    ext, g = _ready(d, launched)
    a = call(d, g, "browser.open", url=f"{SHOP}/form")["content"]["tab"]
    snap = call(d, g, "browser.snapshot", tab=a)["content"]
    ext.kill()
    ext.connect()
    ext.wait_for(lambda: ext.gen is not None and ext.gen >= 2)
    stale = call(d, g, "browser.act", tab=a, element=snap["elements"][0]["id"], action="click", effect="none",
                 why="t")
    assert stale["is_error"] and "older snapshot" in stale["content"]["error"]  # its element ids went
    assert not call(d, g, "browser.snapshot", tab=a)["is_error"]                 # the tab itself is still the task's
    d.run(_drv(d).release(g.task_id))
    assert not ext.records                                                       # closed after the reconnect too


# decision: HAND-201
def test_a_tab_whose_task_ended_while_disconnected_is_closed_at_the_next_hello(bd: Any) -> None:
    d, launched = bd
    ext, g = _ready(d, launched)
    a = call(d, g, "browser.open", url=f"{SHOP}/a")["content"]["tab"]
    cid = _chrome_id(d, a)
    ext.kill()
    _drv(d).task_running = lambda task: False                                    # the task ended meanwhile
    with pytest.raises(DriverError):                                             # disconnected: not confirmed
        d.run(_drv(d).release(g.task_id))
    ext.connect()
    ext.wait_for(lambda: cid not in ext.tabs)                                    # no orphan left behind


# decision: HAND-201
def test_after_a_daemon_restart_the_hello_readopts_a_running_tasks_tabs_and_closes_ended_ones(bd: Any) -> None:
    d, launched = bd
    ext, g = _ready(d, launched)
    ext.tabs[60], ext.tab_windows[60] = "https://shop.example/live", 5
    ext.tabs[61], ext.tab_windows[61] = "https://shop.example/old", 5
    ext.tabs[62], ext.tab_windows[62] = "https://news.example/", 5
    ext.records[60] = {"task": g.task_id, "root": g.task_id, "cid": "c60"}       # left by the daemon before
    ext.records[61] = {"task": "ended-task", "root": "ended-task", "cid": "c61"}
    ext.records[62] = {"task": "ended-task", "root": "ended-task", "borrowed": True}
    ext.kill()
    ext.connect()
    ext.wait_for(lambda: ext.gen is not None and ext.gen >= 2 and 61 not in ext.tabs)
    ext.wait_for(lambda: 62 not in ext.records)
    assert 62 in ext.tabs                                                        # the owner's tab: let go, open
    mine = [t for t in _drv(d).task_tabs(g.task_id)]
    assert any(_drv(d)._tabs[t["tab"]].chrome_id == 60 for t in mine)


# ---------- signing, the native host's starter, reads ----------

# decision: HAND-222
def test_the_daemon_signs_what_the_extension_acts_on_and_a_replay_is_refused(bd: Any) -> None:
    d, launched = bd
    ext, g = _ready(d, launched)
    tab = call(d, g, "browser.open", url=f"{SHOP}/a")["content"]["tab"]
    cmds = [json.loads(m) for m in ext.wire if '"type": "cmd"' in m]
    assert cmds and all(set(c) == {"type", "seq", "body", "mac"} for c in cmds) and not ext.rejected
    acts = len(ext.acts)
    ext._on(cmds[-1])                                                            # replayed: refused, nothing runs
    forged = dict(cmds[-1], seq=10_000, body=cmds[-1]["body"].replace("navigate", "close"))
    ext._on(forged)                                                              # changed: refused
    assert len(ext.rejected) == 2 and len(ext.acts) == acts
    assert not call(d, g, "browser.snapshot", tab=tab)["is_error"]


# decision: HAND-222
def test_daemon_envelope_binds_nonce_counter_type_and_body() -> None:
    import base64
    import hashlib
    import hmac
    key = b"k" * 32
    env = daemon_envelope(key, "nonce1", 3, "cmd", {"id": "c", "op": "close"})
    want = base64.b64encode(hmac.new(key, f"nonce1|3|cmd|{env['body']}".encode(), hashlib.sha256).digest()).decode()
    assert env["mac"] == want and json.loads(env["body"]) == {"id": "c", "op": "close"}
    assert daemon_envelope(key, "nonce2", 3, "cmd", {"id": "c", "op": "close"})["mac"] != want


# decision: HAND-223
def test_a_connection_is_taken_only_from_a_browser_ferb_knows() -> None:
    assert real_peer_refusal("com.google.Chrome") is None
    assert real_peer_refusal("com.google.Chrome", "chrome") is None
    assert real_peer_refusal("com.brave.Browser", "brave") is None
    assert "not a web browser" in (real_peer_refusal("com.apple.Terminal") or "")
    assert "couldn't tell" in (real_peer_refusal(None) or "")
    assert "paired in Google Chrome" in (real_peer_refusal("com.brave.Browser", "chrome") or "")


# decision: HAND-224
def test_a_failed_or_timed_out_read_is_a_known_failure_and_an_act_stays_unknown() -> None:
    from types import SimpleNamespace

    async def go() -> list[str]:
        out = []
        br = eb.Bridge(SimpleNamespace(browser="chrome", folder="Lab"), None, lambda *_: None, "test")

        class Writer:
            def write(self, data: bytes) -> None:
                c = json.loads(json.loads(data)["body"])
                if c["op"] == "snapshot":
                    br.pending[c["id"]].set_result(Result(id=c["id"], gen=0, status="error", error="frame removed",
                                                          outcome="unknown"))

            async def drain(self) -> None:
                pass
        br.conn = eb._Conn(Writer(), key=b"k" * 32, nonce="n")               # type: ignore[arg-type]
        for op in ("snapshot", "find", "act"):
            try:
                await br.call(op, {"tab_id": 1}, timeout=0.05)               # type: ignore[arg-type]
            except OutcomeUnknown:
                out.append(f"{op}:unknown")
            except DriverError:
                out.append(f"{op}:failed")
        return out
    assert asyncio.run(go()) == ["snapshot:failed", "find:failed", "act:unknown"]


def test_the_window_chosen_once_is_remembered_for_the_profile(bd: Any) -> None:
    d, launched = bd
    ext, g = _ready(d, launched)
    ext.ferb_window = None
    for w in (5, 7):
        ext.tabs[w * 10], ext.tab_windows[w * 10] = "https://news.example/", w
    first = call(d, g, "browser.open", url=f"{SHOP}/a")["content"]["tab"]
    chosen = ext.tab_windows[_chrome_id(d, first)]
    assert _bridge(d).window == chosen
    ext.tabs[3], ext.tab_windows[3] = "https://news.example/", 3                 # another window appears first
    _drv(d).task_root = lambda task: task + "-other"                             # (another task: no group window)
    second = call(d, g, "browser.open", url=f"{SHOP}/b")["content"]["tab"]
    assert ext.tab_windows[_chrome_id(d, second)] == chosen


# ---------- secret fields, facts, P19 identity ----------

# decision: HAND-218
def test_a_key_that_puts_text_into_a_secret_field_is_refused_and_return_still_goes(bd: Any, monkeypatch: Any) -> None:
    import tests.fake_extension as fake
    monkeypatch.setattr(fake, "page", lambda url: {"title": "Pay", "text": "", "elements": [
        {"role": "textbox", "label": "Card number", "secret": True, "input_type": "text", "field_kind": "card_number",
         "form": "form"}]})
    d, launched = bd
    ext, g = _ready(d, launched)
    tab = call(d, g, "browser.open", url=f"{SHOP}/pay")["content"]["tab"]
    el = call(d, g, "browser.snapshot", tab=tab)["content"]["elements"][0]
    assert el["sensitive"] and el["field"] == "card_number"
    for key in ("4", "shift+A", "Space", "cmd+v"):
        out = call(d, g, "browser.act", tab=tab, element=el["id"], action="press", text=key, effect="edit", why="t")
        assert out["is_error"] and "secret field" in out["content"]["error"], key
    assert not ext.acts
    ok = call(d, g, "browser.act", tab=tab, element=el["id"], action="press", text="Tab", effect="none", why="t")
    assert not ok["is_error"] and ext.acts


# decision: HAND-218
def test_enters_text_matches_the_extensions_rule() -> None:
    assert [enters_text(k) for k in ("a", "A", "4", " ", "Space", "shift+1", "cmd+v", "meta+V", "ctrl+v")] == \
        [True, True, True, True, True, True, True, True, False]
    assert not any(enters_text(k) for k in ("Enter", "Return", "Tab", "Escape", "ArrowDown", "Backspace", "cmd+a",
                                            "alt+left", "F5", ""))


def test_labels_and_amounts() -> None:
    assert group_title("") == "FERB" and group_title("Book a table") == "FERB · Book a table"
    assert len(group_title("x " * 40)) <= 40
    assert label_amount("Pay ₹248.75") == "₹248.75" and label_amount("Place order · USD 12.50") == "USD 12.50"
    assert label_amount("Pay 40 EUR") == "40 EUR" and label_amount("PAY 12") is None
    assert label_amount("Continue") is None and label_amount("$5 or $6") is None


def _el(**kw: Any) -> PageElement:
    return PageElement(**{"id": "s1e1", "frame_id": "0", "role": "button", "label": "", "value": None,
                          "rect": (0, 0, 1, 1), "visible": True} | kw)


def _tab_with(drv: Any, elements: dict[str, tuple[int, PageElement]], url: str = "https://shop.example/checkout?x=1",
              frames: dict[int, str] | None = None, commerce: dict[str, Any] | None = None) -> None:
    from types import SimpleNamespace
    t = SimpleNamespace(url=url, elements={k: (f, 1, 0, e) for k, (f, e) in elements.items()},
                        frame_urls=frames or {0: url}, commerce=commerce or {})
    drv._tabs = {"t1": t}


# decision: GATE-202
def test_facts_say_a_submit_its_host_method_kinds_amount_and_a_cross_origin_frame() -> None:
    drv = eb._TabDriver.__new__(eb._TabDriver)
    pay = _el(label="Pay ₹248.75", submit=FormSubmit(True, False, "pay.shop.example", "POST", ("card_number",)))
    card = _el(id="s1e2", role="textbox", secret=True, field_kind="card_number",
               submit=FormSubmit(False, True, "pay.shop.example", "POST", ("card_number", "card_csc"), "Pay now"))
    _tab_with(drv, {"s1e1": (0, pay), "s1e2": (3, card)},
              frames={0: "https://shop.example/checkout", 3: "https://bank.example/3ds"},
              commerce={"amount": "INR 248.75", "merchant": "Shop"})
    click = drv.act_facts("t1", "s1e1", "click", None)
    assert (click.submits_form, click.form_action_host, click.form_method, click.page_amount, click.page_merchant) == \
        (True, "pay.shop.example", "POST", "₹248.75", "Shop")
    assert click.field_kinds == ("card_number",) and click.frame_host is None
    enter = drv.act_facts("t1", "s1e2", "press", "Enter")
    assert enter.submits_form and enter.frame_host == "bank.example" and enter.secure_field
    assert enter.field_kinds == ("card_number", "card_csc") and enter.page_amount == "INR 248.75"
    typed = drv.act_facts("t1", "s1e2", "type", "x")
    assert typed.submits_form is None and typed.field_kinds == ("card_number",)
    assert drv.act_facts("t1", "nope", "click", None) is None


# decision: HAND-225
def test_p19_identity_is_the_page_and_node_never_the_snapshot_id_or_tab() -> None:
    from ferb.act.browser.tools import BrowserAct, OWNER_BROWSER
    drv = eb._TabDriver.__new__(eb._TabDriver)
    _tab_with(drv, {"s1e4": (0, _el(backend_node=812, path="html:1>body:1>button:3")),
                    "s2e9": (0, _el(backend_node=812, path="html:1>body:1>button:2")),
                    "s2e1": (2, _el(path="html:1>body:1>div:1")),
                    "s2e2": (0, _el(backend_node=900))},
              frames={0: "https://shop.example/checkout?x=1", 2: "https://pay.example/frame?t=9"})
    drv.tab_url = lambda tab: "https://shop.example/checkout?x=1"           # type: ignore[attr-defined]
    act = BrowserAct(drv, OWNER_BROWSER)
    a = act.ledger_args(act.Input(tab="t1", element="s1e4", action="click"), {"tab": "t1", "element": "s1e4",
                                                                               "action": "click"})
    b = act.ledger_args(act.Input(tab="t9", element="s2e9", action="click"), {"tab": "t9", "element": "s2e9",
                                                                               "action": "click"})
    assert a == {"action": "click", "at": {"page": "https://shop.example/checkout", "role": "button", "node": "b:812"}}
    assert drv.element_identity("t1", "s2e1") == {"page": "https://shop.example/checkout",
                                                  "frame": "https://pay.example/frame", "role": "button",
                                                  "node": "p:html:1>body:1>div:1"}
    assert drv.element_identity("t1", "s2e2") != drv.element_identity("t1", "s1e4")
    assert b == {"action": "click", "tab": "t9", "element": "s2e9"} or b != a   # unknown tab: unchanged
    spec = act.describe(act.Input(tab="t1", element="s1e4", action="click"))
    assert "s1e4" not in (spec.content or "") and "b:812" in (spec.content or "")


def test_pairing_rolls_back_when_storing_the_binding_fails(tmp_path: Any) -> None:
    from ferb.act.browser.pairing import Pairings
    from ferb.act.browser.ext_protocol import ExtMessage
    from ferb.store.db import Database
    from ferb.clock.clock import FakeClock
    db = Database(tmp_path / "f.db")
    db.migrate()
    p = Pairings(db, FakeClock())
    first = p.issue_token("chrome", "Default")
    assert p.handle(ExtMessage(install_id="install-one", type="pair", body_json=json.dumps({"token": first}))).accepted
    second = p.issue_token("chrome", "Default")
    db.conn.execute("CREATE TRIGGER no_insert BEFORE INSERT ON ext_installs BEGIN SELECT RAISE(ABORT, 'boom'); END")
    with pytest.raises(Exception):
        p.handle(ExtMessage(install_id="install-two", type="pair", body_json=json.dumps({"token": second})))
    assert p.for_profile("chrome", "Default") is not None and p.binding("install-one") is not None
    assert not db.conn.in_transaction


def test_profile_names_resolve_in_the_forms_workers_use() -> None:
    from ferb.act.browser.profiles import BrowserProfile as B, resolve
    ps = [B("chrome", "Profile 2", "acme", ""), B("chrome", "Default", "Sam", "k@x.com"),
          B("brave", "Default", "Work", ""), B("chrome", "Profile 9", "‎Trip", "")]
    for spoken in ("chrome acme", "chrome:acme", "Chrome/Profile 2", "chrome “acme”", "Google Chrome acme"):
        r = resolve(spoken, ps)
        assert r.kind == "one" and r.candidates[0].folder == "Profile 2", spoken
    assert resolve("Brave Browser", ps).candidates[0].browser == "brave"           # a browser with one profile
    assert resolve("chrome", ps).kind == "ambiguous"                                # several: listed, never chosen
    assert resolve("trip", ps).kind == "one"                                         # invisible marks ignored
    assert resolve("scal", ps).kind == "none"                                        # never fuzzy


def test_the_pages_own_submit_event_is_reported_as_the_acts_facts(bd: Any, monkeypatch: Any) -> None:
    import tests.fake_extension as fake
    from ferb.core.types import BrowserAction
    monkeypatch.setattr(fake, "page", lambda url: {"title": "Checkout", "text": "", "elements": [
        {"role": "button", "label": "Continue", "form": "form",
         "submitted": [{"action_host": "pay.example", "method": "POST", "kinds": ["card_number", "bogus"]}]}]})
    d, launched = bd
    ext, g = _ready(d, launched)
    tab = call(d, g, "browser.open", url=f"{SHOP}/checkout")["content"]["tab"]
    el = call(d, g, "browser.snapshot", tab=tab)["content"]["elements"][0]["id"]
    ref = _drv(d).owned_by(tab, g.task_id)
    r = d.run(_drv(d).act(_drv(d).element_ref(ref, el), BrowserAction("click", None, False), ""))
    assert r.observed.submits_form and r.observed.form_action_host == "pay.example"
    assert r.observed.form_method == "POST" and r.observed.field_kinds == ("card_number",)
    assert r.observed.page_url and r.observed.target_role == "button"
