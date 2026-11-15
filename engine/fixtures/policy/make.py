"""Fixtures for the Rust port of ferb/policy (rust/crates/ferb-policy), made by the Python code itself (ENG-3).

    cd <repo> && uv run python rust/fixtures/policy/make.py

1. Regenerates rust/crates/ferb-policy/src/tables.rs (gen_tables.py): Python's Unicode facts and idna's tables.
2. Harvests inputs from the guarding tests: it runs the tests of the files in HARVEST that need no daemon, with every
   ferb/policy function below wrapped, and keeps the arguments of each call a test module made itself.
3. Adds generated cases that walk confirm.decide's branches (a seeded cross product), the sink rule, the wording of
   every rule, terminal inputs, certificate commands, payments in a scripted session with a fake clock, stored
   provenance, the stores (a real Database in a temporary folder) and the protection lists on a temporary file tree.
4. Runs the Python code on every case again, outside the tests, and writes each section's cases next to this file:
   inputs and everything produced (returns, raised exception names, stored rows, composed text). The Rust test
   (tests/parity.rs) replays each case and must get the same.
Seams are recorded as call logs (which apps a name finds, whether a destination is known), so the Rust side answers
exactly what Python was told. All data is invented; FERB_HOME and HOME are temporary folders.
"""

import base64
import dataclasses
import io
import json
import os
import random
import sqlite3
import subprocess
import sys
import tempfile
import unicodedata
from contextlib import redirect_stdout
from datetime import datetime, timedelta, timezone
from pathlib import Path
from typing import Any, Callable

REPO = Path(__file__).resolve().parents[3]
HERE = Path(__file__).parent
sys.path.insert(0, str(REPO))
TMP = Path(tempfile.mkdtemp(prefix="ferb-policy-fixtures-"))
os.environ["FERB_HOME"] = str(TMP / "ferbhome")
os.environ["HOME"] = str(TMP / "home")
(TMP / "home").mkdir()

subprocess.run([sys.executable, str(HERE / "gen_tables.py")], check=True)

import idna  # noqa: E402
import shlex  # noqa: E402
from urllib.parse import quote, quote_plus, unquote, unquote_plus, urlsplit, urlunsplit  # noqa: E402
import ipaddress  # noqa: E402

from ferb.core.ids import SequentialIds  # noqa: E402
from ferb.core.types import ActionSpec, Identity, Target, TerminalInput  # noqa: E402
from ferb.policy import (blocked, carried, confirm, destinations, escape, payments, protection, provenance,  # noqa: E402
                         rules, security_digest, services, sites, standing, targets, terminal, tls, urls)
from ferb.act.mac import apps as mac_apps  # noqa: E402
from ferb.act.mac import browsers as mac_browsers  # noqa: E402

HARVEST = ["test_arg_provenance.py", "test_browser_settings_pages.py", "test_confirmations.py",
           "test_foreground_asks.py", "test_payments.py", "test_question_clarity.py", "test_question_cards.py",
           "test_site_own_name.py", "test_same_site_pay.py", "test_sinks.py", "test_sink_false_alarms.py",
           "test_tls_floor.py", "test_terminal_input.py", "test_value_provenance.py", "test_context_exposure.py",
           "test_pay_press.py", "test_steering_scope.py", "test_standing_permission_campaign.py",
           "test_tool_origins.py", "test_mac_hands.py", "test_learning.py", "test_f6_standing.py",
           "test_value_provenance.py", "test_changes_protection.py"]
HEAVY = {"daemon", "running", "unattended", "browser", "ext_browser", "mac", "page_server"}


# ---------- serialization ----------

def norm(v: Any) -> Any:
    if isinstance(v, (set, frozenset)):
        return sorted(norm(x) for x in v)
    if isinstance(v, tuple):
        return [norm(x) for x in v]
    if isinstance(v, list):
        return [norm(x) for x in v]
    if isinstance(v, dict):
        return {str(k): norm(x) for k, x in v.items()}
    if isinstance(v, Path):
        return str(v)
    if dataclasses.is_dataclass(v) and not isinstance(v, type):
        return {f.name: norm(getattr(v, f.name)) for f in dataclasses.fields(v)}
    return v


def ser_target(t: Target | None) -> Any:
    if t is None:
        return None
    return {"account": t.account, "site": t.site, "recipients": list(t.recipients), "path": t.path,
            "amount": t.amount, "currency": t.currency, "contact": t.contact, "page": t.page, "item": t.item}


def de_target(d: Any) -> Target | None:
    if d is None:
        return None
    return Target(account=d["account"], site=d["site"], recipients=tuple(d["recipients"]), path=d["path"],
                  amount=d["amount"], currency=d["currency"], contact=d["contact"], page=d["page"], item=d["item"])


def ser_need(n: confirm.Need) -> dict[str, Any]:
    return {"rule": n.rule, "key": n.key, "values": [[k, v] for k, v in n.values.items()], "always": n.always,
            "destinations": [list(d) for d in n.destinations], "fresh": n.fresh, "spent_on_ask": n.spent_on_ask,
            "reaches": ser_target(n.reaches), "grants": n.grants}


def de_need(d: dict[str, Any]) -> confirm.Need:
    return confirm.Need(d["rule"], d["key"], {k: v for k, v in d["values"]}, always=d["always"],
                        destinations=tuple(tuple(x) for x in d["destinations"]), fresh=d["fresh"],
                        spent_on_ask=d["spent_on_ask"], reaches=de_target(d["reaches"]), grants=d["grants"])


TI_FIELDS = [f.name for f in dataclasses.fields(TerminalInput)]


def ser_ti(i: TerminalInput) -> dict[str, Any]:
    return {k: norm(getattr(i, k)) for k in TI_FIELDS}


def de_ti(d: dict[str, Any]) -> TerminalInput:
    kw = dict(d)
    kw["files"] = tuple(tuple(f) for f in kw["files"])
    kw["named"] = tuple(kw["named"])
    return TerminalInput(**kw)


def outcome(fn: Callable[[], Any], ser: Callable[[Any], Any] = norm) -> dict[str, Any]:
    try:
        return {"ok": ser(fn())}
    except Exception as e:      # the exception's class is the contract, never its words
        return {"raises": type(e).__name__}


# ---------- harvesting from the tests ----------

TEXTS: dict[int, tuple[str, Any]] = {}
HARVESTED: dict[str, list[str]] = {}
_orig_prepare = provenance.prepare


def _prepare(text: str) -> Any:
    t = _orig_prepare(text)
    TEXTS[id(t)] = (text, t)
    return t


def text_of(t: Any) -> str | None:
    hit = TEXTS.get(id(t))
    return hit[0] if hit is not None and hit[1] is t else None


def from_tests() -> bool:
    f = sys._getframe(2)
    name = f.f_globals.get("__name__", "")
    return name.startswith("tests.") or name.startswith("test_")


def harvest(module: Any, name: str, ser: Callable[..., Any]) -> None:
    orig = getattr(module, name)

    def wrapper(*a: Any, **k: Any) -> Any:
        if from_tests():
            try:
                case = ser(*a, **k)
            except Exception:
                case = None
            if case is not None:
                HARVESTED.setdefault(f"{module.__name__.rsplit('.', 1)[1]}.{name}", []).append(
                    json.dumps(case, sort_keys=True, ensure_ascii=False))
        return orig(*a, **k)
    wrapper.__wrapped__ = orig
    setattr(module, name, wrapper)


def recs(lst: Any) -> Any:
    out = []
    for order, t in lst:
        s = text_of(t)
        if s is None:
            raise ValueError("an unknown text")
        out.append([order, s])
    return out


def ser_sink(url, private, outside, owner, origins=None, sites=None, pages=None):
    ids = {id(t): i for i, (_, t) in enumerate(private)}
    return {"url": url, "private": recs(private), "outside": recs(outside), "owner": text_of(owner) if
            text_of(owner) is not None else (_ for _ in ()).throw(ValueError()),
            "origins": None if origins is None else {str(k): sorted(v) for k, v in origins.items()},
            "sites": None if sites is None else {str(ids[k]): sorted(v) for k, v in sites.items() if k in ids},
            "pages": None if pages is None else {str(k): v for k, v in pages.items()}}


def ser_text_sink(text, private, outside, owner):
    return {"text": text, "private": recs(private), "outside": recs(outside), "owner": text_of(owner)}


def install_harvest() -> None:
    provenance.prepare = _prepare
    harvest(provenance, "sources", lambda value, owner_text, external_texts, aliases=(), kind="name", answer_text="":
            {"value": value, "owner": owner_text, "external": [list(x) for x in external_texts],
             "aliases": list(aliases), "kind": kind, "answer": answer_text})
    harvest(provenance, "arg_sources", lambda target, owner_text, external_texts, answer_text="":
            {"target": ser_target(target), "owner": owner_text, "external": [list(x) for x in external_texts],
             "answer": answer_text})
    for name in ("url_carries_private", "url_private_values"):
        harvest(confirm, name, ser_sink)
    for name in ("text_carries_private", "text_private_values"):
        harvest(confirm, name, ser_text_sink)
    harvest(confirm, "decide", lambda kind, declared, target, arg_sources, **kw: {
        "kind": kind, "declared": norm(declared) if not isinstance(declared, str) else [declared],
        "target": ser_target(target), "arg_sources": norm(arg_sources), "read_private": kw["read_private"],
        "read_external": kw["read_external"], "outside_read": norm(list(kw.get("outside_read", ()))),
        "place": kw.get("place"), "content": kw.get("content"), "probe_known": True})
    for name in ("summary", "card"):
        harvest(confirm, name, lambda needs, typed="": {"needs": [ser_need(n) for n in needs], "typed": typed})
    harvest(confirm, "line", lambda needs, call, why, typed="": {"needs": [ser_need(n) for n in needs], "call": call,
                                                                 "why": why, "typed": typed})
    harvest(confirm, "foreground_need", lambda tool, args: {"tool": tool, "args": norm(args)})
    harvest(terminal, "surface", lambda bundle_id, chain, owner_apps=(), shell_app=False, unread=False:
            {"bundle_id": bundle_id, "chain": norm(list(chain)), "owner_apps": norm(list(owner_apps)),
             "shell_app": shell_app, "unread": unread})
    harvest(terminal, "need", lambda inp: ser_ti(inp))
    harvest(terminal, "run_control", lambda chain, editor=False: {"chain": norm(list(chain)), "editor": editor})
    harvest(tls, "certificate_checks_off", lambda command, depth=0: {"command": command})
    harvest(tls, "applescript_checks_off", lambda script: {"script": script})
    harvest(payments, "amount_key", lambda shown: {"shown": shown})
    harvest(carried, "derive", lambda text, shown, made=(): {"text": text, "shown": [
        [sorted(x[0]), x[1], x[2] if len(x) > 2 else None, x[3] if len(x) > 3 else ""] for x in shown],
        "made": sorted(made)})
    harvest(urls, "parse", lambda url: {"url": url})
    for name in ("own_contact",):
        harvest(confirm, name, lambda value, own: {"value": value, "own": sorted(own)})
    for name in ("page_scope", "whole_values", "registrable", "value_kind", "reads_as_ferbs", "task_question",
                 "reply_lines", "contacts_in"):
        harvest(confirm, name, lambda text: {"text": text})


class Light:
    """A pytest plugin: keeps the tests that need no daemon or browser."""

    def pytest_collection_modifyitems(self, config: Any, items: list[Any]) -> None:
        keep = [i for i in items if not (set(getattr(i, "fixturenames", ())) & HEAVY)]
        items[:] = keep


def run_harvest() -> None:
    import pytest
    install_harvest()
    files = [str(REPO / "tests" / f) for f in dict.fromkeys(HARVEST) if (REPO / "tests" / f).exists()]
    out = io.StringIO()
    with redirect_stdout(out):
        code = pytest.main(["-q", "-p", "no:cacheprovider", "--no-header", "--maxfail=1000", *files],
                           plugins=[Light()])
    print("harvest run:", code, out.getvalue().strip().splitlines()[-1:], file=sys.stderr)
    # unwrap everything again: the cases below call the code as it is
    provenance.prepare = _orig_prepare
    for module in (provenance, confirm, terminal, tls, payments, carried, urls):
        for name, value in list(vars(module).items()):
            if callable(value) and hasattr(value, "__wrapped__"):
                setattr(module, name, value.__wrapped__)


_PYTEST_TMP = __import__("re").compile(r"(?:/private)?/(?:var|tmp)/[^\s\"']*?/pytest-\d+/")


def harvested(name: str) -> list[dict[str, Any]]:
    """A function's harvested cases, each once, sorted; a test's own temporary folder written as /tmp/t/."""
    return [json.loads(c) for c in sorted({_PYTEST_TMP.sub("/tmp/t/", c) for c in HARVESTED.get(name, [])})]


# ---------- seams: the Mac's apps, as a call log ----------

RUNNING = [("com.apple.Terminal", "Terminal", 101), ("com.google.Chrome", "Google Chrome", 102),
           ("com.microsoft.VSCode", "Code", 103), ("net.whatsapp.WhatsApp", "\u200eWhatsApp", 104),
           ("com.apple.Safari", "Safari", 105), ("com.example.twin", "Twin", 106),
           ("com.example.twin2", "Twin", 107), ("com.example.noname", "", 108)]
INSTALLED = {"pages": "com.apple.iWork.Pages", "safari": "com.apple.Safari", "Preview": "com.apple.Preview"}
BROWSERS = {"com.google.chrome", "google chrome", "chrome", "safari", "com.apple.safari", "pid:102", "pid:105",
            "arc", "brave browser"}


@dataclasses.dataclass
class FakeRunning:
    bundle_id: str
    name: str
    pid: int
    active: bool = False


class Apps:
    """The fake running and installed apps, logging every question and its answer."""

    def __init__(self) -> None:
        self.log: list[list[Any]] = []

    def candidates(self, q: str) -> list[FakeRunning]:
        if "unreadable" in q:
            self.log.append(["candidates", q, "error"])
            raise RuntimeError("the running-app list can't be read")
        if q.startswith("pid:"):
            found = [FakeRunning(b, n, p) for b, n, p in RUNNING if q[4:].isdigit() and int(q[4:]) == p]
        else:
            c = mac_browsers.clean(q)
            found = [FakeRunning(b, n, p) for b, n, p in RUNNING if c in (b.casefold(), mac_browsers.clean(n))]
        self.log.append(["candidates", q, [[f.bundle_id, f.name] for f in found]])
        return found

    def find(self, q: str) -> FakeRunning | None:
        found = self.candidates(q)
        return found[0] if len(found) == 1 else None

    def installed_bundle(self, name: str) -> str | None:
        if "broken" in name:
            self.log.append(["installed", name, "error"])
            raise RuntimeError("Launch Services can't be asked")
        found = INSTALLED.get(name)
        self.log.append(["installed", name, found])
        return found

    def is_browser_app(self, app: str) -> bool:
        found = mac_browsers.clean(app) in BROWSERS
        self.log.append(["browser", app, found])
        return found


def with_apps(fn: Callable[[], Any]) -> tuple[dict[str, Any], list[Any]]:
    fake = Apps()
    saved = (mac_apps.candidates, mac_apps.find, mac_apps.installed_bundle, mac_browsers.is_browser_app)
    mac_apps.candidates, mac_apps.find, mac_apps.installed_bundle = fake.candidates, fake.find, fake.installed_bundle
    mac_browsers.is_browser_app = fake.is_browser_app
    try:
        return outcome(fn), fake.log
    finally:
        mac_apps.candidates, mac_apps.find, mac_apps.installed_bundle, mac_browsers.is_browser_app = saved


# ---------- 1. text: Python's own text handling the policy rests on ----------

