"""Fixtures for the Rust port of the browser hands (rust/crates/ferb-browser), made by the Python build and by the
unchanged extension's own JavaScript (ENG-3).

    cd <repo> && uv run python rust/fixtures/browser/make.py

Writes, next to this file, from invented data only (temporary folders, never ~/.ferb, never a browser):

- protocol.json: ext_protocol.py's constants, version rules, message validation (pydantic's lax rules on the
  extension's messages, models dumped), both HMACs, the signed daemon envelope, and the small text rules
  (enters_text, group_title, the refusal words, the read-timeout error).
- crypto.json: HMAC and seal vectors from Python AND from protocol.js run under node (js_vectors.js), so the Rust
  code is checked against both ends: each side's MAC of the same input, each side's seal under a fixed IV, and each
  side opening the other's seal.
- pairing.json: a scripted run of pairing.Pairings on a temporary store (fixed clock, counter-made random bytes):
  each call's verdict and the ext_installs rows after it, column types included, so the Rust store writes the bytes
  Python writes and existing pairings survive.
- transcripts.json: the real ext_bridge.Hub and Bridge driven over an in-memory connection by the Python tests'
  scripted fake extension (tests/fake_extension.py, imported; its native-host transport replaced by direct calls):
  every frame both ways, in order, and every call's result or error, per scenario. The Rust test replays the
  extension's frames and must produce the same daemon frames and results.

Randomness is replaced by a counter (secrets.token_bytes, uuid4, os.urandom) and every clock is fixed, so the same
bytes come out on every run.
"""

import asyncio
import base64
import hashlib
import hmac
import json
import os
import secrets
import shutil
import sqlite3
import subprocess
import sys
import tempfile
import typing
import uuid
from datetime import datetime, timezone
from pathlib import Path
from typing import Any

REPO = Path(__file__).resolve().parents[3]
sys.path.insert(0, str(REPO))
if os.environ.get("PYTHONHASHSEED") != "0":                # set iteration order (Bridge.lost) fixed across runs
    os.execve(sys.executable, [sys.executable, *sys.argv], {**os.environ, "PYTHONHASHSEED": "0"})
HERE = Path(__file__).parent

# ---------- determinism: every random byte comes from a counter ----------

_n = [0]


def counter_bytes(n: int) -> bytes:
    """The Rust tests' `Counter` random source: SHA-256 of "ferb-fixture-<i>" for i = 1, 2, ..., cut to n bytes."""
    out = b""
    while len(out) < n:
        _n[0] += 1
        out += hashlib.sha256(f"ferb-fixture-{_n[0]}".encode()).digest()
    return out[:n]


def reset_random() -> None:
    _n[0] = 0


secrets.token_bytes = counter_bytes                       # token_hex / token_urlsafe read it through the module
os.urandom = counter_bytes                                 # seal.seal's nonce
uuid.uuid4 = lambda: uuid.UUID(bytes=counter_bytes(16), version=4)   # the fake extension's install id and nonces

from ferb.act.browser import ext_bridge, ext_protocol, pairing  # noqa: E402
from ferb.act.browser.ext_protocol import (ExtMessage, Event, Hello, Result, TabEvent, daemon_envelope,  # noqa: E402
                                           daemon_mac, mac_text, parse, sign, version_ok, version_tuple)
from ferb.act.browser.profiles import BROWSERS  # noqa: E402
from ferb.act.mac import browsers as mac_browsers  # noqa: E402
from ferb.clock.clock import FakeClock  # noqa: E402
from ferb.core.types import Identity  # noqa: E402
from ferb.identity import seal as seal_mod  # noqa: E402
from ferb.store.db import Database  # noqa: E402

T0 = datetime(2026, 3, 4, 5, 6, 7, 890123, tzinfo=timezone.utc)


def b64(data: bytes) -> str:
    return base64.b64encode(data).decode()


def outcome(fn: Any) -> Any:
    try:
        return {"ok": fn()}
    except Exception as e:                                   # noqa: BLE001 (any refusal is a recorded outcome)
        return {"error": type(e).__name__}


# ---------- protocol ----------

VERSIONS = ["0.19.0", "0.19.10", "0.19.9", "0.18.99", "1", "0.19", "0.19.0.1", "0.19.0-beta", "v0.20.0", "", "..",
            "0.19.x", "10.0.0", "0.019.000", "0.19.0b2", "0.1٩.0", "0.19.²", " 0.19.1", "0.20"]

LAX_INT = [5, -5, 0, True, False, 5.0, -0.0, 5.5, 1e20, 2.0 ** 63, -(2.0 ** 63), 9007199254740993.0, "5", " 5 ", "+5",
           "-5", "05", "5.", "5.0", "5.00", "5.10", ".0", "5.0_0", "1.0.0", "1e3", "_1", "1_", "1__0", "1_000",
           "1_0.0", "\t5\n", " 5", " 5", "5　", "\u00855", "\u001c5", " -0 ", "-5.0", "0x5", "0b1", "",
           "inf", "nan", "١", "5 0", None, [1], {"a": 1}, 9223372036854775807, -9223372036854775808,
           "9223372036854775807", "-9223372036854775808", "+_5", "-.0"]
LAX_BOOL = [True, False, 0, 1, 2, -1, 0.0, 1.0, 0.5, "0", "1", "true", "TRUE", "True", "yes", "Yes", "y", "on", "t",
            "false", "no", "n", "off", "f", " true", "1.0", "", None, [], "ON", "oFf"]