def rng_text(r: random.Random, alphabet: str, n: int) -> str:
    return "".join(r.choice(alphabet) for _ in range(r.randint(0, n)))


UNICODE_SAMPLES = ["", "plain", "Straße", "ΣΑΣ", "aΣ b", "ǅ", "ﬁ", "Ｆｅｒｂ: go", "F.E.R.B: x", "**FERB:** go", "ferb's",
                   "e\u0301", "\u1100\u1161\u11a8", "한국어", "Ä\u0308", "\u0344", "ı", "İstanbul", "ǰ̣", "x\u0307\u0323",
                   "ff", "℃", "①②", "\u212b", "ｶﾞ", "a\u200db", "\u0345", "Ǆǅǆ", "ﬃ", "ß", "ᾳ", "Σσς", "ⅷ",
                   "٣٤", "١٢٣٤٥٦٧٨", "²", "\u2028x", "\ufeffF", "Ⓕerb", "ＦＥＲＢ：check", "Fe\u0301rb",
                   "\U0001d405erb", "𝐅𝐄𝐑𝐁: x", "mañana", "Ǳ", "ʼn", "ŉ", "\u03a3\u0301", "a\u0345", "\u1e9e", "ẞ"]


def text_cases() -> dict[str, Any]:
    r = random.Random(11)
    samples = list(UNICODE_SAMPLES)
    alphabet = "aAzZ09 .-_ßΣσİıſKﬁ①٣²é\u0301\u0308\u200d\u2028ｆＦ한\u1100\u1161\u11a8ǅ"
    samples += [rng_text(r, alphabet, 12) for _ in range(300)]
    unicode = [{"text": s, "lower": s.lower(), "upper": s.upper(), "casefold": s.casefold(),
                "capitalize": s.capitalize(), "nfc": unicodedata.normalize("NFC", s),
                "nfd": unicodedata.normalize("NFD", s), "nfkc": unicodedata.normalize("NFKC", s),
                "split": s.split(), "strip": s.strip(), "isdigit": s.isdigit(), "isalnum": s.isalnum(),
                "isprintable": s.isprintable()} for s in dict.fromkeys(samples)]
    hosts = ["example.com", "Bücher.example", "xn--bcher-kva.example", "ÉXAMPLE.com", "a\u200db.com", "ß.de",
             "faß.de", "ς.gr", "xn--zca.de", "-bad.com", "bad-.com", "ab--cd.com", "xn--ab-.com", "xn---bbk.com",
             "\u0301x.com", "例え.テスト", "مثال.إختبار", "a\u0660\u06f0.com", "l\u00b7l.cat", "\u00b7l.cat",
             "\u0375α.gr", "\u05f3א.il", "\u30fbカ.jp", "\u30fb.jp", "x" * 70 + "é.com", "é" * 1100, "a_b.com",
             "a b.com", "a%2Fb.com", "１２７.0.0.1", "ｅｘａｍｐｌｅ。ｃｏｍ", "\u2488.com", "\ufb01.com", "\u00ad.com",
             "xn--.com", "xn--a.com", "xn--ls8h.la", "\U0001f600.la", "xn--nxasmq6b.com", "βόλος.com",
             "\u200cx.com", "\u094d\u200cx.com", "क\u094d\u200cष.com", "a\u200cb.com", "ﺏ\u200cﺏ.com", "\U000e0100.com"]
    idna_cases = []
    for h in hosts:
        idna_cases.append({"input": h, "remap": outcome(lambda: idna.uts46_remap(h, std3_rules=False,
                                                                                transitional=False))})
        for label in h.split("."):
            idna_cases.append({"label": label, "alabel": outcome(lambda: idna.alabel(label).decode("ascii")),
                               "ulabel": outcome(lambda: idna.ulabel(label))})
    for raw in ["xn--bcher-kva", "XN--BCHER-KVA", "xn--abc", "xn--ab-", "xn--", "xn--zz-", "xn--99zt52a",
                "xn--a-ecp.ru", "xn--a", "xn--ls8h", "xn--55qx5d", "xn--0zwm56d", "xn--bcher-kv", "xn--x" + "9" * 30,
                "xn--ud8c", "xn--hxa", "xn--1ug", "xn--nxa1ugb"]:
        idna_cases.append({"label": raw, "alabel": outcome(lambda: idna.alabel(raw).decode("ascii")),
                           "ulabel": outcome(lambda: idna.ulabel(raw))})
    for word in ["bücher", "ü", "a-b-ü", "例え", "\U0001f600", "ab\u0301c", "αβγ", "mañana-xyz"]:
        idna_cases.append({"punycode": word, "encoded": word.encode("punycode").decode("ascii")})
    url_texts = [
        "https://example.com/a/b?c=d#e", "HTTP://User:Pa%20ss@EXAMPLE.com:8080/p;x=1/q.png?a=1&b=%2540#f/g&h=i",
        "https:attacker.example/x", "https://\\attacker.example\\p\\q?x=\\y#\\z", "https:///x.example",
        "file:///Users/me/a.txt", "file://host/share", "https://[::1]:443/x", "https://[fe80::1%25eth0]/",
        "https://[::ffff:1.2.3.4]/", "https://[1:0:0:2:0:0:0:3]/", "https://0x7f.1/", "https://017.0.0.1/",
        "https://2130706433/", "https://1.2.3.4.5/", "https://256.0.0.1/", "https://0x/", "https://1.2.3/",
        "https://4294967296/", "https://a.b.1/", "https://bücher.example/", "https://xn--bcher-kva.example/",
        "https://ex%41mple.com/", "https://ex%2fmple.com/", "https://exa mple.com/", "https://e\x7fx.com/",
        "https://example.com:99999/", "https://example.com:/x", "https://example.com:0x1/", "  https://a.com/\t\nb ",
        "\x00https://a.com", "ftp://a.com/x", "ws://a.com", "mailto:someone@example.com", "javascript:alert(1)",
        "chrome://extensions", "about:blank", "data:text/html,hi", "custom+x://Host:12/p?q#f", "x://[::1",
        "x://host:abc/", "x://host:70000/", "x://u@[v1.abc]/", "x://[1.2.3.4]/", "x://h\u2100/",
        "x://h\uff03/", "1http://a.com", "http//a.com", "", "https://", "https://@/", "https://u@h@k.com/",
        "https://例え.テスト/パス?クエリ#フラグ", "https://a.com/%E2%82%AC/%ZZ/%C3", "https://www.amazon.co.uk/dp/B0",
        "https://user@xn--nxasmq6b.com/", "HTTPS://WWW.Example.COM./", "https://a..b/", "https://.a.com/",
        "https://example.com:443", "https://xn--ls8h.la/", "http://127.1/", "http://0300.0250.0.1/",
        "http://192.168.0.1:8080/x", "https://[::]/", "https://[1::2::3]/", "https://[::1]x/"]
    r2 = random.Random(5)
    for _ in range(150):
        url_texts.append(rng_text(r2, "hts:/.ab@%2F1[]:?#=&;\\~é ", 30))
    url_cases = []
    for u in dict.fromkeys(url_texts):
        def split_parts(u=u):
            p = urlsplit(u)
            return {"parts": [p.scheme, p.netloc, p.path, p.query, p.fragment], "username": p.username,
                    "password": p.password, "hostname": p.hostname, "port": outcome(lambda: p.port),
                    "unsplit": urlunsplit(p._replace(fragment=""))}
        url_cases.append({"url": u, "parse": outcome(lambda: urls.parse(u)), "urlsplit": outcome(split_parts),
                          "quote": quote(u, safe=""), "quote_path": quote(u, safe=":/"), "quote_plus": quote_plus(u),
                          "unquote": unquote(u), "unquote_plus": unquote_plus(u),
                          "page_scope": confirm.page_scope(u), "site_key": confirm.site_key(u),
                          "registrable": outcome(lambda: confirm.registrable(u))})
    for h in harvested("urls.parse"):
        url_cases.append({"url": h["url"], "parse": outcome(lambda: urls.parse(h["url"]))})
    for t in ["%41%42%zz%4", "%C3%A9%E2%82", "a+b%2B", "é%41", "%25%32%35", "%f0%9f%98%80x%80"]:
        url_cases.append({"url": t, "unquote": unquote(t), "unquote_plus": unquote_plus(t)})
    ips = ["1.2.3.4", "01.2.3.4", "1.2.3", "256.1.1.1", "::1", "::ffff:1.2.3.4", "fe80::1%eth0", "1::2::3",
           "1:2:3:4:5:6:7:8", "1:2:3:4:5:6:7:8:9", ":1:2:3:4:5:6:7", "1::", "::", "12345::", "g::1", "::1.2.3.4",
           "1:0:0:2:0:0:0:3", "0:0:0:0:0:ffff:0102:0304", "", "1.2.3.4/8", "fe80::1%", "[::1]"]
    ip_cases = [{"text": t, "ip": outcome(lambda: str(ipaddress.ip_address(t)))} for t in ips]
    shlex_texts = ["curl -k https://a.example", "a 'b c' \"d e\" f\\ g", "x;y&&z||w|v&u", "a > b 2>&1 < c",
                   "echo \"a\\\"b\\\\c\\$d\\n\"", "unclosed 'quote", "trailing \\", "(a; b)", "a;\nb", "a\r\nb",
                   "''", "\"\"", "a''b", "x=1 y=\"2 3\" cmd", "a|&b", "a;;b", "a<<EOF", "é'ü'", "a\\\nb", "\\x",
                   "\"\\'\"", "'\\\"'"]
    r3 = random.Random(9)
    for _ in range(200):
        shlex_texts.append(rng_text(r3, "ab '\"\\;&|<>()\n\t=-x", 20))

    def lex(t: str) -> list[str]:
        lx = shlex.shlex(t, posix=True, punctuation_chars=";&|<>()\n")
        lx.whitespace, lx.whitespace_split, lx.commenters = " \t\r", True, ""
        return list(lx)
    shlex_cases = [{"text": t, "tokens": outcome(lambda: lex(t))} for t in dict.fromkeys(shlex_texts)]
    # the patterns written by hand in Rust, against Python's own on random text
    r4 = random.Random(3)
    alpha = "0123456789  ab+-./@%:,_\u0663²é+\n"
    corpus = [rng_text(r4, alpha, 40) for _ in range(800)] + [
        "+44 7700 900123", "07 Nov 2026 13:27", "4.5 7", "account 12345678 3 days", "OTP 482193 5",
        "07700900123 07700900456", "DE00 1234 5678 9012", "+1 202 555 0143.", "12 34_", "12 34.5", "12 34:x",
        "x12 34", "12 34 56 78 90 12 34", "12  34", "+91 98765 43210;"]
    regex = []
    for s in dict.fromkeys(corpus):
        regex.append({"text": s, "run": [list(m.span(1)) for m in confirm._RUN.finditer(s)],
                      "grouped_digits": [list(m.span()) for m in confirm._GROUPED_DIGITS.finditer(s)],
                      "joined": carried._JOINED.sub("", s), "fine": confirm._FINE.split(s)})
    r5 = random.Random(4)
    urlish = "abcz09+.-:/@ws\"'<>()[]{}|\\^`.\nx"
    for _ in range(600):
        s = rng_text(r5, urlish, 40)
        regex.append({"text": s, "url_in_text": [list(m.span()) for m in confirm._URL_IN_TEXT.finditer(s)],
                      "host_path": [list(m.span()) for m in confirm._HOST_PATH_IN_TEXT.finditer(s)]})
    for s in ["see www.macworld.com/article/666646/x and https://a.b/c", "a.b.com/x", "x.co/", "@x.com/a",
              "-x.com/a", "a.c0m/x", "a.bc/", "e.g. 1.2/3", "http://x.com/a www.y.org/b", "ab://", "b2b://x"]:
        regex.append({"text": s, "url_in_text": [list(m.span()) for m in confirm._URL_IN_TEXT.finditer(s)],
                      "host_path": [list(m.span()) for m in confirm._HOST_PATH_IN_TEXT.finditer(s)]})
    b64 = []
    for p in ["aGVsbG8gd29ybGQ", "aGVsbG8gd29ybGQ=", "aGVsbG8_d29y-GQ", "YWJjZGVmZ2g", "YWJjZGVmZw==", "YWJjZGVmZ2==",
              "abcdefghi", "abcdefghij", "abcdefghijk", "Zm9vYmFyYmF6", "6162636465666768", "e282ac6162636465",
              "ffffffffffffffff", "dXNlcjpwYXNzd29yZA", "a2V5PXZhbHVlJm90aGVyPTEyMzQ1Ng"]:
        b64.append({"piece": p, "decodings": sorted(confirm._decodings(p))})
    decimals = [{"shown": s, "key": outcome(lambda: payments.amount_key(s))} for s in [
        "₹248.75", "INR 248.75", "₹2,48.75", "248.750", "Rs. 1,200", "1200 USD", "$0.00", "007", "€ 12", "12 €",
        "us$5", "A$ 3.5", "aed 9", "USD", "1" * 40, "9" * 29, "0.000000000000000000000000000001", "1.5e3",
        "١٢٣.٤٥", "Total: ₹ 1 299", "12,34,567.80 INR", "Rs 99", "rs.99", "¥1000", "£ 0.50", "", None, "x",
        "5" * 28 + "5", "1" + "0" * 27 + "5", "2" + "0" * 27 + "5", "1" + "0" * 27 + "51", "9" * 28 + "5"]]
    return {"unicode": unicode, "idna": idna_cases, "urls": url_cases, "ips": ip_cases, "shlex": shlex_cases,
            "regex": regex, "decodings": b64, "amount_keys": decimals}


# ---------- 2. provenance ----------

def prep_out(t: Any) -> dict[str, Any]:
    return {"folded": t.folded, "tokens": sorted(t.tokens), "paths": sorted(t.paths), "hosts": sorted(t.hosts),
            "amounts": list(t.amounts), "phones": sorted(t.phones),
            "words": [[w.folded, w.capital, w.sentence_start, w.tight] for w in t.words]}


PROV_TEXTS = ["Send it to rahul@example.com and +91 98765 43210 today.", "message Rahul", "message Rahul Verma",
              "message neha jainson", "Pay ₹248.75 or $1,200.50 now", "see https://Bücher.example/x and <a "
              "href=\"mailto:me@x.org\">me@x.org</a>", "path ~/Documents/a.txt, /Users/me/b/", "",
              "राहुल को भेजो", "On 2026-10-12 call 12345678, code 123456", "\"(hello)\" [world]!", "a\nb\r\nc",
              "Mr. Rahul Verma. Neha said hi!", "x href=\"https://evil.example/p?q=1\">", "1,234 and 12.5 and ₹3",
              "Call +1 (202) 555-0143 or 202.555.0143", "first-name Rahul,Verma", "ΣΑΣ Σ", "＄12 ９９"]
DEST_TEXTS = ["a@b.co", " a@b.co ", "a@b", "https://x.example/p", "x.example", "+91 98765 43210", "1234 5678",
              "1234567", "2026-10-10", "10.10.2026", "123456", "12-34-56", "<a@x.com>", "a@x.com,", "+91 98765 43210;",
              "(202) 555-0143", "Rahul", "ftp://h/x", "javascript:x", "+١٢٣٤٥٦٧٨٩", "a b@c.d"]