LAX_STR = ["", "x", "é", True, 5, 5.0, None, [], {"a": 1}]


def lax(t: Any, values: list[Any]) -> list[dict[str, Any]]:
    from pydantic import TypeAdapter
    ta = TypeAdapter(t)
    out = []
    for v in values:
        try:
            got = ta.validate_python(v)
            out.append({"in": v, "out": got})
        except Exception:                                     # noqa: BLE001
            out.append({"in": v, "error": True})
    return out


KEY = bytes(range(32))
INSTALL = "inst-0123456789abcdef"


def raw_messages() -> list[dict[str, Any]]:
    """Outer frames as json.loads gives them: what parse() keeps of each, or None."""
    good = {"install_id": INSTALL, "type": "hello", "body_json": "{}"}
    cases: list[Any] = [good, {**good, "seq": 3, "mac": "abc"}, {**good, "seq": "3"}, {**good, "seq": 3.0},
                        {**good, "seq": 3.5}, {**good, "seq": True}, {**good, "seq": None}, {**good, "mac": 5},
                        {**good, "mac": None}, {**good, "extra": [1, 2]}, {**good, "install_id": "1234567"},
                        {**good, "install_id": "12345678"}, {**good, "install_id": "x" * 64},
                        {**good, "install_id": "x" * 65}, {**good, "install_id": "é" * 64},
                        {**good, "install_id": 12345678}, {**good, "type": "welcome"}, {**good, "type": "HELLO"},
                        {**good, "type": "result"}, {**good, "type": "pair"}, {**good, "type": "error"},
                        {**good, "type": "tab_event"}, {**good, "type": "event"}, {**good, "type": None},
                        {**good, "body_json": {"a": 1}}, {**good, "body_json": "x" * (1024 * 1024)},
                        {**good, "body_json": "x" * (1024 * 1024 + 1)}, {**good, "body_json": "é" * (1024 * 1024)},
                        {k: v for k, v in good.items() if k != "body_json"},
                        {k: v for k, v in good.items() if k != "type"}, {},
                        {**good, "seq": " 7 "}, {**good, "seq": "1_0"}, {**good, "seq": -4}]
    out = []
    for raw in cases:
        m = parse(raw)
        big = isinstance(raw.get("body_json"), str) and len(raw["body_json"]) > 1000
        out.append({"raw": raw if not big else {**raw, "body_json": None},
                    "big_body": [len(raw["body_json"]), raw["body_json"][0]] if big else None,
                    "parsed": None if m is None else {"install_id": m.install_id, "type": m.type,
                                                      "body_json": m.body_json if not big else None,
                                                      "seq": m.seq, "mac": m.mac}})
    return out


def body_cases() -> list[dict[str, Any]]:
    """ExtMessage.body(): the body object, or the ValueError."""
    texts = ["{}", '{"a": 1}', "[]", "1", '"x"', "null", "", "{", '{"a": NaN}', '{"a": 1} ', ' {"a":1}',
             '{"a": "\\ud800"}', '{"a": 1, "a": 2}', '{"big": 123456789012345678901234567890}', '{"f": 1.5e300}']
    out = []
    for t in texts:
        m = ExtMessage(install_id=INSTALL, type="hello", body_json=t)
        try:
            v = m.body()
            out.append({"text": t, "dumped": json.dumps(v)})
        except ValueError:
            out.append({"text": t, "error": True})
    return out


def model_cases() -> dict[str, list[dict[str, Any]]]:
    def run(model: Any, bodies: list[dict[str, Any]]) -> list[dict[str, Any]]:
        out = []
        for b in bodies:
            try:
                out.append({"body": b, "dump": model.model_validate(b).model_dump()})
            except ValueError:
                out.append({"body": b, "error": True})
        return out
    hello = {"version": "0.19.10", "protocol": 21, "nonce": "n" * 32}
    hellos = [hello, {**hello, "browser": "chrome", "paired": True, "owned": [{"id": "c-1", "tab_id": 3}],
                      "seen": [{"id": "c-2", "status": "done"}], "armed": [4, "5", True], "tabs": [{"tab_id": 9}],
                      "file_access": True}, {**hello, "protocol": "21"}, {**hello, "protocol": 21.0},
              {**hello, "protocol": 21.5}, {"protocol": 21}, {"version": "x"}, {**hello, "version": 19},
              {**hello, "nonce": "n" * 65}, {**hello, "nonce": None}, {**hello, "paired": "yes"},
              {**hello, "paired": 2}, {**hello, "owned": {"a": 1}}, {**hello, "owned": [1]}, {**hello, "armed": [1.5]},
              {**hello, "armed": None}, {**hello, "file_access": None}, {**hello, "file_access": "off"},
              {**hello, "browser": None}, {**hello, "seen": [{"id": 1}], "extra": 1}]
    res = {"id": "c-1", "gen": 2, "status": "ok"}
    results = [res, {**res, "value": {"a": [1, None]}}, {**res, "status": "error", "error": "boom", "outcome": "unknown",
                                                         "step": "s" * 64, "steps": {"a": 1}, "dialog": {"type": "x"}},
               {**res, "step": "s" * 65}, {**res, "status": "weird"}, {**res, "outcome": "maybe"},
               {**res, "gen": "2"}, {**res, "gen": True}, {**res, "id": 5}, {"gen": 1, "status": "ok"},
               {**res, "error": None, "steps": None, "dialog": None, "value": None}, {**res, "steps": [1]},
               {**res, "value": "x" * 10}]
    tev = {"event": "updated", "tab_id": 4}
    tabs = [tev, {**tev, "url": "https://a.example/x", "title": "T", "active": True, "incognito": False},
            {**tev, "event": "sound", "audible": True, "muted": False, "ferb": True, "changed": ["audible", "muted"],
             "muted_reason": "user"}, {**tev, "changed": ["volume"]}, {**tev, "event": "moved"},
            {**tev, "tab_id": "4"}, {**tev, "same_document": True, "closed_by_page": False},
            {**tev, "url": None, "active": "no"}, {"event": "removed"}]
    ev = {"event": "download", "tab_id": 4}
    events = [ev, {**ev, "download_id": "d" * 64, "name": "n" * 4096, "mime": "m" * 256, "size": 0,
                   "source_url": "u" * 8192, "source": "link", "blank_tab": True, "uncaptured": False, "count": 1},
              {**ev, "download_id": "d" * 65}, {**ev, "size": -1}, {**ev, "count": 0}, {**ev, "name": "n" * 4097},
              {**ev, "event": "dialog", "type": "confirm", "message": "Sure?", "handled": "accepted"},
              {**ev, "event": "detached", "reason": "canceled_by_user"},
              {**ev, "event": "file_chooser", "frame_id": "f", "session_id": "s", "backend_node_id": 7,
               "mode": "selectSingle", "native_picker_shown": False, "transferred": False,
               "direct_upload_available": True, "detail": "x"},
              {**ev, "event": "open", "url": "https://a.example/", "window_id": 3, "background": True, "open_id": "o1"},
              {**ev, "event": "popup", "url": "https://a.example/", "opener_tab_id": 2},
              {**ev, "event": "taken", "reason": "moved_out_of_group"}, {**ev, "event": "closed"},
              {**ev, "size": "12"}, {**ev, "url": "u" * 8193}]
    return {"hello": run(Hello, hellos), "result": run(Result, results), "tab_event": run(TabEvent, tabs),
            "event": run(Event, events)}


MAC_CASES = [
    (KEY, INSTALL, 1, "hello", '{"version": "0.19.10"}'),
    (KEY, INSTALL, 9007199254740993, "result", '{"id":"c-1","gen":1,"status":"ok","value":{"t":"é ☃ 𝄞"}}'),
    (b"k", "inst-xxxxxxxx", 0, "pair", '{"token": "abc"}'),
    (b"", "inst-xxxxxxxx", -1, "event", ""),
    (bytes(range(64)) * 2, "inst-é-xxxxx", 42, "tab_event", '{"title": "a|b|c"}'),
]


def protocol_fixture() -> dict[str, Any]:
    signed = []
    for key, inst, seq, mtype, body in MAC_CASES:
        m = ExtMessage(install_id=inst, type=mtype, body_json=body, seq=seq, mac=sign(key, inst, seq, mtype, body))
        bad = ExtMessage(install_id=inst, type=mtype, body_json=body + " ", seq=seq, mac=m.mac)
        signed.append({"key": b64(key), "install_id": inst, "seq": seq, "type": mtype, "body_json": body,
                       "text": mac_text(inst, seq, mtype, body).decode(), "mac": m.mac,
                       "authentic": ext_protocol.authentic(key, m), "tampered_authentic": ext_protocol.authentic(key, bad)})
    unsigned = ExtMessage(install_id=INSTALL, type="hello", body_json="{}")
    envelopes = []
    for nonce, seq, mtype, body in [("n" * 32, 1, "welcome", {"gen": 1, "identity": "owner"}),
                                    ("abc", 2, "cmd", {"id": "c-1", "gen": 1, "op": "ping", "args": {},
                                                       "timeout_ms": 5000}),
                                    ("", 7, "cmd", {"text": "é ☃ 𝄞 \u007f   \"q\" \\ /", "n": 1.5, "b": None,
                                                    "l": [True, False], "f": 1e16, "neg": -0.0})]:
        envelopes.append({"key": b64(KEY), "nonce": nonce, "seq": seq, "type": mtype, "body": body,
                          "envelope": daemon_envelope(KEY, nonce, seq, mtype, body),
                          "mac": daemon_mac(KEY, nonce, seq, mtype, json.dumps(body))})
    return {
        "constants": {"protocol": ext_protocol.PROTOCOL_VERSION, "min_version": ext_protocol.MIN_EXTENSION_VERSION,
                      "extension_id": ext_protocol.FERB_EXTENSION_ID, "max_message": ext_protocol.MAX_MESSAGE,
                      "ext_types": list(typing.get_args(ext_protocol.ExtType)),
                      "ops": list(typing.get_args(ext_protocol.Op)), "reads": sorted(ext_protocol.READS),
                      "pdf": ext_protocol.PDF_EXTENSION_VERSION, "html": ext_protocol.HTML_EXTENSION_VERSION,
                      "owner_view": ext_protocol.OWNER_VIEW_EXTENSION_VERSION,
                      "upload_bytes": ext_protocol.UPLOAD_BYTES_EXTENSION_VERSION,
                      "input": ext_protocol.INPUT_EXTENSION_VERSION, "upload_part": ext_protocol.UPLOAD_PART,
                      "cmd_timeout_s": ext_bridge.CMD_TIMEOUT_S, "keepalive_s": ext_bridge.KEEPALIVE_S,
                      "token_ttl_s": pairing.TOKEN_TTL_S},
        "versions": [{"v": v, "tuple": outcome(lambda v=v: list(version_tuple(v))), "ok": outcome(lambda v=v: version_ok(v)),
                      "ok_input": outcome(lambda v=v: version_ok(v, ext_protocol.INPUT_EXTENSION_VERSION))}
                     for v in VERSIONS],
        "lax": {"int": lax(int, LAX_INT), "bool": lax(bool, LAX_BOOL), "str": lax(str, LAX_STR)},
        "parse": raw_messages(),
        "body": body_cases(),
        "models": model_cases(),
        "signed": signed,
        "unsigned_authentic": ext_protocol.authentic(KEY, unsigned),
        "envelopes": envelopes,
        "command": ext_protocol.command("c-1", 3, "snapshot", {"tab_id": 4, "snap": 1}, 30000),
        "text": text_rules(),
    }