def provenance_cases() -> dict[str, Any]:
    out: dict[str, Any] = {"prepare": [], "shapes": [], "sources": [], "arg_sources": [], "from_words": []}
    texts = list(PROV_TEXTS) + [c["owner"] for c in harvested("provenance.sources")][:200]
    for t in dict.fromkeys(texts):
        out["prepare"].append({"text": t, "out": prep_out(provenance.prepare(t)),
                               "embedded": provenance.embedded(t), "phone_runs": provenance.phone_runs(t),
                               "raw_tokens": provenance.raw_tokens(t)})
        out["from_words"].append({"text": t, "out": norm(destinations.KnownDestinations.from_words(t))})
    for v in DEST_TEXTS:
        out["shapes"].append({"value": v, "shape": provenance.destination_shape(v),
                              "typed": provenance.typed_destination(v), "host_of": provenance.host_of(v),
                              "destination_of": list(destinations.destination_of(v)),
                              "normalize": {k: destinations.KnownDestinations.normalize(k, v)
                                            for k in ("email", "phone", "host", "name")}})
    gen = [{"value": "Rahul", "owner": "message Rahul Verma about it", "external": [], "aliases": [], "kind": "name",
            "answer": ""},
           {"value": "Rahul Verma", "owner": "message Rahul", "external": [["external:web:a.com", "Rahul Verma"]],
            "aliases": [], "kind": "name", "answer": ""},
           {"value": "Rahul Attacker", "owner": "message Rahul Verma", "external": [], "aliases": [], "kind": "name",
            "answer": ""},
           {"value": "Neha Jain", "owner": "message neha jainson", "external": [], "aliases": [], "kind": "name",
            "answer": "neha"},
           {"value": "+91 98765 43210", "owner": "call 9876543210", "external": [["external:email", "+919876543210"]],
            "aliases": [], "kind": "name", "answer": ""},
           {"value": "248.75", "owner": "pay ₹248.75", "external": [], "aliases": [], "kind": "amount", "answer": ""},
           {"value": "/Users/me/Documents/a.txt", "owner": "open ~/Documents/a.txt", "external": [],
            "aliases": ["~/Documents/a.txt"], "kind": "structured", "answer": ""},
           {"value": "bücher.example", "owner": "go to https://bücher.example/", "external": [], "aliases": [],
            "kind": "structured", "answer": ""},
           {"value": "xn--bcher-kva.example", "owner": "go to https://bücher.example/", "external": [],
            "aliases": [], "kind": "structured", "answer": ""},
           {"value": "ab", "owner": "ab", "external": [], "aliases": [], "kind": "name", "answer": ""},
           {"value": "राहुल", "owner": "राहुल को भेजो", "external": [], "aliases": [], "kind": "name", "answer": ""},
           {"value": "Rahul Verma", "owner": "Hi Rahul Verma.", "external": [["external:web:z.com", "x"],
                                                                             ["external:email", "Rahul Verma"]],
            "aliases": [], "kind": "name", "answer": "Rahul Verma"}]
    for c in harvested("provenance.sources") + gen:
        out["sources"].append({**c, "out": provenance.sources(c["value"], c["owner"], [tuple(x) for x in c["external"]],
                                                              tuple(c["aliases"]), c["kind"], c["answer"])})
    home = str(Path.home())
    targets_gen = [Target(account="gmail:me@x.com", site="mail.google.com", recipients=("a@b.co", "Rahul Verma"),
                          path=home + "/Documents/a.txt", amount=248.75, contact="+91 98765 43210"),
                   Target(recipients=("rahul@example.com",), amount=1e16), Target(site="x.example", amount=12.0)]
    for c in harvested("provenance.arg_sources") + [
            {"target": ser_target(t), "owner": o, "external": [["external:web:a.com", "rahul@example.com 248.75"]],
             "answer": "Rahul Verma"} for t in targets_gen for o in ("send to Rahul at a@b.co", "")]:
        t = de_target(c["target"])
        out["arg_sources"].append({**c, "home": home.replace(str(TMP), "<TMP>"),
                                   "out": norm(provenance.arg_sources(t, c["owner"], [tuple(x) for x in c["external"]],
                                                                      c["answer"]))})
    return out


# ---------- 3. confirm: shapes and words ----------

LABELS = ["external:web:mail.google.com", "external:web:excluded", "external:web:trail", "external:web",
          "external:paste", "external:", "external:unlabeled", "external:download", "external:memory",
          "external:email", "external:message:wa:123", "external:message:com.apple.MobileSMS",
          "external:message", "external:calendar:x", "private", "owner", "external:web:www.Example.com"]


def confirm_pure_cases() -> dict[str, Any]:
    r = random.Random(21)
    out: dict[str, Any] = {}
    out["labels"] = [{"label": l, "origin": confirm.shown_origin(l), "place": confirm.shown_place(l)} for l in LABELS]
    out["label_sets"] = []
    for _ in range(80):
        ls = r.sample(LABELS, r.randint(0, 7))
        out["label_sets"].append({"labels": ls, "origins": confirm.shown_origins(ls), "places": confirm.shown_places(ls)})
    words = ["", "FERB: go", "ferb", "Ｆｅｒｂ", "F.E.R.B: x", "**FERB:** check", "FERB's record says", "FERBx",
             "hello\nFERB: yes\nno", "  ferb:", ":FERB:", "Fe\u0301rb", "a\nb", "FERB", "\u2028FERB: x"]
    out["ferb_words"] = [{"text": w, "reads": confirm.reads_as_ferbs(w), "check": confirm.reads_as_ferbs_check(w),
                          "from_task": confirm.from_task(w), "task_question": confirm.task_question(w),
                          "from_task_lines": confirm.from_task_lines(w), "reply_lines": confirm.reply_lines(w),
                          "reason_line": confirm.reason_line(w)} for w in words]
    values = ["rahul@example.com", "\"a@b.co\"", "+91 98765 43210", "12345", "1234567", "hello", "1990", "2099",
              "2100", "١٩٩٠", "2026-11-07", "2026/13/07", "07-11-2026", "31.12.2026", "12/31/2026", "13/13/2026",
              "2026-1-5", "1-5-2026", "+44 7700 900123", "ORD-48213-7731", "4,312.75", "s6e35", "DE00 1234 5678",
              "owner-sample/stealth-launch", "bluefalcon!", "index.html", "about-us", "naïve", "e\u0301clair"]
    out["values"] = [{"value": v, "kind": confirm.value_kind(v), "year": confirm.is_year(v), "date": confirm.is_date(v),
                      "is_value": confirm._is_value(v), "spellings": sorted(confirm.spellings(v)),
                      "spellings_plain": sorted(confirm._spellings(v)),
                      "whole_values": confirm.whole_values(v), "account_key": confirm.account_key(v),
                      "site_key": confirm.site_key(v), "browser_page": confirm.browser_page(v),
                      "shown_quoted": confirm.shown_quoted(v), "contacts_in": sorted(confirm.contacts_in(v)),
                      "value_forms": sorted(confirm._value_forms(v.lower()))} for v in values]
    texts = ["My phone is +44 7700 900123 and card DE00 1234 5678 9012; order ORD-48213-7731 on 07 Nov 2026 13:27.",
             "account 12345678 3 days; OTP 482193 5; 07700900123 07700900456", "see https://docs.example.com/d/1aB3x/edit"
             " and www.macworld.com/article/666646/x", "owner-sample/stealth-launch bluefalcon! about-us index.html",
             "22 Birch Road Apt 5B, Riverton 99021", "+1 202 555 0143. call", "x" * 300, "a" + " 12" * 40]
    out["whole_values"] = [{"text": t, "out": confirm.whole_values(t)} for t in texts] + [
        {"text": c["text"], "out": confirm.whole_values(c["text"])} for c in harvested("confirm.whole_values")]
    hosts = ["www.amazon.in", "pay.amazon.in", "www.amazon.co.uk", "a.b.c.co.uk", "co.uk", "1.2.3.4", "[::1]", "x.",
             "", "localhost", "WWW.Example.COM.", "a.com.au", "xn--bcher-kva.example"]
    out["registrable"] = [{"host": h, "out": outcome(lambda: confirm.registrable(h))} for h in hosts]
    out["page_scope"] = [{"url": u, "out": confirm.page_scope(u)} for u in [
        "https://docs.google.com/document/d/1aB3x/edit", "www.scaler.com/academy/mentee-dashboard",
        "https://x.com/user/status/123/photo", "x.com", "https://a.com/%41b/c%2Fd", "not a url", "https://a.com/",
        "https://a.com/a%20b/9"]] + [{"url": c["text"], "out": confirm.page_scope(c["text"])}
                                     for c in harvested("confirm.page_scope")]
    owns = [("me@x.com", ["me@x.com"]), ("ME@x.com ", ["me@x.com"]), ("+91 98765 43210", ["919876543210"]),
            ("098765 43210", ["919876543210"]), ("98765", ["98765"]), ("phone 9876543210", ["9876543210"]),
            ("1234567", ["1234567"]), ("12345678901", ["9912345678901"]), ("x", [])]
    out["own_contact"] = [{"value": v, "own": o, "out": confirm.own_contact(v, o)} for v, o in owns] + [
        {**c, "out": confirm.own_contact(c["value"], c["own"])} for c in harvested("confirm.own_contact")]
    amounts = [None, 248.75, 248.0, 0.0, -0.0, 1e16, 1e22, 0.125, 0.135, 2.675, 1.005, 12.5, -3.0, 0.1]
    out["amounts"] = [{"amount": a, "currency": c, "out": confirm.shown_amount(a, c)}
                      for a in amounts for c in (None, "₹", "INR", "  US  $ ", "LONGCURRENCYCODE")]
    out["samples"] = [{"items": i, "out": confirm.sample(i)} for i in ([], ["a"], ["a", "b", "c"], ["a", "b", "c", "d"],
                                                                       ["é\"x", "y"])]
    out["keys"] = [{"parts": p, "out": confirm.key(*p)} for p in (["r", None, 1.5, ["a", "b"]], ["é", 2.0, True],
                                                                  ["x", [["a", "b"]], {"k": 1}], [1e16, 0.1])]
    out["routine"] = [{"rule": r_, "values": v, "out": confirm.routine(confirm.Need(r_, "k", v))}
                      for r_ in (confirm.FOREGROUND, confirm.BROWSER_PAGE, "floor:new_recipient", confirm.PAIR,
                                 "floor:pay", "floor:account_security", "floor:delete_permanent",
                                 confirm.TERMINAL_INPUT, confirm.TLS_OFF, "sink:new_destination",
                                 "sink:outside_value", "sink:private_url", confirm.BULK, "escape:read:~/x",
                                 "escape:write:~/x", "lift:read:x")
                      for v in ({}, {"lift": "read"}, {"lift": "write"})]
    out["pages"] = [{"address": a, "out": confirm.browser_page(a),
                     "need": ser_need(confirm.browser_page_need("Chrome", a or "", "Type"))}
                    for a in ["chrome://extensions", " CHROME://settings ", "about:blank", "about:srcdoc", "about:config",
                              "https://x", "chrome-extension://abc/p", "Brave://x", None, "", "moz-extension://x"]]
    out["pairs"] = [{"args": a, "need": ser_need(confirm.pair_need(*a))} for a in
                    (["Work", "chrome", "Profile 1", ""], ["", "brave", "Default", "me@x.com"], ["X", "ǆx", "f", ""])]
    out["verbs"] = [{"tool": t, "args": a, "out": confirm.input_verb(t, a)} for t, a in
                    [("mac.applescript", {}), ("mac.act", {"action": "type"}), ("mac.act", {"action": "press"}),
                     ("mac.click", {"action": "type"}), ("mac.act", {})]]
    facts_list = [{"path": "/Users/me/old/", "paths": ["/Users/me/old/", "/Users/me/old/a.txt", "/Users/me/old/b/",
                                                       "/elsewhere/c", "/Users/me/old/d", "/Users/me/old/e"],
                   "files": 4, "directories": 1, "bytes": 1234},
                  {"files": "1", "directories": 0, "bytes": 1.9, "paths": []},
                  {"path": "", "files": 2, "directories": 2, "bytes": 0, "paths": ["x", "é\"y"]},
                  {"files": 1, "bytes": 1}, {"files": "x", "directories": 1, "bytes": 1}]
    out["bulk_delete"] = [{"path": "/Users/me/old", "facts": f,
                           "out": outcome(lambda: confirm.bulk_delete("/Users/me/old", f), ser_need)}
                          for f in facts_list]
    out["constants"] = {
        "ALLOW_WINDOW_S": confirm.ALLOW_WINDOW_S, "ROUTINE": sorted(confirm.ROUTINE), "PAY_WINDOW_S": payments.PAY_WINDOW_S,
        "SHOWN": confirm.SHOWN, "QUOTED_CHARS": confirm.QUOTED_CHARS, "SEND_CLASS": sorted(confirm.SEND_CLASS),
        "ALWAYS": norm(confirm.ALWAYS), "UNREAD_AMOUNT": confirm.UNREAD_AMOUNT, "UNREAD": payments.UNREAD,
        "ACCOUNT_SECURITY": confirm.ACCOUNT_SECURITY, "TLS_QUESTION": confirm.TLS_QUESTION,
        "BROWSER_SCHEMES": list(confirm.BROWSER_SCHEMES), "FOREGROUND_TOOLS": sorted(confirm.FOREGROUND_TOOLS),
        "TYPING_ACTIONS": sorted(confirm.TYPING_ACTIONS), "GENERIC_INPUT": sorted(rules.GENERIC_INPUT),
        "REVERSIBLE": sorted(rules.REVERSIBLE), "READING_KINDS": sorted(rules.READING_KINDS),
        "ALWAYS_DECLARED": sorted(rules.ALWAYS_DECLARED), "EFFECTS": list(rules.EFFECTS),
        "ANNOTATIONS": list(rules.ANNOTATIONS), "EFFECT_KINDS": rules.EFFECT_KINDS,
        "TERMINAL_APPS": list(protection.DEFAULT_TERMINAL_APPS), "NEVER_WRITE": list(protection.DEFAULT_NEVER_WRITE),
        "EXTRA_NEVER_READ": list(protection.EXTRA_NEVER_READ), "SERVICE_LABELS": list(protection.SERVICE_LABELS),
        "BUNDLE_SERVICE": str(protection.BUNDLE_SERVICE), "NEVER_LIFTED": sorted(protection.NEVER_LIFTED),
        "CODE_RUNNERS": terminal.CODE_RUNNERS, "OPENS_FILES": sorted(terminal.OPENS_FILES),
        "NO_FACTS_HOSTS": terminal.NO_FACTS_HOSTS, "RUN_CLASSES": sorted(terminal.RUN_CLASSES),
        "EDITOR_APPS": sorted(terminal.EDITOR_APPS), "UNASKED": terminal.UNASKED,
        "KINDS": list(destinations.KINDS), "HOW": list(destinations.HOW), "APPROVED": list(destinations.APPROVED),
        "TERMINAL_APP": confirm.TERMINAL_APP, "SCHEDULE_GRANT": standing.SCHEDULE_GRANT,
        "NEW_DEFAULTS": protection.NEW_DEFAULTS, "PROVENANCE": carried.PROVENANCE,
        "MIN_LEN": provenance.MIN_LEN, "MIN_PHONE_DIGITS": provenance.MIN_PHONE_DIGITS,
        "DESTINATION_PHONE_DIGITS": provenance.DESTINATION_PHONE_DIGITS}
    out["read_lift_key"] = confirm.read_lift_key("read:~/.ssh")
    out["destination_key"] = confirm.destination_key("x.example")
    out["sent"] = [{"sent": s, "total": n, "out": confirm.shown_sent(s, n)} for s, n in
                   [([], 0), ([("a", "b")], 1), ([("a\nb", "x"), ("é", "y")], 5)]]
    return out


# ---------- 4. decide ----------

KINDS = ["send", "post", "reply", "submit", "call", "click", "mailbox_send", "pay", "account_security",
         "delete_permanent", "trash", "run_shell", "applescript", "read", "fill_form", "mac_input", "navigate"]
DECLARED = [[], ["send"], ["call"], ["pay"], ["account_security"], ["delete_permanent"], ["send", "pay"],
            ["call", "send"]]