ENTERS = [" ", "a", "A", "Enter", "enter", "Tab", "Space", "space", "spacebar", "cmd+v", "Cmd+V", "meta+v", "ctrl+v",
          "cmd+a", "shift+a", "shift+A", "alt+a", "cmd+shift+v", "F5", "ArrowLeft", "", "+", "a+", "é", "ab",
          "Return", "Backspace", "opt+e", "control+c", "1", "!", "shift+1", " cmd + v ", "command+v", "Escape",
          "cmd+Enter", "hyper+a", "x+a", "+a", "v", " "]
GROUPS = ["", "   ", "Book a table", "a" * 30, "a" * 31, "Find the cheapest flight to Lisbon next weekend please",
          "word, word; word: word. word word word", "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa bbb",
          "Réserver une table près de la gare, svp", "one two\tthree\nfour", "x" * 29 + ", tail", "αβγ " * 12]
APPS = [None, "com.google.Chrome", "com.google.Chrome.helper", "com.google.Chrome.helper.Renderer", "com.google.chrome",
        "com.brave.Browser", "com.apple.Safari", "com.example.notabrowser", "com.operasoftware.OperaGX",
        "com.google.Chrome.canary", "org.chromium.Thorium", "com.google.Chromex", "COM.GOOGLE.CHROME.HELPER"]


def text_rules() -> dict[str, Any]:
    nr = []
    for op, timeout, step, steps in [("snapshot", 30.0, None, None), ("screenshot", 2.5, "capture", {"a_ms": 3}),
                                     ("find", 30, "ax", {"x": 1, "y": True, "z": "s", "w": 1.5}),
                                     ("wait", 10.0, "mystery", None), ("snapshot", 0.25, "", {})]:
        e = ext_bridge.not_responding(op, timeout, step, steps)
        nr.append({"op": op, "timeout": timeout, "step": step, "steps": steps, "message": str(e), "facts": e.facts,
                   "timing": e.timing})
    return {
        "enters_text": [{"spec": s, "enters": ext_bridge.enters_text(s)} for s in ENTERS],
        "group_title": [{"text": t, "title": ext_bridge.group_title(t)} for t in GROUPS],
        "too_old": ext_bridge.too_old("0.18.2"),
        "version_refusal": [{"version": v, "protocol": p, "out": list(ext_bridge.version_refusal(v, p))}
                            for v, p in [("0.18.2", 20), ("0.30.0", 22), ("0.19.10", 21), ("0.17.0", 21)]],
        "peer_refusal": [{"app": a, "browser": b, "why": ext_bridge.peer_refusal(a, b)}
                         for a in APPS for b in (None, "chrome", "brave", "nosuch")],
        "browsers": {k: {"bundle_id": v.bundle_id, "label": v.label} for k, v in BROWSERS.items()},
        "known": {a: mac_browsers.known(a) for a in APPS if a},
        "not_responding": nr,
        "read_steps": ext_bridge.READ_STEPS,
    }


# ---------- crypto: Python and protocol.js ----------

SEALS = [(KEY, bytes(range(12)), "hunter2-invented", "c-0001", "https://login.example"),
         (bytes([7]) * 32, bytes([255]) * 12, "", "f-abcdef", "https://a.example:8443"),
         (counter_bytes(32), counter_bytes(12), "é ☃ 𝄞 | pipes | in value", "r-1|2", "https://mail.google.com/mail/feed/atom"),
         (counter_bytes(32), counter_bytes(12), "x" * 33000, "c-big", "https://big.example")]