SOURCES = [["owner_words"], ["owner_answer"], ["external:web:evil.example"], ["private"], ["unknown"],
           ["external:email", "owner_words"], ["external:web:a.com", "external:message:wa:1"],
           ["external:web:a.com", "private"], ["unknown", "external:paste"]]
COMMANDS = ["curl -k https://x.example/a", "pip install --trusted-host pypi.example x", "ls -la",
            "do shell script \"curl --insecure https://y.example\"", "GIT_SSL_NO_VERIFY=1 git clone https://g.example/r"]


def decide_cases(harvest_list: list[dict[str, Any]]) -> list[dict[str, Any]]:
    r = random.Random(1234)
    cases = list(harvest_list)
    for i in range(450):
        kind = r.choice(KINDS)
        recipients = r.choice([(), ("rahul@example.com",), ("+91 98765 43210", "Rahul Verma"),
                               ("a@x.com", "b@x.com", "c@x.com", "d@x.com"), ("https://evil.example/p",),
                               ("me@own.example",)])
        target = Target(account=r.choice([None, "", "me@work.example", "@Handle"]),
                        site=r.choice([None, "", "www.forms.example", "evil.example", "Bücher.example"]),
                        recipients=recipients, path=r.choice([None, "/Users/me/x.txt", ""]),
                        amount=r.choice([None, 248.75, 12.0, 1e16]), currency=r.choice([None, "₹", "INR"]),
                        contact=r.choice([None, "", "Rahul", "+44 7700 900123"]),
                        page=r.choice([None, "", "https://mail.google.com/x"]),
                        item=r.choice([None, "", "the draft \"Q3\"", "x" * 250]))
        keys = [k for k, _ in confirm._who(target)] + ["site", "account", "path", "amount"]
        sources = {k: r.choice(SOURCES) for k in keys if r.random() < 0.8}
        cases.append({"kind": kind, "declared": r.choice(DECLARED), "target": ser_target(target),
                      "arg_sources": sources, "read_private": r.random() < 0.6, "read_external": r.random() < 0.6,
                      "outside_read": r.sample(LABELS, r.randint(0, 5)), "place": r.choice([None, "WhatsApp", ""]),
                      "content": r.choice([None, "", "hello there", *COMMANDS]),
                      "known": r.sample([["email", "rahul@example.com"], ["host", "evil.example"],
                                         ["phone", "+91 98765 43210"], ["name", "Rahul Verma"],
                                         ["host", "www.forms.example"], ["email", "a@x.com"]], r.randint(0, 3)),
                      "approved": r.sample([["host", "evil.example"], ["host", "www.forms.example"]], r.randint(0, 1)),
                      "own": r.choice([[], ["me@own.example"], ["919876543210", "me@own.example"]])})
    out = []
    for c in cases:
        known_set = {tuple(x) for x in c.get("known", [])}
        approved_set = {tuple(x) for x in c.get("approved", [])}
        log: dict[str, list[Any]] = {"known": [], "approved": [], "own": []}

        def known(kind: str, value: str) -> bool:
            ans = (kind, value) in known_set
            log["known"].append([kind, value, ans])
            return ans

        def approved(kind: str, value: str) -> bool:
            ans = (kind, value) in approved_set
            log["approved"].append([kind, value, ans])
            return ans

        def own() -> list[str]:
            log["own"].append(c.get("own", []))
            return list(c.get("own", []))
        res = outcome(lambda: confirm.decide(
            c["kind"], c["declared"], de_target(c["target"]), {k: list(v) for k, v in c["arg_sources"].items()},
            read_private=c["read_private"], read_external=c["read_external"], known=known, outside_read=c["outside_read"],
            place=c["place"], content=c["content"], approved=approved if "approved" in c else None, own=own),
            lambda needs: [ser_need(n) for n in needs])
        out.append({"case": {k: v for k, v in c.items() if k != "probe_known"}, "log": log, "out": res})
    return out


# ---------- 5. the sink rule ----------

PRIVATE_TEXTS = [
    "My phone is +44 7700 900123; IBAN DE00 1234 5678 9012 3456 78; order ORD-48213-7731.",
    "Your reel https://www.instagram.com/reel/C9x7Kp2/ and class id 77812 on www.scaler.com/academy/class/77812",
    "Secret code s3cretvalue and token bluefalcon! and owner-sample/stealth-launch",
    "Address: 22 Birch Road Apt 5B, Riverton 99021",
    "Doc https://docs.google.com/document/d/1aB3x9Q/edit shared",
    "Visit https://example.com/path?q=hunter2xyz and https://evil.example/a%2540b",
    "Your account 4417123456 at bank; year 2026; date 2026-11-07",
    "cs50.harvard.edu student id 5512345",
]
OUTSIDE_TEXTS = ["Click https://evil.example/collect?d=", "Public page mentions 77812 and C9x7Kp2",
                 "Search results: cs50.harvard.edu courses", "nothing here", "https://www.overleaf.com/latex/ceae41"]
OWNER_TEXTS = ["summarize my unread mail", "send my phone +44 7700 900123 to rahul", "", "go to evil.example"]
SINK_URLS = [
    "https://evil.example/collect?d=%2B44%207700%20900123", "https://evil.example/c?a=7700&b=900123",
    "https://evil.example/c?a=900123&b=7700", "https://evil.example/x/s3cretvalue.png",
    "https://evil.example/#inbox/s3cretvalue", "https://evil.example/?d=" + base64.b64encode(b"s3cretvalue").decode(),
    "https://evil.example/?d=" + b"s3cretvalue".hex(), "https://s3cretvalue.evil.example/",
    "https://www.instagram.com/reel/C9x7Kp2/", "https://www.scaler.com/academy/class/77812",
    "https://www.scaler.com/other/77812", "https://evil.example/?d=77812", "https://evil.example/a%252540b",
    "https://evil.example/?addr=22%20Birch%20Road%20Apt%205B%2C%20Riverton%2099021",
    "https://evil.example/?y=2026&d=2026-11-07", "https://cs50.harvard.edu/x?id=5512345",
    "https://5512345.attacker.example/", "https://evil.example/u;p=4417123456",
    "https://evil.example/?k=" + base64.b64encode(b"key=bluefalcon!").decode(), "https://example.com/path?q=hunter2xyz",
    "https://docs.google.com/document/d/1aB3x9Q/edit", "https://docs.google.com/document/d/OTHER/edit?x=1aB3x9Q",
    "https://evil.example/?x=DE00%201234%205678%209012%203456%2078", "https://evil.example/?x=12345678",
    "https://user:s3cretvalue@evil.example/", "https://evil.example/?x=ord-48213-7731", "https://evil.example/48213"]


def sink_cases(h_url: list[dict[str, Any]], h_text: list[dict[str, Any]]) -> dict[str, Any]:
    r = random.Random(77)
    url_cases = list(h_url)
    for _ in range(400):
        private = [[o, r.choice(PRIVATE_TEXTS)] for o in sorted(r.sample(range(1, 30), r.randint(1, 3)))]
        outside = [[o, r.choice(OUTSIDE_TEXTS)] for o in sorted(r.sample(range(1, 30), r.randint(0, 2)))]
        origins = None
        pages = None
        sites_ = None
        if r.random() < 0.5:
            origins = {str(o): r.choice([["private"], ["private", "external:web:www.scaler.com"],
                                         ["private", "external:web:docs.google.com"], ["private", "external:email"],
                                         ["private", "external:web:www.instagram.com"]]) for o, _ in private}
            pages = {str(o): r.choice(["https://www.scaler.com/academy/class/77812",
                                       "https://docs.google.com/document/d/1aB3x9Q/edit", ""]) for o, _ in private}
        if r.random() < 0.3:
            sites_ = {str(i): r.choice([["www.scaler.com/academy/class/77812"], ["docs.google.com/document/d/1ab3x9q"],
                                        []]) for i in range(len(private))}
        url_cases.append({"url": r.choice(SINK_URLS), "private": private, "outside": outside,
                          "owner": r.choice(OWNER_TEXTS), "origins": origins, "sites": sites_, "pages": pages})
    out_urls = []
    for c in url_cases:
        def run(c=c) -> Any:
            private = [(o, provenance.prepare(t)) for o, t in c["private"]]
            outside = [(o, provenance.prepare(t)) for o, t in c["outside"]]
            sites_ = None if c["sites"] is None else {id(private[int(i)][1]): frozenset(v)
                                                      for i, v in c["sites"].items()}
            hits = confirm.url_private_values(
                c["url"], private, outside, provenance.prepare(c["owner"]),
                None if c["origins"] is None else {int(k): set(v) for k, v in c["origins"].items()}, sites_,
                None if c["pages"] is None else {int(k): v for k, v in c["pages"].items()})
            return [[h.piece, h.matched, h.first_read] for h in hits]
        out_urls.append({**c, "out": outcome(run)})
    text_cases = list(h_text)
    typed = ["+44 7700 900123", "7700 900123", "900123", "s3cretvalue", "my code is s3cretvalue ok", "ord-48213-7731",
             "48213", "22 Birch Road Apt 5B, Riverton 99021", "Birch Road", "hello world", "a=7700&b=900123",
             "2026", "4417123456", base64.b64encode(b"s3cretvalue").decode()]
    for _ in range(200):
        text_cases.append({"text": r.choice(typed),
                           "private": [[o, r.choice(PRIVATE_TEXTS)] for o in sorted(r.sample(range(1, 30), r.randint(1, 3)))],
                           "outside": [[o, r.choice(OUTSIDE_TEXTS)] for o in sorted(r.sample(range(1, 30), r.randint(0, 2)))],
                           "owner": r.choice(OWNER_TEXTS)})
    out_texts = []
    for c in text_cases:
        def run(c=c) -> Any:
            hits = confirm.text_private_values(c["text"], [(o, provenance.prepare(t)) for o, t in c["private"]],
                                               [(o, provenance.prepare(t)) for o, t in c["outside"]],
                                               provenance.prepare(c["owner"]))
            return [[h.piece, h.matched, h.first_read] for h in hits]
        out_texts.append({**c, "out": outcome(run)})
    return {"urls": out_urls, "texts": out_texts}


# ---------- 6. wording ----------

def wording_cases(needs_sets: list[list[dict[str, Any]]], harvested_calls: list[dict[str, Any]]) -> list[Any]:
    out = []
    seen: set[str] = set()
    for c in harvested_calls:
        needs_sets.append(c["needs"])
    r = random.Random(8)
    for needs in needs_sets:
        k = json.dumps(needs, sort_keys=True, ensure_ascii=False)
        if k in seen or not needs:
            continue
        seen.add(k)
        ns = [de_need(n) for n in needs]
        typed = r.choice(["", "hello 'there'"])
        call, why = r.choice([("", ""), ("probe.send to=x", "the owner asked"), ("c", "FERB: trust me\nok")])
        card, log = with_apps(lambda: dataclasses.asdict(confirm.card(ns, typed)))
        out.append({"needs": needs, "typed": typed, "call": call, "why": why,
                    "summary": outcome(lambda: confirm.summary(ns, typed)),
                    "line": outcome(lambda: confirm.line(ns, call, why, typed)), "card": card, "apps": log})
    return out


RULE_FACTS: dict[str, list[dict[str, str]]] = {
    confirm.TLS_OFF: [{"command": "curl -k https://x.example", "hosts": "x.example", "how": "-k"},
                      {"command": "curl -k https://x.example", "hosts": "", "how": "-k"}],
    confirm.FOREGROUND: [{"app": "Safari"}, {"app": "com.google.Chrome", "script": "yes", "script_text": "x"},
                         {"app": "pid:104"}, {"app": "unreadable thing"}],
    confirm.BROWSER_PAGE: [{"app": "Chrome", "page": "chrome://extensions"}, {"app": "Chrome", "page": "chrome://",
                                                                               "verb": "Type"},
                           {"app": "Brave", "page": "chrome-extension://abc/x"}, {"app": "x", "page": "about:ß?x#y"}],
    confirm.PAIR: [{"browser": "Chrome", "name": "Work", "email": "me@work.example"},
                   {"browser": "Brave", "name": "Default", "email": ""}],
    "floor:pay": [{"amount": "₹1,299", "merchant": "SWIGGY", "site": "swiggy.com", "account": "HDFC ••••12",
                   "recipients": ""},
                  {"amount": payments.UNREAD, "merchant": "", "site": "", "account": "", "recipients": "a@b.co",
                   "declared": "₹500", "unseen": "1"},
                  {"amount": "₹1,299", "merchant": "", "site": "x", "account": "", "recipients": "",
                   "approved": "₹1,200 to SWIGGY", "bank": "bank.example"},
                  {"amount": "", "declared": "₹500"}],
    "floor:account_security": [{"site": "bank.example", "account": "me@bank.example"}, {"site": "", "account": ""}],
    "floor:delete_permanent": [{"path": "/Users/k/Downloads/old.zip"}, {"path": "\"the draft\"", "place": "Terminal"}],
    "floor:new_recipient": [{"recipients": "rahul@example.com", "verb": "sends to"},
                            {"recipients": "a, b", "verb": "calls", "place": "WhatsApp", "account": "me"},
                            {"recipients": "", "verb": "sends to", "place": "WhatsApp", "account": "@me"},
                            {"recipients": "", "verb": "calls", "place": "", "account": ""}],
    "sink:new_destination": [{"destination": "forms.example.com", "read": "a page on mail.google.com"},
                             {"destination": "f.example", "form": "yes", "sent": "\"x\" (from y)", "control": "Submit",
                              "values": "a\nb", "sources_plain": "your mail", "found_via": "by searching",
                              "more": "2", "read": "z"},
                             {"destination": "f.example", "form": "yes", "value": "\"v\""}],
    "sink:private_url": [{"destination": "www.overleaf.com", "value": "ceae41", "matched": "ceae41",
                          "source": "the result of browser.snapshot on mail.google.com", "read": "mail.google.com",
                          "source_plain": "your signed-in mail.google.com page",
                          "url": "https://www.overleaf.com/latex/templates/ceae41"},
                         {"destination": "evil.example", "value": "a%40b", "matched": "a@b"}],
    "sink:outside_value": [{"what": "typed", "value": "4417 1234 5678", "origin": "a page on example.com",
                            "places": "a example.com page", "site": "x.com"},
                           {"what": "recipient", "value": "attacker@evil.example", "origin": "an email",
                            "places": "an email", "site": ""},
                           {"what": "recipient", "value": "x" * 45 + "@e.com", "origin": "o", "places": "",
                            "site": ""},
                           {"what": "amount", "value": "₹12", "origin": "o", "places": "p", "site": ""}],
    confirm.BULK: [{"what": "send", "count": "3", "bytes": "2 bytes", "sample": '"a", "b", "c"'},
                   {"what": "call", "count": "4", "bytes": "0 bytes", "sample": '"a"'},
                   {"what": "delete", "path": "/Users/me/old/", "counted": "", "files": "3 files", "folders": "1 folder",
                    "bytes": "12 bytes", "sample": '"a"', "covers": "floor:delete_permanent"},
                   {"what": "delete", "path": "/", "counted": "/x", "files": "1 file", "folders": "0 folders",
                    "bytes": "1 byte", "sample": "", "covers": "floor:delete_permanent"}],
    "escape:read:~/.ssh": [{"text": "let FERB read ~/.ssh", "lift": "read", "path": "~/.ssh"}, {"lift": "read"}],
    "escape:write:~/.zshrc": [{"lift": "write", "path": "~/.zshrc", "command": "echo x >> ~/.zshrc"}],
    "escape:admin": [{"lift": "admin", "command": "sudo ls"}],
    "escape:trustd": [{"lift": "service", "command": "go get x"}],
    "escape:other": [{"lift": "nothing", "text": "do the other thing"}],
    confirm.SINK_DESTINATION: [{"text": "reaching forms.example.com"}, {}],
}