def crypto_fixture() -> dict[str, Any]:
    reset_random()
    py_seals = []
    for key, iv, text, cid, origin in SEALS:
        os.urandom = lambda n, iv=iv: iv[:n]
        sealed = seal_mod.seal(seal_mod.seal_key(key), text, cid, origin)
        py_seals.append({"key": b64(key), "seal_key": b64(seal_mod.seal_key(key)), "iv": b64(iv), "text": text,
                         "command": cid, "origin": origin, "sealed": sealed})
    os.urandom = counter_bytes
    daemon = [{"key": e["key"], "nonce": e["nonce"], "seq": e["seq"], "type": e["type"], "body": e["envelope"]["body"],
               "mac": e["envelope"]["mac"]} for e in protocol_fixture_envelopes()]
    extension = [{"key": b64(k), "install_id": i, "seq": s, "type": t, "body": json.loads(b)}
                 for k, i, s, t, b in MAC_CASES if b and s < 2 ** 53]
    for c in extension:                                       # what Python says of the extension's own body_json
        c["py_mac_of_js_body"] = None
    js_cases = {"daemon": daemon, "extension": extension,
                "seal": [{"key": s["key"], "iv": s["iv"], "text": s["text"], "command": s["command"],
                          "origin": s["origin"]} for s in py_seals],
                "unseal": [{"key": s["key"], "sealed": s["sealed"], "command": s["command"], "origin": s["origin"]}
                           for s in py_seals]
                + [{"key": s["key"], "sealed": s["sealed"], "command": s["command"] + "x", "origin": s["origin"]}
                   for s in py_seals[:1]]
                + [{"key": s["key"], "sealed": s["sealed"], "command": s["command"], "origin": s["origin"] + "/"}
                   for s in py_seals[:1]],
                "enters_text": ENTERS}
    js = run_node(js_cases)
    for c, got in zip(extension, js["extension"]):
        c["js_body_json"] = got["body_json"]
        c["js_mac"] = got["mac"]
        c["py_mac_of_js_body"] = sign(base64.b64decode(c["key"]), c["install_id"], c["seq"], c["type"],
                                      got["body_json"])
    for c, got in zip(daemon, js["daemon"]):
        c["js_text"], c["js_mac"] = got["text"], got["mac"]
    js_seals = []
    for s, got in zip(py_seals, js["seal"]):
        try:
            py_open = seal_mod.unseal(seal_mod.seal_key(base64.b64decode(s["key"])), got, s["command"], s["origin"])
        except Exception:                                     # noqa: BLE001
            py_open = None
        js_seals.append({"key": s["key"], "iv": s["iv"], "text": s["text"], "command": s["command"],
                         "origin": s["origin"], "sealed": got, "python_opens": py_open})
    return {"node": subprocess.run(["node", "--version"], capture_output=True, text=True).stdout.strip(),
            "constants": {k: js[k] for k in ("protocol", "ops", "read_ops", "daemon_signed", "upload_part",
                                             "download_part")},
            "daemon": daemon, "extension": extension, "python_seals": py_seals, "js_seals": js_seals,
            "js_unseal": [{"case": c, **r} for c, r in zip(js_cases["unseal"], js["unseal"])],
            "js_enters_text": [{"spec": s, "enters": e} for s, e in zip(ENTERS, js["enters_text"])]}


def protocol_fixture_envelopes() -> list[dict[str, Any]]:
    out = []
    for nonce, seq, mtype, body in [("n" * 32, 1, "welcome", {"gen": 1, "identity": "owner"}),
                                    ("0f" * 16, 12, "cmd", {"id": "c-1", "gen": 1, "op": "fill",
                                                            "args": {"sealed": {"nonce": "AA==", "ct": "BB=="},
                                                                     "origin": "https://a.example"},
                                                            "timeout_ms": 15000}),
                                    ("abc", 3, "cmd", {"text": "é ☃ 𝄞   \"q\" \\ /"})]:
        out.append({"key": b64(KEY), "nonce": nonce, "seq": seq, "type": mtype,
                    "envelope": daemon_envelope(KEY, nonce, seq, mtype, body)})
    return out


def run_node(cases: dict[str, Any]) -> dict[str, Any]:
    with tempfile.TemporaryDirectory() as tmp:
        path = Path(tmp) / "cases.json"
        path.write_text(json.dumps(cases))
        r = subprocess.run(["node", str(HERE / "js_vectors.js"), str(REPO / "extensions" / "chromium" / "protocol.js"),
                            str(path)], capture_output=True, text=True, timeout=60)
    if r.returncode != 0:
        raise SystemExit(f"node failed: {r.stderr}")
    return dict(json.loads(r.stdout))


# ---------- pairing ----------

def rows(db: Database) -> list[dict[str, Any]]:
    out = []
    for r in db.conn.execute("SELECT install_id, browser, folder, identity, hex(key) AS key, typeof(key) AS key_type, "
                             "last_seq, typeof(last_seq) AS seq_type, paired_at, ext_version FROM ext_installs "
                             "ORDER BY install_id").fetchall():
        out.append(dict(r))
    return out


def verdict(v: pairing.Verdict) -> dict[str, Any]:
    b = v.binding
    return {"accepted": v.accepted, "reason": v.reason, "reply": v.reply,
            "binding": None if b is None else {"install_id": b.install_id, "browser": b.browser, "folder": b.folder,
                                               "identity": b.identity.value}}


def ext_msg(install: str, mtype: str, body: Any, key: bytes | None = None, seq: int | None = None,
            body_json: str | None = None) -> dict[str, Any]:
    raw: dict[str, Any] = {"install_id": install, "type": mtype,
                           "body_json": body_json if body_json is not None else json.dumps(body)}
    if key is not None and seq is not None:
        raw["seq"], raw["mac"] = seq, sign(key, install, seq, mtype, raw["body_json"])
    elif seq is not None:
        raw["seq"] = seq
    return raw