def rule_need_sets() -> list[list[dict[str, Any]]]:
    sets: list[list[dict[str, Any]]] = []
    singles = []
    for rule, facts in RULE_FACTS.items():
        for f in facts + [{}]:
            n = ser_need(confirm.Need(rule, rule + ":k", f, destinations=(("email", "x@y.z"),) if "value" in f else ()))
            singles.append(n)
            sets.append([n])
    r = random.Random(17)
    for _ in range(150):
        sets.append(r.sample(singles, r.randint(2, 4)))
    return sets


# ---------- 7. terminal ----------

CHAINS: list[list[dict[str, Any]]] = [
    [], [{"role": "AXTextArea", "dom_classes": ["xterm-helper-textarea"]}, {"role": "AXGroup", "dom_classes": ["xterm"]}],
    [{"role": "AXTextArea", "dom_classes": ["monaco-editor"]}], [{"role": "AXTextArea", "subrole": "TerminalView"}],
    [{"role": "AXTextArea", "identifier": "shell-input"}], [{"role": "AXTextArea", "title": "Terminal 1, zsh"}],
    [{"role": "AXWindow", "title": "Terminal notes"}], [{"role": "AXTextArea", "description": "Debug Console"}],
    [{"role": "AXTextArea", "title": "terminals"}], [{"role": "AXTextArea", "role_description": "shellfish"}],
    [{"role": "AXTextArea", "identifier": "myConsoleView"}], [{"role": "AXTextArea", "title": "İnstaller sHell"}],
    [{"role": "AXButton", "dom_classes": ["codicon", "codicon-play"]}], [{"role": "AXButton", "title": "Run Python File"}],
    [{"role": "AXMenuItem", "title": "Debug: Start", "dom_classes": ["x"]}], [{"role": "AXButton", "title": "Runner"}],
    [{"role": "AXLink", "description": "  run all"}], [{"role": "AXStaticText", "title": "Run"}],
    [{"role": "AXTextArea", "dom_identifier": "x", "title": "Terminal"}], [{"role": "AXTextArea", "title": 5}]]
BUNDLES = ["com.apple.Terminal", "com.googlecode.iTerm2", "com.microsoft.VSCode", "dev.zed.Zed", "com.jetbrains.pycharm",
           "com.apple.TextEdit", "com.example.owned", "com.apple.dt.Xcode"]


def terminal_cases(h_surface: list[Any], h_need: list[Any], h_run: list[Any]) -> dict[str, Any]:
    out: dict[str, Any] = {"surface": [], "need": [], "run_control": [], "misc": []}
    for b in BUNDLES:
        for chain in CHAINS:
            for owner, shell, unread in ((["com.example.owned"], False, False), ([], True, False), ([], False, True)):
                out["surface"].append({"bundle_id": b, "chain": chain, "owner_apps": owner, "shell_app": shell,
                                       "unread": unread})
    out["surface"] += h_surface
    for c in out["surface"]:
        c["out"] = terminal.surface(c["bundle_id"], c["chain"], c["owner_apps"], c["shell_app"], c["unread"])
        c["can_host"] = terminal.can_host(c["bundle_id"], c["owner_apps"], c["shell_app"])
        c["structural"] = [terminal.structural(f) for f in c["chain"]]
    for chain in CHAINS:
        for editor in (False, True):
            out["run_control"].append({"chain": chain, "editor": editor})
    out["run_control"] += h_run
    for c in out["run_control"]:
        c["out"] = terminal.run_control(c["chain"], c["editor"])
    inputs = [TerminalInput("Terminal", "com.apple.Terminal", "a terminal app (com.apple.Terminal)", text="ls -la"),
              TerminalInput("Terminal", "com.apple.Terminal", "s", text="rm -rf x\n", enter=True, window="w1",
                            label="Terminal 108, zsh — a very long label that keeps going"),
              TerminalInput("Code", "com.microsoft.VSCode", "s", keys="ctrl+c"),
              TerminalInput("Code", "com.microsoft.VSCode", "s", keys="return"),
              TerminalInput("Code", "com.microsoft.VSCode", "s", text="secret \u200b\u3164x", paste=True, clipboard=3),
              TerminalInput("Code", "com.microsoft.VSCode", "s", paste=True, concealed=True),
              TerminalInput("iTerm", "com.googlecode.iterm2", "s", secret="github token"),
              TerminalInput("Finder", "com.apple.finder", "s", script="tell app \"Finder\" to open x", opens=True,
                            files=(("/Users/me/run.command", "abc", "#!/bin/sh\necho hi"),), named=("/Users/me/n.txt",)),
              TerminalInput("System Events", "com.apple.systemevents", "s", script="keystroke \"x\""),
              TerminalInput("Terminal", "com.apple.Terminal", "s", menu="Shell > New Window"),
              TerminalInput("Terminal", "com.apple.Terminal", "s", drag=True, text="dragged"),
              TerminalInput("Code", "com.microsoft.VSCode", "s", runs=True, control="Run Python File",
                            selection="print(1)\nprint(2)", file="/x/a.py", runs_where="Code's terminal", keys="F5"),
              TerminalInput("Code", "com.microsoft.VSCode", "s", runs=True, menu="Run > Run Without Debugging",
                            runs_where="Code"),
              TerminalInput("Code", "com.microsoft.VSCode", "s", runs=True, runs_where=""),
              TerminalInput("Terminal", "com.apple.Terminal", "s", files=(("/a/b.command", "h", "x"),)),
              TerminalInput("\u200eTerm", "com.apple.Terminal", "s\u0301", text="e\u0301\u0301 \u0301x\U000e0001"),
              TerminalInput("Shortcuts", "com.apple.shortcuts", "s", script="run")]
    for inp in inputs:
        out["need"].append(ser_ti(inp))
    out["need"] += h_need
    for c in out["need"]:
        ti = de_ti(c)
        c["out"] = ser_need(terminal.need(ti))
        c["wide"] = terminal.app_wide(ti)
    out["misc"] = [{"text": t, "visible": terminal.visible(t), "visible_raw": terminal.visible(t, False),
                    "newline": terminal.newline(t), "run_menu": terminal.run_menu([t]) if t else terminal.run_menu([]),
                    "terminal_menu": bool(terminal.TERMINAL_MENUS.match(t)), "app_key": terminal.app_key(t)}
                   for t in ["", "ls", "a\nb", "\x00\x7f\u2028\u200b", "\u0301x", "a\u0301", "Terminal", " run\n",
                             "SHELL", "Ruń", "Consoleİ", "conſole", "\u3164", "\U0001f600", "Run > x", "\u00ad"]]
    return out


# ---------- 8. certificate checks ----------

TLS_COMMANDS = [
    "curl -k https://x.example", "curl -sSLk https://a.example", "curl -o out -k x", "curl -ko f u", "curl -K cfg u",
    "curl -Kcfg", "curl --config c.txt https://x", "curl --config=c https://x", "curl --proxy-insecure x",
    "wget --no-check-certificate https://w.example", "wget --check-certificate=off u", "wget -e check_certificate=off u",
    "wget --execute=check_certificate = no u", "wget -echeck_certificate=0", "pip install --trusted-host pypi.ex a",
    "pip install --trusted-host=pypi.ex a", "pip3.11 install --cert '' x", "python -m pip install --cert=/dev/null x",
    "uv pip install --allow-insecure-host h.example x", "pip config set global.trusted-host h1.example,h2.example",
    "pip config set global.cert ''", "git -c http.sslVerify=false clone https://g.example/r",
    "git -c http.https://g.example.sslverify=0 clone x", "git config --global http.sslVerify false",
    "GIT_SSL_NO_VERIFY=1 git clone https://g.example/r",
    "GIT_CONFIG_COUNT=1 GIT_CONFIG_KEY_0=http.sslVerify GIT_CONFIG_VALUE_0=false git pull",
    "GIT_CONFIG_PARAMETERS=\"'http.sslverify'='false'\" git pull", "NODE_TLS_REJECT_UNAUTHORIZED=0 node app.js",
    "node -e 'https.get(u,{rejectUnauthorized: false})'", "node --eval=\"x={rejectUnauthorized:false}\"",
    "npm config set strict-ssl false", "npm set strict-ssl=false", "npm install --no-strict-ssl",
    "npm i --strict-ssl false", "yarn config set enableStrictSsl false", "npm_config_strict_ssl=false npm i",
    "PYTHONHTTPSVERIFY=0 python x.py", "python -c 'import ssl; ssl._create_unverified_context()'",
    "python3 -c \"requests.get(u, verify=False)\"", "python <<EOF\nimport ssl\nssl.CERT_NONE\nEOF",
    "REQUESTS_CA_BUNDLE= python x.py", "SSL_CERT_FILE=/dev/null curl https://x", "kubectl --insecure-skip-tls-verify get po",
    "helm --insecure-skip-tls-verify=false x", "aws s3 ls --no-verify-ssl", "GOINSECURE=g.example,h.example go get x",
    "sh <<EOF\ncurl -k https://in.heredoc\nEOF", "printf 'curl -k https://p.example' | sh", "echo curl -k u | bash -s",
    "ssh host curl -k https://remote.example", "ssh -p 22 host -- curl --insecure x", "deno run --unsafely-ignore-certificate-errors=a.example,b.example x",
    "docker pull --tls-verify=false x", "podman login --tlsverify false r", "http --verify=no https://h.example",
    "https --verify=False x", "aria2c --check-certificate=false u", "conda config --set ssl_verify false",
    "CONDA_SSL_VERIFY=false conda install x", "env -S 'curl -k https://env.example'", "sudo -u me curl -k u",
    "nice -n 10 curl -k u", "timeout 5 curl -k u", "watch 'curl -k https://w.example'", "eval curl -k x",
    "bash -lc 'curl -k https://b.example'", "export PIP_TRUSTED_HOST=pypi.example; pip install x",
    "`curl -k u`", "echo $(curl -k u)", "arch -arch arm64 curl -k u", "xargs -n 1 curl -k < f",
    "osascript -e 'do shell script \"curl -k https://o.example\"'", "ls -la; echo done", "curl https://safe.example",
    "curl 'unclosed -k", "yarn config set enableStrictSsl", "python -m pip install --cert", "pip install --cert",
    "env -u X -S curl -k u", "env -Scurl -k u", "watch", "a=1 b=2", "cat <<-'EOF'\n\tcurl -k x\n\tEOF\nsh <<EOF\ncurl -k y\nEOF",
    "bash <<\"END\"\ncurl -k h.example\nEND", "python3.11 - <<EOF\nverify=False\nEOF", "node <<X\nrejectUnauthorized:false\nX"]
APPLESCRIPTS = ['do shell script "curl -k https://a.example"', 'DO SHELL SCRIPT "curl \\"-k\\" x"',
                'do shell script "ls"', 'do  shell\tscript "curl --insecure u" & do shell script "wget --no-check-certificate w"',
                'tell app "x" to do shell scrİpt "curl -k z"']


def tls_cases(h_cmd: list[Any], h_script: list[Any]) -> dict[str, Any]:
    def ser(off: Any) -> Any:
        return None if off is None else {"how": list(off.how), "hosts": list(off.hosts)}
    cmds = list(dict.fromkeys(TLS_COMMANDS + [c["command"] for c in h_cmd]))
    scripts = list(dict.fromkeys(APPLESCRIPTS + [c["script"] for c in h_script]))
    return {"commands": [{"command": c, "out": outcome(lambda: tls.certificate_checks_off(c), ser)} for c in cmds],
            "scripts": [{"script": s, "out": outcome(lambda: tls.applescript_checks_off(s), ser)} for s in scripts]}


# ---------- 9. payments ----------

BASE = datetime(2026, 1, 2, 3, 4, 5, tzinfo=timezone.utc)


def pay_facts(d: dict[str, Any]) -> payments.PayFacts:
    return payments.PayFacts(d["chain"], d["host"], d.get("frame_host"), d.get("amount"), d.get("merchant"),
                             d.get("submits_card", False), d.get("submits", False), tuple(d.get("shown", ())),
                             d.get("declared"), d.get("page"), d.get("control"))


def ser_approval(a: Any) -> Any:
    if a is None:
        return None
    d = norm(a)
    d["at"] = a.at.isoformat()
    return d


def payments_cases() -> dict[str, Any]:
    r = random.Random(42)
    out: dict[str, Any] = {"same": [], "page_key": [], "digits": [], "stated": [], "sessions": [], "otp": []}
    shown = ["₹248.75", "INR 248.75", "₹2,48.75", "248.75", "$248.75", "₹249", "", None, "x", "Rs. 1,200", "1200",
             "USD 1200.00"]
    for a in shown:
        for b in shown:
            out["same"].append({"a": a, "b": b, "amount": payments._same(a, b, payments.amount_key),
                                "merchant": payments._same(a, b, payments.merchant_key)})
        out["digits"].append({"amount": a, "out": payments.digits_of(a)})
    for u in [None, "", "https://shop.example/cart#pay", "https://a.com/x?y#z", "x://[::1", "https://a.com",
              "a.com/x#y", "http://[bad/"]:
        out["page_key"].append({"url": u, "out": outcome(lambda: payments.page_key(u))})
    for read, declared, sh in [(None, "₹500", ["₹500"]), (None, "₹500", ["₹499"]), ("₹1", "₹500", []), (None, None, []),
                               ("", "₹5", ["INR 5"])]:
        out["stated"].append({"read": read, "declared": declared, "shown": sh,
                              "out": payments.stated(read, declared, tuple(sh))})
    hosts = ["shop.example", "bank.example", "pg.example"]

    def run(pay: Any, clock: dict[str, Any], step: dict[str, Any]) -> None:
        op, tree, facts = step["op"], step["tree"], step["facts"]
        if op == "check":
            c = pay.check(tree, pay_facts(facts), step["values"])
            step["out"] = {"approval": ser_approval(c.approval), "need": ser_need(c.need) if c.need else None}
        elif op == "approve":
            step["out"] = ser_approval(pay.approve(tree, pay_facts(facts), step["cid"]))
        elif op == "ran":
            pay.ran(tree, pay_facts(facts), step["ok"])
        elif op == "observe":
            pay.observe(tree, facts["chain"], facts["host"], facts.get("amount"), facts.get("merchant"))
        elif op == "for_otp":
            a = pay.for_otp(tree, facts["host"])
            step["out"] = ser_approval(a)
            step["otp_line"] = payments.otp_line(a)
            step["otp_card"] = list(payments.otp_card(facts["host"], a)) if a else None
        elif op == "set_aside":
            pay.set_aside(tree, facts["chain"])
        elif op == "first_change":
            step["out"] = pay.first_change(tree, facts["chain"], tuple(step["state"]))
        elif op == "tick":
            clock["t"] = clock["t"] + timedelta(seconds=step["seconds"])
        elif op == "active":
            step["out"] = ser_approval(pay.active(tree, facts["chain"]))
        elif op == "pressed":
            a = pay.active(tree, facts["chain"])
            now = None if step["now"] is None else payments.Pressed(
                step["now"]["url"], None if step["now"]["amounts"] is None else tuple(step["now"]["amounts"]))
            step["out"] = outcome(lambda: payments.pressed_change(a, pay_facts(facts), now, step["asked_here"])) \
                if a is not None else None
        elif op == "forget":
            pay.forget({tree})

    for _ in range(40):
        clock = {"t": BASE}
        pay = payments.Payments(now=lambda clock=clock: clock["t"])
        steps = []
        for _ in range(r.randint(3, 14)):
            op = r.choice(["check", "approve", "ran", "observe", "for_otp", "set_aside", "first_change", "tick",
                           "active", "pressed", "forget", "check"])
            facts = {"chain": r.choice(["tab-1", "tab-2"]), "host": r.choice(hosts),
                     "frame_host": r.choice([None, None, "", "pg.example"]),
                     "amount": r.choice([None, "₹248.75", "INR 248.75", "₹300", ""]),
                     "merchant": r.choice([None, "SWIGGY", "swiggy ", "Other"]), "submits_card": r.random() < 0.3,
                     "submits": r.random() < 0.4, "shown": r.sample(["₹248.75", "₹500", "₹300"], r.randint(0, 2)),
                     "declared": r.choice([None, "₹500", "₹248.75"]),
                     "page": r.choice([None, "https://shop.example/pay"]), "control": r.choice([None, "Pay now"])}
            step: dict[str, Any] = {"op": op, "tree": r.choice(["t1", "t2"]), "facts": facts}
            if op == "check":
                step["values"] = {"site": "shop.example", "account": "", "declared": facts["declared"] or ""}
            elif op == "approve":
                step["cid"] = f"c{len(steps)}"
            elif op == "ran":
                step["ok"] = r.random() < 0.8
            elif op == "first_change":
                step["state"] = r.choice([["page", "u1"], ["amount", 1], ["amount", 1.0], ["page", "u2"]])
            elif op == "tick":
                step["seconds"] = r.choice([30, 300, 601])
            elif op == "pressed":
                step["now"] = r.choice([None, {"url": "https://shop.example/pay#x", "amounts": ["₹500"]},
                                        {"url": "https://shop.example/other", "amounts": None},
                                        {"url": None, "amounts": []}])
                step["asked_here"] = r.random() < 0.5
            run(pay, clock, step)
            steps.append(step)
        out["sessions"].append(steps)

    # scripted: one payment, one question (GATE-204), what a bank page may show (GATE-216/217), when it is done,
    # the press-time re-check (GATE-580), and an OTP's payment
    def f(**kw: Any) -> dict[str, Any]:
        base = {"chain": "tab-1", "host": "shop.example", "frame_host": None, "amount": "₹248.75",
                "merchant": "SWIGGY", "submits_card": False, "submits": False, "shown": [], "declared": None,
                "page": "https://shop.example/pay", "control": "Pay now"}
        return base | kw
    V = {"site": "shop.example", "account": "HDFC", "declared": ""}
    scripts = [
        [("check", f()), ("approve", f()), ("check", f()), ("check", f(amount="INR 248.75")),
         ("check", f(amount="₹249")), ("check", f(amount=None)), ("check", f(chain="tab-2")),
         ("check", f(host="bank.example", amount="₹248.75", merchant="swiggy ")),
         ("check", f(host="bank.example", amount="₹300")), ("check", f(host="bank.example", merchant="Other")),
         ("check", f(frame_host="pg.example", amount="248.75")), ("check", f(frame_host="pg.example", amount="$5")),
         ("for_otp", f(host="bank.example")), ("observe", f(host="bank.example", amount="₹248.75", merchant="SWIGGY")),
         ("for_otp", f(host="bank.example")), ("ran", f(host="bank.example")), ("check", f()),
         ("active", f()), ("set_aside", f()), ("active", f())],
        [("approve", f(submits_card=True)), ("ran", f(submits_card=True)), ("check", f(submits_card=True)),
         ("check", f()), ("approve", f(chain="tab-2", amount=None)), ("ran", f(chain="tab-2", amount=None, submits=True)),
         ("check", f(chain="tab-2", amount=None)), ("tick", None), ("check", f()), ("approve", f()),
         ("tick", 599), ("check", f()), ("tick", 2), ("check", f()), ("active", f())],
        [("approve", f(amount=None, declared="₹500", shown=["₹500", "₹20"])), ("check", f(amount=None)),
         ("pressed", f(amount=None), {"url": "https://shop.example/pay#top", "amounts": ["₹500"]}, True),
         ("pressed", f(amount=None), {"url": "https://shop.example/other", "amounts": ["₹500"]}, True),
         ("pressed", f(amount=None, control="Other"), {"url": "https://shop.example/other", "amounts": ["₹500"]}, False),
         ("pressed", f(amount=None), {"url": "https://shop.example/pay", "amounts": ["₹499"]}, False),
         ("pressed", f(amount=None), {"url": "https://shop.example/pay", "amounts": None}, False),
         ("pressed", f(amount=None), None, True), ("pressed", f(amount=None, frame_host="pg.example"),
                                                   {"url": "x", "amounts": []}, True),
         ("first_change", f(), ["page", "u1"]), ("first_change", f(), ["page", "u1"]),
         ("first_change", f(), ["amount", 1]), ("first_change", f(), ["amount", 1.0]),
         ("first_change", f(), ["amount", True]), ("first_change", f(chain="tab-2"), ["page", "u1"]),
         ("check", f(amount=None, declared="₹500", shown=["₹50"])), ("forget", f()), ("active", f())],
        [("approve", f()), ("approve", f(chain="tab-2", host="other.example")), ("for_otp", f(host="bank.example")),
         ("observe", f(chain="tab-2", host="bank.example", amount="₹9")), ("for_otp", f(host="bank.example")),
         ("for_otp", f(host="other.example")), ("for_otp", f(host="nowhere.example")), ("set_aside", f()),
         ("for_otp", f(host="nowhere.example"))],
    ]
    for script in scripts:
        clock = {"t": BASE}
        pay = payments.Payments(now=lambda clock=clock: clock["t"])
        steps = []
        for n, item in enumerate(script):
            op, facts = item[0], item[1]
            step = {"op": op, "tree": "t1", "facts": facts if facts is not None else f()}
            if op == "check":
                step["values"] = dict(V, declared=facts.get("declared") or "")
            elif op == "approve":
                step["cid"] = f"cid-{n}"
            elif op == "ran":
                step["ok"] = True
            elif op == "tick":
                step["seconds"] = 601 if facts is None else facts
                step["facts"] = f()
            elif op == "pressed":
                step["now"], step["asked_here"] = item[2], item[3]
            elif op == "first_change":
                step["state"] = item[2]
            run(pay, clock, step)
            steps.append(step)
        out["sessions"].append(steps)
    return out


# ---------- 10. stored provenance ----------

def carried_cases(h: list[Any]) -> dict[str, Any]:
    r = random.Random(5)
    texts = ["Your phone +44 7700 900123 and order ORD-48213-7731 are noted.", "I sent stealth-launch to you on 2026-11-07.",
             "Code s3cretvalue at www.scaler.com/academy/class/77812", "nothing of yours here", "77812 C9x7Kp2 bluefalcon!",
             "Your word owner-sample appears"]
    shown_pool = [
        [["owner"], "my phone is +44 7700 900123", None, ""],
        [["private"], "order ORD-48213-7731 and s3cretvalue", None, ""],
        [["private", "external:web:www.scaler.com"], "class 77812 here", None,
         "https://www.scaler.com/academy/class/77812"],
        [["external:web:evil.example"], "C9x7Kp2 and 77812", None, ""],
        [["ferb"], "It is 2026-11-07", None, ""],
        [["ferb", "external"], "I saw 77812 earlier", {"77812": ["site:www.scaler.com/academy/class/77812"]}, ""],
        [["owner"], "the word owner-sample and bluefalcon!", None, ""],
        [["private"], "", None, ""]]
    cases = list(h)
    for _ in range(120):
        cases.append({"text": r.choice(texts), "shown": r.sample(shown_pool, r.randint(0, 4)),
                      "made": r.sample(["owner", "private", "ferb", "external:web:x.com"], r.randint(0, 2))})
    out = []
    for c in cases:
        shown = [(set(s[0]), s[1], s[2], s[3]) for s in c["shown"]]
        out.append({**c, "out": outcome(lambda: carried.derive(c["text"], shown, c["made"]))})
    segs = [
        ("Hi there\n\nI found 77812 for you\n\nowner msg", [{"len": 8, "kinds": ["owner"], "whole": True},
                                                            {"len": 21, "kinds": ["ferb"], "values": {"77812": [
                                                                "site:www.scaler.com/academy/class/77812"]}},
                                                            {"len": 9, "kinds": ["private"], "whole": True}]),
        ("a\n\nb\n\nc", [{"len": 1, "kinds": ["owner", "external"], "whole": True},
                         {"len": 1, "kinds": ["ferb"], "values": {"x1": ["private", "site:a"], "y2": ["site:a", "site:b"],
                                                                  "z3": ["site:a", "site:a"]}},
                         {"len": 1, "kinds": ["external"], "whole": True}]),
        ("ünï\n\ncödé", [{"len": 3, "kinds": ["owner"], "whole": True}, {"len": 4, "kinds": ["ferb"]}])]
    parts = []
    for payload, segments in segs:
        parts.append({"payload": payload, "segments": segments,
                      "sink_parts": norm(list(carried.sink_parts(payload, segments))),
                      "owner_said": carried.owner_said(payload, segments),
                      "sink_text": carried.sink_text(payload, segments)})
        Res = type("Res", (), {})
        res = Res()
        res.origins, res.text, res.segments, res.page = {"private"}, payload, segments, ""
        plain = Res()
        plain.origins, plain.text, plain.segments, plain.page = {"owner"}, payload, None, "https://x.com/p"
        parts[-1]["records"] = [[sorted(x[0]), x[1], x[2], x[3] if len(x) > 3 else None]
                                for x in carried.shown_records([res, plain])]
    for p in parts:
        for rec in p["records"]:
            if rec[2] is not None and isinstance(rec[2], dict):
                rec[2] = norm(rec[2])
    return {"derive": out, "parts": parts}


# ---------- 11. rules ----------

def rules_cases() -> dict[str, Any]:
    out: dict[str, Any] = {"declare": [], "hash": [], "declared_of": []}
    specs = [ActionSpec("click", Target(recipients=("a@x.com",))), ActionSpec("send", Target(account="me")),
             ActionSpec("read"), ActionSpec("run_shell", Target(amount=5.0)), ActionSpec("mac_input", Target(item="x")),
             ActionSpec("fill_form", Target(currency="₹", account=""))]
    for spec in specs:
        for effect in ["send", " PAY ", "destroy", "account_security", "navigate", "", "Send"]:
            d = rules.declare(spec, effect, (" b@x.com ", "a@x.com", ""), " acct ", 12.5, " the item ",
                              "  INRUPEES LONG ")
            out["declare"].append({"kind": spec.kind, "target": ser_target(spec.target), "effect": effect,
                                   "out_kind": d.kind, "out_effect": d.effect_kind, "out_target": ser_target(d.target),
                                   "declared_kinds": sorted(rules.declared_kinds(d))})
    for v in ["send", "", ["a", "b"], None, 3]:
        out["declared_of"].append({"value": v, "out": sorted(rules.declared_of(v))})
    reqs = [rules.ActionRequest("probe.send", "send", Identity.OWNER, frozenset(), "t", "w", "r", frozenset(),
                                Target(account="é", site="s", recipients=("b", "a"), amount=1.5), "hello é"),
            rules.ActionRequest("x", "click", Identity.FERB, frozenset(), "t", "w", "r", frozenset())]
    for a in reqs:
        for args in [None, {"why": "x", "point": [1, 2], "z": {"b": 1, "a": "é"}, "f": 1.0, "n": None, "t": True}]:
            out["hash"].append({"capability": a.capability, "identity": a.identity.value, "target": ser_target(a.target),
                                "content": a.content, "args": args, "out": rules.payload_hash(a, args)})
    return out


# ---------- 12. foreground and app keys ----------

def foreground_cases(h: list[Any]) -> list[Any]:
    cases = list(h)
    for tool in ["mac.act", "mac.click", "mac.pointer", "mac.applescript", "mac.menu", "mac.read", "browser.click"]:
        for args in [{"app": "Safari"}, {"app": "Google Chrome", "path": "File > New"}, {"app": "pid:102", "path": " "},
                     {"app": "Terminal", "allow_foreground": True}, {"app": "Terminal", "mechanism": "foreground"},
                     {"app": "pages", "allow_foreground": 1}, {"app": "Twin", "allow_foreground": True},
                     {"app": "unreadable app", "allow_foreground": True}, {"allow_foreground": True},
                     {"app": "\u200eWhatsApp", "mechanism": "foreground"}, {"app": "broken app", "allow_foreground": True},
                     {"app": "com.google.Chrome", "script": "tell x"}, {"app": "arc", "allow_foreground": False}]:
            cases.append({"tool": tool, "args": args})
    out = []
    for c in cases:
        res, log = with_apps(lambda: (lambda n: ser_need(n) if n else None)(confirm.foreground_need(c["tool"], c["args"])))
        out.append({**c, "out": res, "apps": log})
    for app in ["Terminal", "pid:101", "TERMINAL", "\u200eWhatsApp", "Twin", "pages", "Pages", "nothing", "broken thing",
                "unreadable", "com.apple.Terminal", "pid:999", "com.example.noname", "net.whatsapp.WhatsApp", " safari "]:
        res, log = with_apps(lambda: [confirm.app_key(app), confirm.app_label(app)])
        out.append({"app": app, "out": res, "apps": log})
    return out


# ---------- 13. stores ----------