def pairing_fixture() -> dict[str, Any]:
    reset_random()
    steps: list[dict[str, Any]] = []
    with tempfile.TemporaryDirectory() as tmp:
        db = Database(Path(tmp) / "ferb.db")
        db.migrate()
        clock = FakeClock(T0)
        p = pairing.Pairings(db, clock)
        keys: dict[str, bytes] = {}

        def step(op: str, args: dict[str, Any], fn: Any) -> Any:
            got = fn()
            entry = {"op": op, "args": args, "rows": rows(db)}
            if isinstance(got, pairing.Verdict):
                entry["verdict"] = verdict(got)
                if got.reply:
                    keys[args["raw"]["install_id"]] = base64.b64decode(got.reply["key"])
            elif isinstance(got, pairing.Binding):
                entry["binding"] = {"install_id": got.install_id, "browser": got.browser, "folder": got.folder,
                                    "identity": got.identity.value}
            elif isinstance(got, list) and got and isinstance(got[0], pairing.Binding):
                entry["bindings"] = [[b.install_id, b.browser, b.folder, b.identity.value] for b in got]
            elif isinstance(got, bytes):
                entry["bytes"] = got.hex()
            else:
                entry["value"] = got
            steps.append(entry)
            return got

        def handle(raw: dict[str, Any]) -> pairing.Verdict:
            m = parse(raw)
            assert m is not None, raw
            return step("handle", {"raw": raw}, lambda: p.handle(m))

        def advance(s: float) -> None:
            steps.append({"op": "advance", "args": {"seconds": s}, "rows": rows(db)})
            clock.advance(s)

        a, b, c = "inst-aaaaaaaaaaaa", "inst-bbbbbbbbbbbb", "inst-cccccccccccc"
        t1 = step("issue_token", {"browser": "chrome", "folder": "Profile 1", "identity": "owner"},
                  lambda: p.issue_token("chrome", "Profile 1"))
        step("pending", {"browser": "chrome", "folder": "Profile 1"}, lambda: p.pending("chrome", "Profile 1"))
        step("pending", {"browser": "chrome", "folder": "Default"}, lambda: p.pending("chrome", "Default"))
        handle(ext_msg(a, "hello", {"version": "0.19.10", "protocol": 21, "browser": "chrome"}))
        handle(ext_msg(a, "tab_event", {"event": "created", "tab_id": 1}))
        handle(ext_msg(a, "pair", {}, body_json="[]"))
        handle(ext_msg(a, "pair", {}, body_json="{"))
        handle(ext_msg(a, "pair", {"token": 5}))
        handle(ext_msg(a, "pair", {"token": "nope"}))
        handle(ext_msg(a, "pair", {"token": t1}))
        step("pending", {"browser": "chrome", "folder": "Profile 1"}, lambda: p.pending("chrome", "Profile 1"))
        handle(ext_msg(b, "pair", {"token": t1}))                   # single use
        ka = keys[a]
        step("key", {"install_id": a}, lambda: p.key(a))
        step("key", {"install_id": "inst-nosuchinstall"}, lambda: p.key("inst-nosuchinstall"))
        step("version", {"install_id": a}, lambda: p.version(a))
        handle(ext_msg(a, "hello", {"version": "0.19.10", "protocol": 21}, ka, 1))
        handle(ext_msg(a, "hello", {"version": "0.19.10", "protocol": 21}, ka, 1))          # replay
        handle(ext_msg(a, "tab_event", {"event": "created", "tab_id": 1}, ka, 0))           # replay
        handle(ext_msg(a, "tab_event", {"event": "created", "tab_id": 1}, b"x" * 32, 2))    # bad mac
        handle(ext_msg(a, "tab_event", {"event": "created", "tab_id": 1}, None, 2))         # no mac
        handle(ext_msg(a, "tab_event", {"event": "created", "tab_id": 1}))                  # unsigned
        handle(ext_msg(a, "result", {"id": "c", "gen": 1, "status": "ok"}, ka, 7))
        handle(ext_msg(a, "event", {"event": "taken", "tab_id": 1}, ka, 6))                 # below last_seq
        handle(ext_msg(a, "pair", {"token": "whatever"}, ka, 8))                             # pair is never MAC'd
        step("record_version", {"install_id": a, "version": "0.18.2"}, lambda: p.record_version(a, "0.18.2"))
        step("record_version", {"install_id": a, "version": "v" * 40}, lambda: p.record_version(a, "v" * 40))
        step("record_version", {"install_id": "inst-nosuchinstall", "version": "1"},
             lambda: p.record_version("inst-nosuchinstall", "1"))
        step("version", {"install_id": a}, lambda: p.version(a))
        t2 = step("issue_token", {"browser": "brave", "folder": "Default", "identity": "ferb"},
                  lambda: p.issue_token("brave", "Default", Identity.FERB))
        advance(300)
        step("pending", {"browser": "brave", "folder": "Default"}, lambda: p.pending("brave", "Default"))
        handle(ext_msg(b, "pair", {"token": t2}))                   # exactly at the deadline: still good
        t3 = step("issue_token", {"browser": "chrome", "folder": "Profile 2", "identity": "owner"},
                  lambda: p.issue_token("chrome", "Profile 2"))
        advance(300.5)
        step("pending", {"browser": "chrome", "folder": "Profile 2"}, lambda: p.pending("chrome", "Profile 2"))
        handle(ext_msg(c, "pair", {"token": t3}))                   # expired (and gone)
        handle(ext_msg(c, "pair", {"token": t3}))
        t4 = step("issue_token", {"browser": "chrome", "folder": "Profile 1", "identity": "owner"},
                  lambda: p.issue_token("chrome", "Profile 1"))
        step("revoke", {"token": t4}, lambda: p.revoke(t4))
        step("revoke", {"token": "never-issued"}, lambda: p.revoke("never-issued"))
        step("pending", {"browser": "chrome", "folder": "Profile 1"}, lambda: p.pending("chrome", "Profile 1"))
        handle(ext_msg(c, "pair", {"token": t4}))
        t5 = step("issue_token", {"browser": "chrome", "folder": "Profile 1", "identity": "owner"},
                  lambda: p.issue_token("chrome", "Profile 1"))
        handle(ext_msg(c, "pair", {"token": t5}, ka, 1))           # a new install for a paired profile replaces it
        t6 = step("issue_token", {"browser": "chrome", "folder": "Profile 3", "identity": "owner"},
                  lambda: p.issue_token("chrome", "Profile 3"))
        handle(ext_msg(c, "pair", {"token": t6}))                   # the same install for another profile moves
        step("binding", {"install_id": c}, lambda: p.binding(c))
        step("for_profile", {"browser": "brave", "folder": "Default"}, lambda: p.for_profile("brave", "Default"))
        step("for_profile", {"browser": "chrome", "folder": "Profile 1"}, lambda: p.for_profile("chrome", "Profile 1"))
        step("all", {}, lambda: p.all() or None)
        step("unpair", {"browser": "brave", "folder": "Default"}, lambda: p.unpair("brave", "Default"))
        step("unpair", {"browser": "brave", "folder": "Default"}, lambda: p.unpair("brave", "Default"))
        step("all", {}, p.all)
        step("paired", {}, lambda: sorted(p.paired))
        db.conn.close()
    return {"start": T0.isoformat(), "monotonic_start": 1000.0, "steps": steps}


def digit_tables() -> None:
    """src/generated.rs: Python 3's str.isdigit() beyond its decimal digits, and where each run of decimal digits
    starts (unicodedata.decimal == 0), for version_tuple's int() of the digits it keeps."""
    import unicodedata
    zeros, others = [], []
    for cp in range(0x110000):
        ch = chr(cp)
        if ch.isdecimal() and unicodedata.decimal(ch) == 0:
            zeros.append(cp)
        elif ch.isdigit() and not ch.isdecimal():
            others.append(cp)
    def table(name: str, cps: list[int], doc: str) -> str:
        body = "".join(f"    0x{c:X},\n" for c in cps)
        return f"/// {doc}\npub const {name}: [u32; {len(cps)}] = [\n{body}];\n"
    text = ("//! Generated by rust/fixtures/browser/make.py from Python " + sys.version.split()[0]
            + " (Unicode " + unicodedata.unidata_version + "). Do not edit.\n\n"
            + table("DECIMAL_ZEROS", zeros, "Each run of ten decimal digits starts here (its digit 0).")
            + "\n" + table("DIGITS_NOT_DECIMAL", others, "`str.isdigit()` is true, `str.isdecimal()` false: int() refuses them."))
    folds = [(cp, chr(cp).casefold()) for cp in range(0x80, 0x110000)
             if chr(cp).casefold().isascii() and chr(cp).casefold() != chr(cp)]
    text += ("\n/// Characters beyond ASCII whose `str.casefold()` is ASCII text (compared with FERB's ASCII words).\n"
             f"pub const ASCII_FOLDS: [(u32, &str); {len(folds)}] = [\n"
             + "".join(f"    (0x{cp:X}, {json.dumps(f)}),\n" for cp, f in folds) + "];\n")
    out = REPO / "rust" / "crates" / "ferb-browser" / "src" / "generated.rs"
    out.parent.mkdir(parents=True, exist_ok=True)
    out.write_text(text)


# ---------- snapshot ----------