def stores_cases() -> list[Any]:
    from ferb.sanitize.redact import Redactor
    from ferb.store.db import Database

    class Clock:
        def __init__(self) -> None:
            self.n = 0

        def now(self) -> datetime:
            self.n += 1
            return BASE + timedelta(seconds=self.n, microseconds=250000 * (self.n % 4))

    scenarios = []
    rnd = random.Random(99)
    for s in range(12):
        folder = TMP / f"store{s}"
        folder.mkdir()
        db = Database(folder / "ferb.db")
        db.migrate()
        clock, ids = Clock(), SequentialIds("id")
        red = Redactor()
        red.add_secret_value("quietlemon4417")
        kd = destinations.KnownDestinations(db, clock, ids, red)
        cf = destinations.Confirmations(db, clock, ids, red)
        sp = standing.StandingPermissions(db)
        steps = []
        keys = [confirm.key("floor:new_recipient", ["a@x.com"]), confirm.key("floor:pay", "s", None, 1.5, []),
                confirm.key("floor:new_recipient", ["quietlemon4417@x.com"]), "code 482193",
                confirm.destination_key("evil.example"), terminal.app_key("com.apple.Terminal")]
        cids: list[str] = []
        for _ in range(rnd.randint(6, 18)):
            op = rnd.choice(["record_dest", "known", "approved", "record_conf", "find", "noes", "refused", "covers",
                             "use", "release", "age", "grant", "covers_standing", "scopes", "revoke",
                             "revoke_approval", "held", "approvals"])
            tree = rnd.choice(["task-1", "task-2"])
            step: dict[str, Any] = {"op": op}
            if op == "record_dest":
                step["args"] = [rnd.choice(["email", "host", "phone", "name", "url"]),
                                rnd.choice(["Rahul@Example.com", "evil.example.", "+91 98765 43210", "Rahul  VERMA",
                                            "quietlemon4417@x.com", "code 482193", ""]),
                                rnd.choice(["named", "answered", "confirmed", "guessed"]), rnd.choice([None, "req-1"]),
                                rnd.choice([None, "span-1"])]
                kd.record(*step["args"])
            elif op in ("known", "approved"):
                step["args"] = [rnd.choice(["email", "host", "phone", "name"]),
                                rnd.choice(["rahul@example.com", "EVIL.example", "919876543210", "rahul verma",
                                            "quietlemon4417@x.com", "code 482193"])]
                step["out"] = getattr(kd, op)(*step["args"])
            elif op == "record_conf":
                step["args"] = [tree, "floor:x", rnd.choice(keys), rnd.random() < 0.6, rnd.choice([None, "ask-1"])]
                cid = cf.record(*step["args"])
                cids.append(cid)
                step["out"] = cid
            elif op == "find":
                step["args"] = [tree, rnd.choice(keys), rnd.sample(cids, min(len(cids), rnd.randint(0, 2)))]
                row = cf.find(*step["args"])
                step["out"] = None if row is None else {k: row[k] for k in row}
            elif op == "noes":
                step["args"] = [tree]
                step["out"] = cf.noes(tree)
            elif op == "refused":
                step["args"] = [tree]
                step["out"] = cf.refused(tree)
            elif op == "covers":
                step["args"] = [rnd.choice(cids) if cids else "none", rnd.choice(keys)]
                step["out"] = cf.covers(*step["args"])
            elif op in ("use", "release"):
                if not cids:
                    continue
                step["args"] = [rnd.choice(cids)]
                getattr(cf, op)(*step["args"])
            elif op == "age":
                if not cids:
                    continue
                cid = rnd.choice(cids)
                row = dict(db.conn.execute("SELECT * FROM confirmations WHERE id = ?", (cid,)).fetchone())
                step["args"] = [cid]
                step["out"] = cf.age_s(row)
            elif op == "grant":
                from ferb.orchestrator.plans import SendScope
                scopes = [SendScope(capability=rnd.choice(["probe.send", "browser.click"]), identity="owner",
                                    account=rnd.choice([None, "@Me ", "me"]), site=rnd.choice([None, "www.X.example", "x.example"]),
                                    recipients=rnd.sample(["A@x.com", "b@x.com", "ünï"], rnd.randint(0, 2)),
                                    content=rnd.choice([None, "", "hi é"])) for _ in range(rnd.randint(1, 3))]
                step["args"] = [tree, [json.loads(sc.model_dump_json()) for sc in scopes], rnd.choice(["ap-1", "schedule:7"])]
                sp.grant(tree, scopes, step["args"][2])
            elif op == "covers_standing":
                spec = ActionSpec(rnd.choice(["send", "click", "pay", "post"]),
                                  Target(account=rnd.choice([None, "me", "@ME"]), site=rnd.choice([None, "x.example", "www.x.example"]),
                                         recipients=tuple(rnd.sample(["a@x.com", "B@x.com"], rnd.randint(0, 2))),
                                         amount=rnd.choice([None, None, 3.0])),
                                  content=rnd.choice([None, "", "hi é"]), declared=rnd.choice([None, "send", "pay"]),
                                  effect_kind=rnd.choice([None, "send"]))
                reaches = rnd.choice([None, Target(site="x.example", recipients=("a@x.com",))])
                step["args"] = [tree, rnd.choice(["probe.send", "browser.click"]), "owner",
                                {"kind": spec.kind, "target": ser_target(spec.target), "content": spec.content,
                                 "declared": spec.declared, "effect_kind": spec.effect_kind}, ser_target(reaches)]
                step["out"] = sp.covers(tree, step["args"][1], "owner", spec, reaches)
                step["sends"] = standing.StandingPermissions.sends(spec)
            elif op == "scopes":
                step["args"] = [tree]
                step["out"] = [json.loads(s_.model_dump_json()) for s_ in sp.scopes(tree)]
            elif op == "revoke":
                step["args"] = [tree]
                sp.revoke(tree)
            elif op == "revoke_approval":
                step["args"] = [rnd.choice(["ap-1", "schedule:7"])]
                step["out"] = sp.revoke_approval(step["args"][0])
            elif op == "held":
                step["args"] = [rnd.choice(["ap-1", "schedule:7"])]
                step["out"] = sp.held(step["args"][0])
            elif op == "approvals":
                step["args"] = [tree]
                step["out"] = sp.approvals(tree)
            steps.append(step)
        db.conn.row_factory = sqlite3.Row
        dump = {t: [dict(r_) for r_ in db.conn.execute(f"SELECT rowid AS _rowid, * FROM {t} ORDER BY rowid")]
                for t in ("known_destinations", "confirmations", "standing_permissions")}
        scenarios.append({"steps": steps, "rows": dump})
    # scripted: each behaviour the gateway builds on, step by step (GATE-205/218, D33 per tree, GATE-585/586, D30)
    from ferb.orchestrator.plans import SendScope
    k_new = confirm.key("floor:new_recipient", ["rahul@example.com"])
    k_pay = confirm.key("floor:pay", "shop.example", None, 248.75, [])
    k_secret = confirm.key("floor:new_recipient", ["quietlemon4417@x.com"])
    k_term = terminal.need(TerminalInput("Terminal", "com.apple.Terminal", "s", text="ls")).key
    k_app = terminal.app_key("com.apple.Terminal")
    scope_a = {"capability": "probe.send", "identity": "owner", "account": "@Me", "site": "www.X.example",
               "recipients": ["A@x.com"], "content": "hello é"}
    scope_b = {"capability": "browser.click", "identity": "owner", "account": None, "site": "x.example",
               "recipients": [], "content": None}
    scope_c = {"capability": "probe.send", "identity": "owner", "account": None, "site": None, "recipients": [],
               "content": ""}

    def spec_args(kind: str, target: Target, content: Any = None, declared: Any = None, effect: Any = None) -> dict:
        return {"kind": kind, "target": ser_target(target), "content": content, "declared": declared,
                "effect_kind": effect}
    scripted = [
        # known for normal use versus approved for the sink rule (destinations.py:80 and :86)
        [("record_dest", ["host", "Evil.Example.", "named", "req-1", None]), ("known", ["host", "evil.example"]),
         ("approved", ["host", "evil.example"]), ("record_dest", ["host", "evil.example", "answered", None, "span-2"]),
         ("approved", ["host", "evil.example"]), ("record_dest", ["host", "evil.example", "named", "req-3", None]),
         ("approved", ["host", "evil.example"]), ("record_dest", ["email", "Rahul@Example.com", "confirmed", None, None]),
         ("known", ["email", "rahul@example.com"]), ("approved", ["email", "RAHUL@example.com"]),
         ("record_dest", ["phone", "+91 98765 43210", "named", None, None]), ("known", ["phone", "919876543210"]),
         ("approved", ["phone", "+91-98765-43210"]), ("record_dest", ["name", "Rahul  VERMA", "answered", None, None]),
         ("known", ["name", "rahul verma"]), ("record_dest", ["email", "quietlemon4417@x.com", "confirmed", None, None]),
         ("known", ["email", "quietlemon4417@x.com"]), ("record_dest", ["name", "code 482193", "confirmed", None, None]),
         ("known", ["name", "code 482193"]), ("approved", ["name", "Code  482193"]),
         ("record_dest", ["host", "x.example", "guessed", None, None]), ("known", ["host", "x.example"]),
         ("record_dest", ["url", "x.example", "confirmed", None, None]), ("known", ["url", "x.example"]),
         ("record_dest", ["host", "  ", "confirmed", None, None]), ("known", ["host", ""])],
        # one tree's answers: per tree, the latest answer read, a No refusing, a reopened No, one-run claims
        [("record_conf", ["task-1", "floor:new_recipient", k_new, True, "ask-1"]), ("find", ["task-1", k_new, []]),
         ("find", ["task-2", k_new, []]), ("find", ["task-1.3", k_new, []]),
         ("record_conf", ["task-1", "floor:new_recipient", k_new, False, "ask-2"]), ("find", ["task-1", k_new, []]),
         ("noes", ["task-1"]), ("refused", ["task-1"]), ("find", ["task-1", k_new, ["id-000002"]]),
         ("find", ["task-1", k_new, ["id-000001"]]), ("covers", ["id-000001", k_new]), ("covers", ["id-000002", k_new]),
         ("covers", ["id-000001", k_pay]), ("record_conf", ["task-1", "floor:new_recipient", k_new, True, None]),
         ("noes", ["task-1"]), ("refused", ["task-1"]), ("find", ["task-1", k_new, ["id-000002"]]),
         ("record_conf", ["task-1", "floor:pay", k_pay, True, "ask-4"]), ("use", ["id-000004"]),
         ("find", ["task-1", k_pay, []]), ("release", ["id-000004"]), ("find", ["task-1", k_pay, []]),
         ("age", ["id-000004"]), ("record_conf", ["task-1", "floor:new_recipient", k_secret, True, None]),
         ("find", ["task-1", k_secret, []]), ("covers", ["id-000005", k_secret]),
         ("record_conf", ["task-2", "x", "code 482193", True, None]), ("find", ["task-2", "code 482193", []]),
         ("covers", ["id-000006", "code 482193"]), ("noes", ["task-2"])],
        # GATE-585/586: the Yes to one terminal input also records its app for the tree; another tree asks again
        [("record_conf", ["task-1", confirm.TERMINAL_INPUT, k_term, True, "ask-1"]),
         ("record_conf", ["task-1", confirm.TERMINAL_APP, k_app, True, "ask-1"]),
         ("find", ["task-1", terminal.app_key("COM.APPLE.TERMINAL"), []]), ("find", ["task-2", k_app, []]),
         ("find", ["task-1", terminal.app_key("com.googlecode.iterm2"), []]),
         ("record_conf", ["task-1", confirm.TERMINAL_INPUT, k_term, False, "ask-2"]), ("find", ["task-1", k_term, []]),
         ("find", ["task-1", k_app, []]), ("refused", ["task-1"])],
        # D30: standing scopes, matched need by need on the call's own and the need's target
        [("grant", ["task-1", [scope_a, scope_b], "ap-1"]), ("grant", ["schedule:7", [scope_c], "schedule:7"]),
         ("scopes", ["task-1"]), ("approvals", ["task-1"]), ("held", ["ap-1"]), ("held", ["schedule:7"]),
         ("covers_standing", ["task-1", "probe.send", "owner", spec_args("send", Target(account="me", site="x.example",
                                                                                    recipients=("a@X.com",)),
                                                                      "hello é"), None]),
         ("covers_standing", ["task-1", "probe.send", "owner", spec_args("send", Target(account="me", site="x.example",
                                                                                    recipients=("a@x.com",)),
                                                                      "hello"), None]),
         ("covers_standing", ["task-1", "probe.send", "ferb", spec_args("send", Target(account="me", site="x.example",
                                                                                   recipients=("a@x.com",)),
                                                                     "hello é"), None]),
         ("covers_standing", ["task-1", "browser.click", "owner", spec_args("click", Target(page="p"), None, None,
                                                                           "send"),
                              ser_target(Target(site="www.x.example"))]),
         ("covers_standing", ["task-1", "browser.click", "owner", spec_args("click", Target(), None, None, None),
                              ser_target(Target(site="x.example"))]),
         ("covers_standing", ["task-1", "browser.click", "owner", spec_args("click", Target(), None, "pay", "send"),
                              ser_target(Target(site="x.example"))]),
         ("covers_standing", ["task-1", "browser.click", "owner", spec_args("post", Target(amount=3.0), "x"), None]),
         ("covers_standing", ["task-1", "browser.click", "owner", spec_args("post", Target(contact="Rahul"), "x"),
                              ser_target(Target(site="x.example"))]),
         ("covers_standing", ["schedule:7", "probe.send", "owner", spec_args("send", Target(), "x"), None]),
         ("revoke", ["task-1"]), ("scopes", ["task-1"]), ("revoke_approval", ["schedule:7"]),
         ("held", ["schedule:7"]), ("approvals", ["schedule:7"])],
    ]
    for n, script in enumerate(scripted):
        folder = TMP / f"scripted{n}"
        folder.mkdir()
        db = Database(folder / "ferb.db")
        db.migrate()
        clock, ids = Clock(), SequentialIds("id")
        red = Redactor()
        red.add_secret_value("quietlemon4417")
        kd = destinations.KnownDestinations(db, clock, ids, red)
        cf = destinations.Confirmations(db, clock, ids, red)
        sp = standing.StandingPermissions(db)
        steps = []
        for op, args in script:
            step = {"op": op, "args": args}
            if op == "record_dest":
                kd.record(*args)
            elif op in ("known", "approved"):
                step["out"] = getattr(kd, op)(*args)
            elif op == "record_conf":
                step["out"] = cf.record(*args)
            elif op == "find":
                row = cf.find(*args)
                step["out"] = None if row is None else {k: row[k] for k in row}
            elif op in ("noes", "refused"):
                step["out"] = getattr(cf, op)(*args)
            elif op == "covers":
                step["out"] = cf.covers(*args)
            elif op in ("use", "release"):
                getattr(cf, op)(*args)
            elif op == "age":
                row = dict(db.conn.execute("SELECT * FROM confirmations WHERE id = ?", (args[0],)).fetchone())
                step["out"] = cf.age_s(row)
            elif op == "grant":
                sp.grant(args[0], [SendScope(**x) for x in args[1]], args[2])
            elif op == "covers_standing":
                a = args[3]
                spec = ActionSpec(a["kind"], de_target(a["target"]), content=a["content"], declared=a["declared"],
                                  effect_kind=a["effect_kind"])
                step["out"] = sp.covers(args[0], args[1], args[2], spec, de_target(args[4]))
                step["sends"] = standing.StandingPermissions.sends(spec)
            elif op == "scopes":
                step["out"] = [json.loads(x.model_dump_json()) for x in sp.scopes(args[0])]
            elif op == "revoke":
                sp.revoke(args[0])
            elif op in ("revoke_approval", "held", "approvals"):
                step["out"] = getattr(sp, op)(*args)
            steps.append(step)
        db.conn.row_factory = sqlite3.Row
        dump = {t: [dict(r_) for r_ in db.conn.execute(f"SELECT rowid AS _rowid, * FROM {t} ORDER BY rowid")]
                for t in ("known_destinations", "confirmations", "standing_permissions")}
        scenarios.append({"steps": steps, "rows": dump})
    # the daily digest and sites contacted, over seeded rows (the same SQL runs on the Rust store)
    folder = TMP / "digest"
    folder.mkdir()
    db = Database(folder / "ferb.db")
    db.migrate()
    span_sql = ("INSERT INTO spans (id, task_id, kind, started_at, ended_at, input_json, output_json, error) "
                "VALUES (?, ?, ?, ?, ?, ?, ?, ?)")
    seed: list[list[Any]] = [
        ["INSERT INTO known_destinations VALUES ('d1','host','x.example','confirmed',NULL,NULL,'2026-01-02T00:00:00+00:00')", []],
        ["INSERT INTO known_destinations VALUES ('d2','email','a@b.co','named','r',NULL,'2026-01-03T00:00:00+00:00')", []],
        ["INSERT INTO confirmations VALUES ('c1','t','floor:pay','k',1,NULL,1,'2026-01-02T01:00:00+00:00')", []],
        ["INSERT INTO confirmations VALUES ('c2','t','floor:pay','k',0,NULL,0,'2026-01-03T01:00:00+00:00')", []],
    ]
    spans_rows = [
        ("s1", "t1", "tool.call", {"tool": "probe.send", "standing": {"task": "t1", "rules": ["floor:new_recipient"]},
                                   "target": {"site": "x"}, "declared": "send", "kind": "send", "args": {}},
         {"network": {"refused": [{"host": "h.example"}, "plain"]}}, None, "2026-01-02T00:00:01+00:00",
         "2026-01-02T00:00:02+00:00"),
        ("s2", "t1.1", "tool.refused", {"tool": "shell.run"}, None, "refused: x", "2026-01-02T00:00:03+00:00", None),
        ("s3", "t1", "tool.call", {"tool": "browser.open", "kind": "open_tab", "args": {"url": "https://Www.A.example./x"}},
         {"url": "https://b.example/"}, None, "2026-01-02T00:00:04+00:00", "2026-01-02T00:00:05+00:00"),
        ("s4", "t1.2", "tool.call", {"tool": "browser.go", "kind": "navigate", "args": {"url": "chrome://settings"}},
         {"url": "https://c.example/"}, None, "2026-01-02T00:00:06+00:00", "2026-01-02T00:00:07+00:00"),
        ("s5", "t1", "tool.call", {"tool": "browser.go", "kind": "navigate", "args": {"url": "https://d.example/"},
                                   "outcome": "unknown"}, None, "boom", "2026-01-02T00:00:08+00:00",
         "2026-01-02T00:00:09+00:00"),
        ("s6", "t2", "tool.call", {"tool": "browser.go", "kind": "navigate", "args": {"url": "https://other.example/"}},
         None, None, "2026-01-02T00:00:10+00:00", "2026-01-02T00:00:11+00:00"),
        ("s7", "t1", "tool.call", {"tool": "browser.go", "kind": "navigate", "args": {"url": "https://e.example/"}},
         None, "failed", "2026-01-02T00:00:12+00:00", "2026-01-02T00:00:13+00:00"),
        ("s8", "t1", "tool.call", {"tool": "browser.go", "kind": "navigate", "args": {"url": "https://f.example/"}},
         None, None, "2026-01-02T00:00:14+00:00", None),
        ("s9", "t10", "tool.call", {"tool": "browser.go", "kind": "navigate", "args": {"url": "https://g.example/"}},
         {"url": "ftp://h.example/"}, None, "2026-01-03T00:00:14+00:00", "2026-01-03T00:00:15+00:00"),
    ]
    for sid, task, kind, inp, out_, err, started, ended in spans_rows:
        seed.append([span_sql, [sid, task, kind, started, ended, json.dumps(inp),
                                json.dumps(out_) if out_ is not None else None, err]])
    journal_sql = "INSERT INTO journal (id, at, kind, data_json, origin) VALUES (?, ?, ?, ?, ?)"
    for jid, data, origin_ in [("j1", {"task_id": "t1", "destinations": [{"host": "Pypi.Org."}, {"host": "excluded"},
                                                                          {"host": ""}, {}]}, None),
                               ("j2", {"task_id": "t1.3", "destinations": [{"host": "files.example"}]}, None),
                               ("j3", {"task_id": "t2", "destinations": [{"host": "t2.example"}]}, None),
                               ("j4", {"task_id": "t1"}, "external:web:x")]:
        seed.append([journal_sql, [jid, "2026-01-02T00:00:00+00:00", "shell.network", json.dumps(data), origin_]])
    trail_sql = ("INSERT INTO trail (id, at, day, device_id, actor, task_id, source, data_json) "
                 "VALUES (?, ?, ?, ?, ?, ?, ?, ?)")
    for tid, actor, task, source, data in [("r1", "ferb", "t1", "browser_tab", {"url": "https://Loaded.example/p"}),
                                           ("r2", "ferb", "t1.4", "browser_tab", {"host": "Hosted.Example"}),
                                           ("r3", "owner", "t1", "browser_tab", {"url": "https://owner.example/"}),
                                           ("r4", "ferb", "t1", "browser_tab", {"url": "brave://history"}),
                                           ("r5", "ferb", "t1", "window", {"url": "https://win.example/"}),
                                           ("r6", "ferb", "t1", "browser_tab", {"url": "", "host": "fallback.example"})]:
        seed.append([trail_sql, [tid, "2026-01-02T00:00:00+00:00", "2026-01-02", "dev", actor, task, source,
                                 json.dumps(data)]])
    mem_sql = ("INSERT INTO memory_items (id, kind, subject, key, content, origin, status, confidence, observed_at, "
               "valid_from, created_at) VALUES (?, 'fact', 'owner', 'k', 'c', ?, 'active', 1.0, 'x', 'x', ?)")
    for mid, origin_, created in [("m1", "third_party", "2026-01-02T05:00:00+00:00"),
                                  ("m2", "owner_stated", "2026-01-02T06:00:00+00:00"),
                                  ("m3", "observed", "2026-01-02T07:00:00+00:00"),
                                  ("m4", "third_party", "2026-01-04T05:00:00+00:00")]:
        seed.append([mem_sql, [mid, origin_, created]])
    seed.append(["INSERT INTO memory_sources (item_id, kind, source_id) VALUES ('m2', 'trail', 'r5')", []])
    seed.append(["INSERT INTO memory_sources (item_id, kind, source_id) VALUES ('m3', 'journal', 'j4')", []])
    for sql, params in seed:
        db.conn.execute(sql, params)
    digest = security_digest.build(db, "2026-01-02")
    from ferb.store.spans import SpanStore
    contacted = {t: sites.sites_contacted(SpanStore(db, Clock(), SequentialIds("sp"), lambda v: v), t)
                 for t in ("t1", "t2", "t10", "t3")}
    return {"scenarios": scenarios, "digest": {"seed": seed, "out": norm(digest), "contacted": contacted}}