def snapshot_fixture() -> dict[str, Any]:
    """snapshot.py: element() on raw elements as the extension sends them, for_model() previews and full forms, and
    bot_check() on frame addresses."""
    import dataclasses
    from ferb.act.browser import snapshot as snap
    from ferb.core.types import PageSnapshot, TabRef
    raws = [
        {},
        {"role": "button", "label": "Pay ₹248.75", "rect": [1, 2.5, 30, 40], "submit": {"click": True, "enter": False,
         "action_host": "pay.example", "method": "post", "kinds": ["card_number", "nope", "card_csc"],
         "label": "Pay now"}, "path": "html>body>form:0>button:1", "backend_node": 77},
        {"role": "textbox", "label": "Password", "secret": True, "value": "hunter2", "input_type": "password",
         "code_fill": True, "field_kind": "password", "in_view": False, "offscreen": "below", "occluded": True},
        {"role": "textbox", "label": "Card", "field_kind": "card_number", "value": "", "code_fill": True},
        {"role": "textbox", "label": "Code", "field_kind": "made_up", "value": "123"},
        {"role": None, "label": None, "value": None, "disabled": 1, "expanded": False, "selected": True, "level": 2.7,
         "terminal": True, "dialog": True, "container": "", "backend_node": True, "path": 5},
        {"role": "treeitem", "label": "x" * 100, "expanded": True, "level": "3", "in_view": None, "offscreen": "left",
         "submit": {"click": False, "enter": False}, "href": "https://a.example/", "form": "f1"},
        {"role": "link", "label": "Next", "submit": {"enter": True, "action_host": "", "method": "", "kinds": "x",
                                                   "label": 5}, "path": "p" * 2100, "rect": ["1", 2, 3, 4]},
        {"role": "combobox", "label": "Country", "value": "PT", "input_type": "text", "container": "c1",
         "path": "html>body>form:1>select:0"},
        {"role": "button", "label": "Send", "input_type": "submit", "submit": {"click": True},
         "path": "html>body>form:1>button:0", "in_view": False},
    ]
    elements = [snap.element(f"s1e{i + 1}", "0" if i % 3 else "2", r) for i, r in enumerate(raws)]
    tab = TabRef("dev-1", "chrome", "Profile 1", 1, "t1")
    many_raw = [{"role": ["button", "link", "listitem", "textbox"][i % 4], "label": f"item {i} " + "w" * (i % 30),
                 "in_view": i % 5 != 0, "dialog": i == 300, "path": f"html>body>form:{i % 2}>x:{i}",
                 "submit": {"click": True} if i % 97 == 0 else None} for i in range(400)]
    many = [snap.element(f"s2e{i + 1}", "0", r) for i, r in enumerate(many_raw)]
    snaps = {
        "small": PageSnapshot(tab, 1, "https://a.example/x", "A", elements, "hello world", False, [], None, [], [], []),
        "everything": PageSnapshot(tab, 1, "https://a.example/x", "A", elements, "é" * 9000, True,
                                   ["confirm: Sure? (dismissed)"], {"y": 0, "height": 2000}, [{"id": "c1", "frame": 0}],
                                   [{"index": 0, "id": "m1-abcdef", "playing": True}, {"index": 1}],
                                   ["https://challenges.cloudflare.com/x", "https://a.example/"], loading=True,
                                   content_type="application/pdf"),
        "big": PageSnapshot(tab, 2, "https://b.example/", "B", many, "t" * 100, False, [], None, [], [], []),
    }
    out = {"elements": [{"raw": r, "element": dataclasses.asdict(e)} for r, e in zip(raws, elements)],
           "many_raw": many_raw, "snapshots": {}}
    for name, s in snaps.items():
        out["snapshots"][name] = {
            "snapshot": {**{k: v for k, v in dataclasses.asdict(s).items() if k != "elements"},
                         "elements": "many" if s.elements is many else "elements"},
            "preview": snap.for_model(s), "full": snap.for_model(s, full=True),
            "probe": snap.for_model(s, probe=True) if s.media else None}
    urls = [[], ["https://challenges.cloudflare.com/cdn-cgi/x"], ["https://www.google.com/recaptcha/api2/anchor",
            "https://www.google.com/maps", "https://newassets.hcaptcha.com/captcha/v1", "https://x.recaptcha.net/a",
            "http://[::1/", "https://geo.captcha-delivery.com/captcha/", "https://hcaptcha.com.evil.example/"],
            ["https://client-api.arkoselabs.com/fc", "https://funcaptcha.com/x", "https://a.awswaf.com/",
             "https://static.geetest.com/", "https://ARKOSELABS.COM/"]]
    out["bot_check"] = [{"urls": u, "check": snap.bot_check(u)} for u in urls]
    return out


def main() -> None:
    import hands                                             # noqa: E402 (next to this file)
    only = os.environ.get("FIXTURE_ONLY", "")                # a comma list of names: write only those (development)
    if only:
        for name in only.split(","):
            if name == "ops":
                import driver_ops                            # noqa: E402 (next to this file)
                write("ops.json", driver_ops.make(reset_random))
                continue
            if name in MORE:
                import more                                  # noqa: E402 (next to this file)
                reset_random()
                write(f"{name}.json", getattr(more, f"{name}_fixture")())
                continue
            if name == "tools_more":
                import tools_more                            # noqa: E402 (next to this file)
                write("tools_more.json", tools_more.make(reset_random))
                continue
            reset_random()
            write(f"{name}.json", getattr(hands, f"{name}_fixture")())
        return
    reset_random()
    digit_tables()
    hands.mime_table()
    write("snapshot.json", snapshot_fixture())
    write("protocol.json", protocol_fixture())
    write("crypto.json", crypto_fixture())
    write("pairing.json", pairing_fixture())
    if os.environ.get("FIXTURE_SKIP_TRANSCRIPTS") != "1":
        import transcripts                                   # noqa: E402 (next to this file)
        write("transcripts.json", transcripts.make(reset_random))
    for name in hands.FIXTURES:
        reset_random()
        write(f"{name}.json", getattr(hands, f"{name}_fixture")())
    if os.environ.get("FIXTURE_SKIP_TRANSCRIPTS") != "1":
        import driver_ops                                    # noqa: E402 (next to this file)
        write("ops.json", driver_ops.make(reset_random))
    import more                                              # noqa: E402 (next to this file)
    for name in MORE:
        reset_random()
        write(f"{name}.json", getattr(more, f"{name}_fixture")())
    if os.environ.get("FIXTURE_SKIP_TRANSCRIPTS") != "1":
        import tools_more                                    # noqa: E402 (next to this file)
        write("tools_more.json", tools_more.make(reset_random))


MORE = ["profiles_mac", "uploads", "downloads"]             # more.py's, in this order


def write(name: str, data: Any) -> None:
    (HERE / name).write_text(json.dumps(data, indent=1, ensure_ascii=False, sort_keys=False) + "\n")
    print(f"{name}: {(HERE / name).stat().st_size} bytes")


if __name__ == "__main__":
    sys.path.insert(0, str(HERE))
    main()