# ---------- 14. protection ----------

def above_tmp(path: str) -> bool:
    """A folder above this run's temporary root, in any spelling (it differs from one run or machine to the next)."""
    roots = [str(TMP), os.path.realpath(TMP)]
    roots += ["/System/Volumes/Data" + r for r in roots]
    return any(r.startswith(path.rstrip("/") + "/") for r in roots)


def protection_cases() -> list[Any]:
    out = []
    for n, layout in enumerate(["plain", "linked", "app"]):
        root = TMP / f"prot{n}"
        home = root / "home"
        ferb = home / ".ferb"
        for d in [home / "Documents", home / ".ssh", ferb / "workspace" / "uploads", home / "Library" / "Keychains",
                  home / "Library" / "Containers" / "com.apple.Safari" / "Data", home / "Library" / "LaunchAgents",
                  home / "notes", root / "FERB.app" / "Contents" / "Library" / "LaunchAgents", root / "other"]:
            d.mkdir(parents=True, exist_ok=True)
        (home / ".ssh" / "id_test").write_text("x")
        (home / "Documents" / "a.txt").write_text("a")
        (home / ".zshrc").write_text("x")
        (ferb / "ferb.db").write_text("x")
        (root / "FERB.app" / "Contents" / "Library" / "LaunchAgents" / "com.ferb.daemon.plist").write_text("x")
        (home / "Library" / "LaunchAgents" / "com.ferb.daemon.plist").write_text("x")
        os.symlink(home / ".ssh", home / "Documents" / "sshlink")
        os.symlink(ferb, home / "notes" / "ferblink")
        os.symlink(home / "loop2", home / "loop1")
        os.symlink(home / "loop1", home / "loop2")
        os.symlink("relative/../../.ssh", home / "Documents" / "rel")
        os.mkfifo(home / "Documents" / "fifo")
        if layout == "linked":
            os.symlink(home / "Documents", root / "other" / "docs")
            os.symlink(root / "other", home / "Library" / "Containers" / "linkedcontainer")
        never_read = list(protection.DEFAULT_NEVER_READ) + (["~/notes", "relative/thing", "~", "~/.ssh"]
                                                            if layout != "plain" else [])
        never_write = list(protection.DEFAULT_NEVER_WRITE) + (["/System/Volumes/Data" + str(home / "Documents")]
                                                              if layout == "linked" else [])
        lists = {"never_read": never_read, "never_write": never_write,
                 "terminal_apps": list(protection.DEFAULT_TERMINAL_APPS) + ["Com.Example.TERM"],
                 "local_ports": [{"port": 8080, "executable": str(root / "other")}] if layout == "app" else []}
        app = str(root / "FERB.app") if layout == "app" else None
        p = protection.Protection(home, ferb, lists["never_read"], lists["never_write"], lists["terminal_apps"],
                                  local_ports=lists["local_ports"], app_bundle=Path(app) if app else None)
        probes = [home / "Documents" / "a.txt", home / ".ssh" / "id_test", home / "Documents" / "sshlink" / "id_test",
                  ferb / "ferb.db", ferb / "workspace" / "x.txt", ferb / "workspace" / "uploads" / "y",
                  ferb / "WORKSPACE" / "z", home / "notes" / "ferblink" / "ferb.db", home / ".zshrc",
                  home / "Library" / "LaunchAgents" / "com.ferb.daemon.plist", home / "Library" / "LaunchAgents" / "other.plist",
                  root / "FERB.app" / "Contents" / "MacOS" / "x", root / "FERB.app", home / "Library",
                  home / "Library" / "Containers" / "com.apple.Safari" / "Data" / "x", home / "loop1" / "x",
                  home / "Documents" / "rel", home / "Documents" / "fifo", Path("relative.txt"), Path("/.vol/1/2/x"),
                  Path("/System/Volumes/Data") / str(home / ".ssh").lstrip("/"), home, root, Path("/"),
                  home / "Documents" / ".." / ".ssh" / "k", home / ".SSH" / "id_TEST",
                  home / "Library" / "Containers" / "linkedcontainer" / "f", root / "other" / "docs" / "a.txt",
                  home / "Ｄocuments", home / ".ssh"]
        results = []
        for probe in probes:
            res: dict[str, Any] = {"path": str(probe)}
            res["read_denied"] = outcome(lambda: p.read_denied(probe))
            res["write_denied"] = outcome(lambda: p.write_denied(probe))
            res["ferb_state"] = outcome(lambda: p.ferb_state(probe))
            res["app_data"] = p.app_data(probe)
            for access in ("read", "write", "remove", "metadata", "trash", "delete_permanent"):
                res["fs_" + access] = outcome(lambda: p.fs_rules(probe, access))
            results.append(res)
        state = {"home": str(p.home), "ferb_home": str(p.ferb_home), "workspace": str(p.workspace),
                 "uploads": str(p.uploads), "reads": norm(p.reads), "writes": norm(p.writes),
                 "terminal_apps": sorted(p.terminal_apps), "service_files": norm(p.service_files),
                 "local_ports": norm(p.local_ports), "read_entries": norm(p.read_entries()),
                 "write_entries": norm(p.write_entries()), "app_entries": norm(p.app_entries()),
                 "guarded_parents": [g for g in norm(p.guarded_parents()) if not above_tmp(g)], "rules": p.rules(),
                 "liftable": {r_: p.liftable(r_) for r_ in ["admin", "trustd", "ferb_state", "network", "ferb_app",
                                                            "ferb_service", "read:~/.ssh", "write:~/.zshrc", "read:~/nope"]},
                 "forms_now": [norm(p.forms_now(x)) for x in [home / ".ssh", home / "Library" / "Containers" / "com.apple.Safari",
                                                              home / "Documents" / "sshlink"]],
                 "workspace_problem": protection.workspace_problem(ferb)}
        out.append({"layout": layout, "lists": lists, "app": app, "results": results, "state": state})
    # a workspace that is a link, and one that is a file
    bad = []
    for kind in ("link", "file", "fine", "missing"):
        root = TMP / f"ws-{kind}"
        ferb = root / ".ferb"
        ferb.mkdir(parents=True)
        if kind == "link":
            (root / "elsewhere").mkdir()
            os.symlink(root / "elsewhere", ferb / "workspace")
        elif kind == "file":
            (ferb / "workspace").write_text("x")
        elif kind == "fine":
            (ferb / "workspace").mkdir()
        bad.append({"kind": kind, "out": protection.workspace_problem(ferb)})
    errors = []
    for lists in [{"never_read": "~/.ssh"}, {"never_read": [1]}, {"never_read": [" "]}, {"never_write": {"a": 1}},
                  {"terminal_apps": ["", "x"]}, {"local_ports": [{"port": 0, "executable": "/bin/sh"}]},
                  {"local_ports": [{"port": 8080, "executable": "bin/sh"}]},
                  {"local_ports": [{"port": True, "executable": "/bin/sh"}]},
                  {"local_ports": [{"port": 1, "executable": "/a"}, {"port": 1, "executable": "/b"}]},
                  {"local_ports": [{"port": 1, "executable": "/a", "x": 1}]}, {"local_ports": {"port": 1}}]:
        full = {"never_read": [], "never_write": [], "terminal_apps": [], "local_ports": [], **lists}
        errors.append({"lists": full, "out": outcome(lambda: protection.Protection(
            TMP / "home", TMP / "ferbhome", full["never_read"], full["never_write"], full["terminal_apps"],
            local_ports=full["local_ports"]).rules())})
    return [{"trees": out, "workspace": bad, "errors": errors, "browser_stores": list(protection.BROWSER_STORES),
             "default_never_read": list(protection.DEFAULT_NEVER_READ),
             "never_read_offered": list(protection.NEVER_READ_OFFERED)}]


# ---------- 15. small modules ----------

def small_cases() -> dict[str, Any]:
    env = services.Environment([("a", "b")], [("chrome", "Default", "Me"), ("brave", "P 1", "Work", "me@x.com"),
                                              ("safari", "x", "Ünï", "")])
    return {
        "blocked": [blocked.blocked(*a) for a in [(blocked.REMINDERS, blocked.NOT_IN_WORDS, "nothing was set",
                                                   "write it yourself."), ("  a  b. ", "x.", "y..", "")]],
        "escape": [escape.escape_key(*a) for a in [("read:~/.ssh", "ls ~/.ssh", "/x"), ("admin", "é \"q\"", "")]],
        "services": list(services.summary(env, "27.1", {"com.b", "com.a", "ü.x"})),
        "targets": [targets.Targets.from_json(d).to_json() for d in
                    [{"apps": ["b", "a", 1], "sites": "x", "paths": None}, '{"recipients": ["x"]}', None]],
        "owner_named": [{"scope": s, "text": t, "out": standing.owner_named(
            __import__("ferb.orchestrator.plans", fromlist=["SendScope"]).SendScope(**s), t)} for s, t in [
            ({"capability": "x", "identity": "owner", "site": "x.example", "recipients": ["a@b.co"]},
             "post on x.example to a@b.co"),
            ({"capability": "x", "identity": "owner", "site": "x.example"}, "post somewhere"),
            ({"capability": "x", "identity": "owner"}, "anything")]],
    }


# ---------- write ----------

def write(name: str, data: Any) -> None:
    text = json.dumps(data, ensure_ascii=False, indent=None, sort_keys=False, separators=(",", ":"))
    text = text.replace(os.path.realpath(TMP), "<RTMP>").replace(str(TMP), "<TMP>")
    (HERE / name).write_text(text + "\n", encoding="utf-8")


def main() -> None:
    run_harvest()
    write("text.json", text_cases())
    write("provenance.json", provenance_cases())
    write("confirm.json", confirm_pure_cases())
    decided = decide_cases(harvested("confirm.decide"))
    write("decide.json", decided)
    write("sink.json", sink_cases(harvested("confirm.url_private_values") + harvested("confirm.url_carries_private"),
                                  harvested("confirm.text_private_values") + harvested("confirm.text_carries_private")))
    by_rules: dict[str, list[Any]] = {}
    for d in decided:
        if d["out"].get("ok"):
            by_rules.setdefault("|".join(n["rule"] for n in d["out"]["ok"]), []).append(d["out"]["ok"])
    need_sets = [v for group in by_rules.values() for v in group[:4]] + rule_need_sets()
    write("wording.json", wording_cases(need_sets, harvested("confirm.summary") + harvested("confirm.card")
                                        + harvested("confirm.line")))
    write("terminal.json", terminal_cases(harvested("terminal.surface"), harvested("terminal.need"),
                                          harvested("terminal.run_control")))
    write("tls.json", tls_cases(harvested("tls.certificate_checks_off"), harvested("tls.applescript_checks_off")))
    write("payments.json", payments_cases())
    write("carried.json", carried_cases(harvested("carried.derive")))
    write("rules.json", rules_cases())
    write("foreground.json", foreground_cases(harvested("confirm.foreground_need")))
    write("stores.json", stores_cases())
    write("protection.json", protection_cases())
    write("small.json", small_cases())
    counts = {k: len(v) for k, v in sorted(HARVESTED.items())}
    write("harvest.json", counts)
    print("harvested:", counts)


if __name__ == "__main__":
    main()
