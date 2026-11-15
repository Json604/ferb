"""Fixtures for the Rust identity port (rust/crates/ferb-identity), made by the Python identity code (ENG-3).

    cd <repo> && uv run python rust/fixtures/identity/make.py

runs ferb/identity on invented data in temporary folders (never ~/.ferb, never the Keychain, never the real helper)
and writes cases.json next to this file. The helper is never started: `subprocess.run` is replaced by a recorder, the
way tests/test_secret_helper.py replaces it, which keeps every call's argv and stdin bytes and answers as
ferb/identity/secret_helper.swift would (an in-memory stand-in for the Keychain) or with a scripted reply. The Rust
tests make the same calls against a fake helper that replays these replies and assert the same argv, the same stdin
bytes, the same results and the same owner-secret-handles.json bytes.

Every string that looks like a secret is invented; stdin bytes, replies and values are stored base64 so the gate's
`secrets` check has nothing to find. The temporary home is written as $HOME.

    make.py unseal KEY_B64 NONCE CT COMMAND ORIGIN   prints what Python's seal.unseal reads (base64), for the Rust
                                                     test that checks a Rust-made seal opens in Python
"""

import asyncio
import base64
import json
import os
import stat
import subprocess
import sys
import tempfile
import unicodedata
from pathlib import Path
from typing import Any

REPO = Path(__file__).resolve().parents[3]
sys.path.insert(0, str(REPO))

import ferb.act.mac.browsers as browsers  # noqa: E402
import ferb.identity.helper as helper  # noqa: E402
import ferb.policy.urls as urls  # noqa: E402
from ferb.core.errors import Refused  # noqa: E402
from ferb.core.types import SecretRef  # noqa: E402
from ferb.identity import mailbox, owner_secrets, seal, secrets, seed  # noqa: E402
from ferb.identity.owner_secrets import OwnerSecrets  # noqa: E402
from ferb.identity.secret_tools import SecretsList  # noqa: E402

REQUIREMENT = 'identifier "com.ferb.secrets" and certificate leaf = H"abc"'


def b64(data: bytes | str) -> str:
    return base64.b64encode(data.encode() if isinstance(data, str) else data).decode()


# ---------- the two seams: what ferb.policy.urls and ferb.act.mac.browsers answered ----------

URL_HOST: dict[str, str | None] = {}
IS_BROWSER: dict[str, bool] = {}
_parse, _is_browser = urls.parse, browsers.is_browser


def parse(url: str) -> Any:
    parsed = _parse(url)
    URL_HOST[url] = parsed.host if parsed is not None and parsed.host else None
    return parsed


def is_browser(bundle_id: str) -> bool:
    IS_BROWSER[bundle_id] = _is_browser(bundle_id)      # the Launch Services list is never asked here: known ids only
    return IS_BROWSER[bundle_id]


urls.parse, browsers.is_browser = parse, is_browser


# ---------- the recorder that stands in for subprocess.run ----------

def _pipe(value: Any) -> str | None:
    return {subprocess.PIPE: "pipe", subprocess.DEVNULL: "null", None: None}[value]


class Recorder:
    def __init__(self, home: Path) -> None:
        self.home, self.calls, self.items = home, [], {}
        self.codesign, self.script = 0, []

    def keychain(self, request: dict[str, Any]) -> tuple[int, bytes]:
        """ferb/identity/secret_helper.swift's answers, with a dict for the Keychain."""
        def reply(out: dict[str, Any]) -> tuple[int, bytes]:
            return 0, json.dumps(out, separators=(",", ":"), ensure_ascii=False).encode()
        failed = 1, b'{"error":"secret operation failed"}'
        operation = request.get("operation")
        if operation == "seal":
            return reply(seal.seal(base64.b64decode(request["key"]), request["value"], request["command"],
                                   request["origin"]))
        service, account = request.get("service"), request.get("account")
        if service not in ("ferb", "ferb.owner-secrets") or not isinstance(account, str):
            return failed
        key = (service, account)
        if operation == "store" and isinstance(request.get("value"), str):
            self.items[key] = request["value"]
            return reply({"ok": True})
        if operation == "delete":
            return reply({"deleted": self.items.pop(key, None) is not None})
        if operation == "present":
            return reply({"present": key in self.items})
        if operation == "lookup" and key in self.items:
            return reply({"value": self.items[key]})
        return failed

    def __call__(self, argv: list[str], **kwargs: Any) -> subprocess.CompletedProcess[bytes]:
        given = kwargs.get("input")
        if argv[0] == "/usr/bin/codesign":
            code, out = self.codesign, b""
        elif self.script:
            code, out = self.script.pop(0)
        else:
            code, out = self.keychain(json.loads(given))
        self.calls.append({"argv": [a.replace(str(self.home), "$HOME") for a in argv],
                           "stdin": None if given is None else b64(given), "stdout": _pipe(kwargs.get("stdout")),
                           "stderr": _pipe(kwargs.get("stderr")), "close_fds": kwargs.get("close_fds"),
                           "env_given": "env" in kwargs, "code": code, "reply": b64(out)})
        return subprocess.CompletedProcess(argv, code, out, b"")

    def taken(self) -> list[dict[str, Any]]:
        out, self.calls = self.calls, []
        return out


class Home:
    """A temporary home with a stand-in helper file and its requirement, and the recorder in place of subprocess.run."""

    def __init__(self, helper_files: bool = True) -> None:
        self.helper_files = helper_files

    def __enter__(self) -> "Home":
        self.folder = tempfile.TemporaryDirectory()
        self.path = Path(self.folder.name).resolve()
        if self.helper_files:
            (self.path / "bin").mkdir()
            (self.path / "bin/ferb-secrets").write_text("binary")
            (self.path / "bin/ferb-secrets.requirement").write_text(REQUIREMENT + "\n")
        self.rec = Recorder(self.path)
        self.told: list[dict[str, Any]] = []
        self._run, self._hook = subprocess.run, helper.on_failure
        subprocess.run, helper.on_failure = self.rec, self.told.append      # type: ignore[assignment]
        return self

    def __exit__(self, *exc: Any) -> None:
        subprocess.run, helper.on_failure = self._run, self._hook            # type: ignore[assignment]
        self.folder.cleanup()

    def told_taken(self) -> list[dict[str, Any]]:
        out = list(self.told)
        self.told.clear()
        return out


def outcome(call: Any, secret: bool = False) -> dict[str, Any]:
    """What a call gave: its result (base64 of its JSON when it may hold a value), or the refusal in Python's words."""
    try:
        result = call()
    except helper.HelperFailed as e:
        return {"refused": str(e), "facts": e.facts}
    except Refused as e:
        return {"refused": str(e)}
    except Exception as e:                              # not a refusal: Python lets it through
        return {"raised": type(e).__name__}
    return {"ok_b64": b64(json.dumps(result))} if secret else {"ok": result}


# ---------- json.dumps ----------

def pyjson_cases() -> list[dict[str, Any]]:
    values: list[Any] = [
        {}, [], "", None, True, False, 0, -7, 2 ** 53 + 1, {"a": [1, {"b": None}], "c": "d"},
        "plain", 'quote " and \\ backslash', "tab\tnewline\ncr\rbs\bff\f", "\x00\x01\x1f\x7f", "caf\u00e9 \u00b7 na\u00efve",
        "\u2028\u2029 \ufeff", "astral \U0001f510 \U00010000 \U0010ffff", "https://login.example \u00b7 password",
        {"version": 2, "records": {"https://x.example": {"username": None, "sites": [], "entries": {"p": "a \u00b7 p"}}},
         "unusable": []}, "</script>", "/",
    ]
    return [{"value": v, "dumps": b64(json.dumps(v))} for v in values]


# ---------- origins and places ----------

ORIGINS = [
    "https://www.bookslots.example/en-US/niv/login?x=1", "https://WWW.BookSlots.Example", "http://www.bookslots.example",
    "bookslots.example", "https://x.example:99999/login", "https://", "https:", "https:///path", "", "   ",
    "  https://padded.example/a  ", "\t\nhttps://ctl.example", "https://tab\tbed.example", "HTTPS://Upper.Example:443/",
    "https://x.example:443", "https://x.example:8443/a", "http://x.example:80", "http://x.example:8080",
    "https://x.example:0", "https://x.example:", "https://x.example:65535", "https://x.example:65536",
    "https://x.example:-1", "https://x.example:8a", "https://x.example:\u0663", "https://x.example:\u00b2",
    "https://user:pw@x.example:444/p", "https://user@x.example", "https://a@b@x.example", "https://x.example#frag",
    "https://x.example?q=1", "https://x.example\\evil.example", "https://[::1]", "https://[::1]:8443/x",
    "https://[2001:DB8::1]", "https://[fe80::1%25en0]", "https://[fe80::1%En0]:9", "https://[::ffff:1.2.3.4]",
    "https://[1.2.3.4]", "https://[::1", "https://::1]", "https://[v1.x]", "https://[vg.x]", "https://[::1]x",
    "https://x[::1]", "https://[]", "https://[:::1]", "https://[12345::1]", "https://[::1%]",
    "https://b\u00fccher.example", "https://B\u00dcCHER.example", "https://\u0130stanbul.example",
    "https://\u03a3\u039f\u03a6\u039f\u03a3.example", "https://x\uff0fy.example", "https://x\u2100.example",
    "https://x\uff1a80.example", "https://x\uff03.example", "https://x\uff20.example", "https://x\uff1f.example",
    "https://\u2168.example", "https://\ufb01.example", "https://e\u0301.example", "ftp://files.example",
    "app:com.apple.Notes", "app:", "  app:com.apple.Notes ", "1https://x.example", "ht tps://x.example",
    "https+x.y-z://h.example", "//x.example/path", "https:/x.example", "https:x.example", "\u00e9://x.example",
    "https://x.example.", "https://x.example:443:444", "https://x.example:80:", "https://x%41.example",
    "https://x.example\x1f", "\x1fhttps://x.example", "https://%E2%84%80.example",
]

SITES = [
    "bookslots.example", "BookSlots.Example.", "https://appauth.b2clogin.example/login", "www.bookslots.example",
    "app:com.apple.Notes", "", "  ", "https://b\u00fccher.example", "x.example:8443", "http://x.example/path",
    "https://[::1]", "not a host", "https://x.example..", "ftp://files.example/a",
]

SAME_SITE = [("", ""), ("www.", ""), ("", "www."), ("bookslots.example", ""), ("", "bookslots.example"),
             ("www.bookslots.example", "bookslots.example"), ("ais.bookslots.example", "bookslots.example"),
             ("www.www.x.example", "www.x.example"), ("x.example", "x.example"), ("X.example", "x.example")]

CARD = [None, "card_number", "card_csc", "card_expiry", "secret", "other"]


class Typed:
    def __init__(self, tokens: list[str], hosts: list[str]) -> None:
        self.tokens, self.hosts = set(tokens), set(hosts)


TYPED = [("bookslots.example", ["book", "bookslots.example"], []), ("www.bookslots.example", [], ["bookslots.example"]),
         ("bookslots.example", [], ["www.bookslots.example"]), ("bookslots.example", ["slots"], ["x.example"]),
         ("", ["www."], [""]), ("www.", ["www."], [])]


def places_cases() -> dict[str, Any]:
    def origin(text: str) -> dict[str, Any]:
        try:
            normalized: str | None = secrets.normalize_origin(text)
        except ValueError:
            normalized = None                              # Python raises ValueError
        return {"in": text, "normalize_origin": normalized, "login_place": owner_secrets.login_place(text)}
    return {
        "origins": [origin(t) for t in ORIGINS],
        "site_host": [{"in": t, "out": owner_secrets.site_host(t)} for t in SITES],
        "same_site": [{"a": a, "b": b, "out": owner_secrets.same_site(a, b)} for a, b in SAME_SITE],
        "card_mismatch": [{"saved": s, "field": f, "out": owner_secrets.card_mismatch(s, f)} for s in CARD for f in CARD],
        "typed_site": [{"host": h, "tokens": t, "hosts": hs, "out": owner_secrets.typed_site(h, Typed(t, hs))}
                       for h, t, hs in TYPED],
    }


def nfkc_delimiters() -> list[int]:
    """Every character outside ASCII whose NFKC form holds one of / ? # @ : (urllib.parse._checknetloc refuses a
    netloc with one), from Python's own Unicode tables."""
    return [cp for cp in range(0x80, 0x110000) if not 0xD800 <= cp <= 0xDFFF
            and any(c in unicodedata.normalize("NFKC", chr(cp)) for c in "/?#@:")]


# ---------- the helper caller ----------

OWNER, FERB = "ferb.owner-secrets", "ferb"
ACCOUNT = "https://login.example \u00b7 password"
FAILED = (1, b'{"error":"secret operation failed"}')
LONG = "User   interaction\tis\nnot allowed. " + "x" * 250

HELPER = [
    # name, request body, scripted replies, codesign's exit code, helper files present
    ("store", {"operation": "store", "service": OWNER, "account": ACCOUNT, "value": "invented canary \u00e9\U0001f510\"\\\n"},
     [(0, b'{"ok":true}')], 0, True),
    ("lookup", {"operation": "lookup", "service": OWNER, "account": ACCOUNT},
     [(0, '{"value":"invented \\/ value \u00e9 \\ud83d\\udd10\\n"}'.encode())], 0, True),
    ("lookup with no value in the reply", {"operation": "lookup", "service": FERB, "account": "api.sealed/invented"},
     [(0, b'{}')], 0, True),
    ("present", {"operation": "present", "service": FERB, "account": "ferb/email/password"}, [(0, b'{"present":true}')], 0, True),
    ("absent", {"operation": "present", "service": FERB, "account": "ferb/email/password"}, [(0, b'{"present":false}')], 0, True),
    ("delete", {"operation": "delete", "service": OWNER, "account": ACCOUNT}, [(0, b'{"deleted":true}')], 0, True),
    ("seal", {"operation": "seal", "key": b64(bytes(range(32))), "value": "invented fill", "command": "cmd-1",
              "origin": "https://login.example"}, [(0, b'{"nonce":"AAAA","ct":"BBBB"}')], 0, True),
    ("the Keychain's status and text", {"operation": "lookup", "service": OWNER, "account": ACCOUNT},
     [(1, b'{"error":"keychain","status":-25308,"message":"User interaction is not allowed."}')], 0, True),
    ("a long message is cut and its spaces joined", {"operation": "store", "service": OWNER, "account": ACCOUNT, "value": "v"},
     [(1, json.dumps({"error": "keychain", "status": -25293, "message": LONG}).encode())], 0, True),
    ("a status with no message", {"operation": "delete", "service": OWNER, "account": ACCOUNT},
     [(1, b'{"error":"keychain","status":-25300}')], 0, True),
    ("a status with a message that is no text", {"operation": "delete", "service": OWNER, "account": ACCOUNT},
     [(1, b'{"error":"keychain","status":0,"message":5}')], 0, True),
    ("true is no status", {"operation": "lookup", "service": OWNER, "account": ACCOUNT},
     [(1, b'{"error":"keychain","status":true}'), (0, b'{"present":true}')], 0, True),
    ("a fraction is no status", {"operation": "lookup", "service": OWNER, "account": ACCOUNT},
     [(1, b'{"error":"keychain","status":-25308.0}'), (0, b'{"present":false}')], 0, True),
    ("a failed lookup, the item is there", {"operation": "lookup", "service": OWNER, "account": ACCOUNT},
     [FAILED, (0, b'{"present":true}')], 0, True),
    ("a failed lookup, the item is not shown", {"operation": "lookup", "service": OWNER, "account": ACCOUNT},
     [FAILED, (0, b'{"present":false}')], 0, True),
    ("a failed lookup, the check fails too", {"operation": "lookup", "service": OWNER, "account": ACCOUNT},
     [FAILED, FAILED], 0, True),
    ("a failed lookup, the check says 1", {"operation": "lookup", "service": OWNER, "account": ACCOUNT},
     [FAILED, (0, b'{"present":1}')], 0, True),
    ("a failed lookup, the check is not JSON", {"operation": "lookup", "service": OWNER, "account": ACCOUNT},
     [FAILED, (0, b'present')], 0, True),
    ("a failed lookup with no account is not checked", {"operation": "lookup", "service": OWNER, "account": ""},
     [FAILED], 0, True),
    ("a failed store gives no reason", {"operation": "store", "service": OWNER, "account": ACCOUNT, "value": "v"},
     [FAILED], 0, True),
    ("an error with exit 0", {"operation": "present", "service": FERB, "account": "a"}, [(0, b'{"error":"x"}')], 0, True),
    ("an empty error with exit 0 is no error", {"operation": "present", "service": FERB, "account": "a"},
     [(0, b'{"error":"","present":true}')], 0, True),
    ("a zero error with exit 0 is no error", {"operation": "present", "service": FERB, "account": "a"},
     [(0, b'{"error":0}')], 0, True),
    ("a list error with exit 0", {"operation": "present", "service": FERB, "account": "a"},
     [(0, b'{"error":[0]}')], 0, True),
    ("a reply that is a list", {"operation": "present", "service": FERB, "account": "a"}, [(0, b'[1]')], 0, True),
    ("a reply that is not JSON", {"operation": "present", "service": FERB, "account": "a"}, [(0, b'ok')], 0, True),
    ("an empty reply", {"operation": "present", "service": FERB, "account": "a"}, [(0, b'')], 0, True),
    ("a reply that is not UTF-8", {"operation": "present", "service": FERB, "account": "a"}, [(0, b'{"a":"\xff"}')], 0, True),
    ("exit 1 and no JSON", {"operation": "delete", "service": FERB, "account": "a"}, [(1, b'')], 0, True),
    ("killed by a signal", {"operation": "delete", "service": FERB, "account": "a"}, [(-9, b'{"deleted":true}')], 0, True),
    ("the signature does not match", {"operation": "store", "service": OWNER, "account": ACCOUNT, "value": "invented canary"},
     [], 1, True),
    ("no helper installed", {"operation": "lookup", "service": OWNER, "account": ACCOUNT}, [], 0, False),
]


def helper_cases() -> list[dict[str, Any]]:
    out = []
    for name, body, script, codesign, files in HELPER:
        with Home(files) as home:
            home.rec.codesign, home.rec.script = codesign, list(script)
            result = outcome(lambda: helper.request(dict(body), home.path), secret=True)
            assert not home.rec.script, name
            out.append({"name": name, "body_b64": b64(json.dumps(body)), "codesign": codesign, "helper_files": files,
                        "result": result, "calls": home.rec.taken(), "told": home.told_taken()})
    with Home() as home:                                # helper.seal: the reply's nonce and ct only
        home.rec.script = [(0, b'{"nonce":"AAAA","ct":"BBBB","extra":1}')]
        sealed = helper.seal(home.path, bytes(range(32)), "invented fill", "cmd-1", "https://login.example")
        out.append({"name": "helper.seal", "seal": {"key": b64(bytes(range(32))), "value": b64("invented fill"),
                                                    "command": "cmd-1", "origin": "https://login.example"},
                    "result": {"ok": sealed}, "calls": home.rec.taken(), "told": []})
    return out


# ---------- owner secrets ----------

PERMIT, ATLAS, BARE = "https://www.bookslots.example", "https://appauth.b2clogin.example", "https://bookslots.example"
NOTES, CHROME = "app:com.apple.Notes", "app:com.google.Chrome"
QUESTION = "What is your favorite food? \u00b7 caf\u00e9"


def step(op: str, **args: Any) -> dict[str, Any]:
    return {"op": op, **args}


FLOW = [
    step("listing", place=PERMIT),
    step("remember", name="password", place=PERMIT, value="first-invented"),
    step("remember", name="password", place=PERMIT, value="second-invented \u00e9\U0001f510"),
    step("remember", name=QUESTION, place=PERMIT, value="invented answer"),
    step("reveal", name="password", place=PERMIT),
    step("reveal", name="password", place="https://attacker.example"),
    step("reveal", name="missing", place=PERMIT),
    step("set_username", place=PERMIT, username="Asha Rao", source="memory:item-1"),
    step("listing", place=PERMIT),
    step("listing", place=PERMIT, holding=[]),
    step("listing", place=PERMIT, holding=["memory:item-1"]),
    step("remember", name="password", place=ATLAS, value="v", sites=["bookslots.example", ATLAS + "/login",
                                                                     "BookSlots.Example."]),
    step("records"),
    step("listing", place=PERMIT),
    step("listing", place=BARE),
    step("listing", place="https://ais.bookslots.example"),
    step("listing", place="https://x.example", sites=["bookslots.example"]),
    step("listing", place=ATLAS),
    step("account", place=ATLAS),
    step("account", place=ATLAS, holding=["memory:other"]),
    step("username", place=ATLAS),
    step("username", place="https://x.example", sites=["www.bookslots.example"]),
    step("reveal", name="password", place=PERMIT, kind="card_number"),
    step("remember", name="card", place="https://shop.example", value="4111 invented", kind="card_number"),
    step("remember", name="csc", place="https://shop.example", value="000", kind="card_csc"),
    step("listing", place="https://shop.example"),
    step("reveal", name="card", place="https://shop.example", kind="card_number"),
    step("reveal", name="card", place="https://shop.example", kind="card_csc"),
    step("reveal", name="card", place="https://shop.example"),
    step("reveal", name="csc", place="https://shop.example", kind="card_expiry"),
    step("remember", name="card", place="https://shop.example", value="plain now"),
    step("remember", name="card", place="https://shop.example", value="x", kind="not a kind"),
    step("inventory"),
    step("remember", name="password", place=CHROME, value="invented for a browser"),
    step("reveal", name="password", place=CHROME),
    step("set_username", place=CHROME, username="Asha Rao", source="memory:item-2"),
    step("set_username", place="http://bookslots.example", username="Asha Rao", source="memory:item-2"),
    step("set_username", place=PERMIT + "/login", username="Asha Rao", source="memory:item-2"),
    step("set_username", place=BARE, username="A. Rao", source="memory:item-2"),
    step("set_username", place=NOTES, username="asha", source="memory:item-3"),
    step("account", place=ATLAS),
    step("replace_usernames", sources=["memory:item-2"], username="asha.rao", source="memory:item-4"),
    step("replace_usernames", sources=["memory:none"], username="nobody", source="memory:item-5"),
    step("clear_usernames", sources=["memory:item-1", "memory:item-3"]),
    step("clear_usernames", sources=["memory:item-1"]),
    step("records"),
    step("delete", place=PERMIT, name="missing"),
    step("delete", place="https://shop.example", name="csc"),
    step("delete", place="https://shop.example", name="card"),
    step("delete", place=PERMIT, name=QUESTION),
    step("delete", place=PERMIT, name="password"),
    step("delete", place=ATLAS, name="password"),
    step("inventory"),
    step("records"),
    step("unusable"),
]

OLD = {"US permit password (Chrome)": CHROME, "US permit password": PERMIT, "notes": NOTES,
       PERMIT + " \u00b7 password": "https://other.example", "shared": BARE}
MIGRATION = [
    step("records"),
    step("unusable"),
    step("inventory"),
    step("listing", place=PERMIT),
    step("reveal", name="US permit password", place=PERMIT),
    step("reveal", name="US permit password (Chrome)", place=CHROME),
    step("clear_usernames", sources=["memory:none"]),
    step("remember", name="password", place=PERMIT, value="new"),          # its account is a version-1 handle's
    step("delete", place=PERMIT, name="nothing"),
    step("remember", name="other", place=PERMIT, value="new-web"),
    step("listing", place=NOTES),
    step("delete", place=CHROME, name="US permit password (Chrome)"),
    step("delete", place=NOTES, name="notes"),
    step("records"),
]

EXTRA = {"version": 2, "later": {"kept": False}, "unusable": [{"name": "old", "place": CHROME, "why": "w", "more": "m"}],
         "records": {PERMIT: {"note": [1, "two", None], "entries": {"password": "shared item", "pin": "shared item"},
                              "username": "Asha Rao"},
                     BARE: {"entries": {}, "sites": ["z.example", "a.example"], "kinds": {"gone": "card_csc"},
                            "username_source": "memory:1", "username": ""}}}
EXTRA_FLOW = [
    step("records"),
    step("listing", place=PERMIT),
    step("listing", place=PERMIT, holding=["memory:1"]),
    step("inventory"),
    step("delete", place=PERMIT, name="pin"),                               # the item another entry names stays
    step("remember", name="new", place=BARE, value="v", sites=["m.example", "www.bookslots.example", "app:x.y.z"]),
    step("delete", place=CHROME, name="old"),
    step("delete", place=PERMIT, name="password"),
    step("records"),
]

FAILING = [
    step("remember", name="password", place=PERMIT, value="v", script=[(1, b'{"error":"keychain","status":-25308,'
                                                                           b'"message":"User interaction is not allowed."}')]),
    step("records"),
    step("remember", name="password", place=PERMIT, value="v"),
    step("reveal", name="password", place=PERMIT, script=[FAILED, (0, b'{"present":true}')]),
    step("reveal", name="password", place=PERMIT, script=[(0, b'{}')]),
    step("delete", place=PERMIT, name="password", script=[FAILED]),
    step("records"),
    step("delete", place=PERMIT, name="password", script=[(0, b'{"deleted":false}')]),
    step("records"),
]

UNREADABLE = [b"{not json", b'["a", "b"]', b'{"version": 3, "records": {}}', b'"text"', b"7", b"", b"\xff\xfe{}",
              b'{"version": 2, "records": {"https://x.example": {"entries": ["password"]}}}',
              b'{"version": 2, "records": {"https://x.example": {"entries": {}, "sites": [1]}}}',
              b'{"version": 2, "records": {"https://x.example": {"entries": {}, "sites": "a"}}}',
              b'{"version": 2, "records": {"https://x.example": {"entries": {}, "username": 5}}}',
              b'{"version": 2, "records": {"https://x.example": {"entries": {}, "username_source": []}}}',
              b'{"version": 2, "records": {"https://x.example": {"entries": {}, "kinds": {"a": 1}}}}',
              b'{"version": 2, "records": {"https://x.example": {"entries": {"a": null}}}}',
              b'{"version": 2, "records": {"https://x.example": {}}}',
              b'{"version": 2, "records": {"https://x.example": []}}',
              b'{"version": 2, "records": []}', b'{"version": 2}', b'{"version": 2, "records": {}, "unusable": {}}',
              b'{"version": 2, "records": {}, "unusable": [{"name": "a", "place": "b"}]}',
              b'{"version": 2, "records": {}, "unusable": [{"name": "a", "place": "b", "why": 1}]}',
              b'{"version": 2, "records": {}, "unusable": ["a"]}',
              b'{"version": "2", "records": {}}', b'{"version": true, "records": {}}', b'{"a": "b", "c": 1}']
READABLE = [b"{}", b'{"version": 2, "records": {}}', b'{"version": 2.0, "records": {}, "unusable": []}',
            b'{"a": "https://x.example", "a": "https://y.example"}', b' {"version": 2, "records": {}} \n',
            b'{"version": "https://x.example"}']
PROBES = [step("listing", place=PERMIT), step("reveal", name="password", place=PERMIT),
          step("remember", name="password", place=PERMIT, value="x"),
          step("set_username", place=PERMIT, username="Asha Rao", source="memory:x"),
          step("clear_usernames", sources=["memory:x"]), step("inventory"), step("delete", place=PERMIT, name="password")]


def run_step(home: Home, s: dict[str, Any]) -> dict[str, Any]:
    store, op = OwnerSecrets(home.path), s["op"]
    holding = None if s.get("holding") is None else set(s["holding"])
    sites = s.get("sites", [])
    home.rec.script = list(s.get("script", []))
    calls: dict[str, Any] = {
        "listing": lambda: store.listing(s["place"], sites, holding),
        "remember": lambda: store.remember(s["name"], s["place"], s["value"], sites, s.get("kind")),
        "reveal": lambda: store.reveal(s["name"], s["place"], s.get("kind")),
        "set_username": lambda: store.set_username(s["place"], s["username"], s["source"]),
        "replace_usernames": lambda: store.replace_usernames(set(s["sources"]), s.get("username"), s.get("source")),
        "clear_usernames": lambda: store.clear_usernames(set(s["sources"])),
        "inventory": store.inventory, "records": store.records, "unusable": store.unusable,
        "delete": lambda: store.delete(s["place"], s["name"]),
        "account": lambda: list(store.account(s["place"], sites, holding)),
        "username": lambda: store.username(s["place"], sites),
    }
    result = outcome(calls[op], secret=op == "reveal")
    assert not home.rec.script, s
    shown = {k: v for k, v in s.items() if k not in ("value", "script")}
    if "value" in s:
        shown["value_b64"] = b64(s["value"])
    path = store.path
    return {**shown, "result": result, "calls": home.rec.taken(), "told": home.told_taken(),
            "file": b64(path.read_bytes()) if path.is_file() else None,
            "mode": oct(stat.S_IMODE(path.stat().st_mode)) if path.is_file() else None,
            "left": sorted(p.name for p in home.path.iterdir())}


def owner_cases() -> dict[str, Any]:
    def flow(steps: list[dict[str, Any]], start: bytes | None = None, items: dict[str, str] | None = None) -> dict[str, Any]:
        with Home() as home:
            if start is not None:
                (home.path / "owner-secret-handles.json").write_bytes(start)
            for account, value in (items or {}).items():
                home.rec.items[("ferb.owner-secrets", account)] = value
            return {"start": None if start is None else b64(start), "steps": [run_step(home, s) for s in steps]}
    flows = {"a fresh home": flow(FLOW),
             "a version-1 file": flow(MIGRATION, json.dumps(OLD).encode(), {k: "old " + k for k in OLD}),
             "a version-2 file with fields this FERB doesn't know": flow(EXTRA_FLOW, json.dumps(EXTRA).encode(),
                                                                        {"shared item": "v", "old": "o"}),
             "the helper fails": flow(FAILING)}
    files = [{"content": b64(content), "readable": readable,
              "steps": [flow([probe], content)["steps"][0] for probe in PROBES]}
             for readable, contents in ((False, UNREADABLE), (True, READABLE)) for content in contents]
    with Home() as home:                                # a folder where the file should be
        (home.path / "owner-secret-handles.json").mkdir()
        folder = [run_step(home, probe) for probe in PROBES[:2]]
    return {"flows": flows, "files": files, "folder": folder}


# ---------- configured secrets (ferb/identity/secrets.py) ----------

CONFIG = {"ferb/email/password": {"allowed_origins": ["https://Accounts.Example:443/login", "https://mail.example"],
                                  "account_hint": "ferb@example.com"},
          "bare": {}, "odd": {"allowed_origins": ["https://x.example:8443", 5, True, None], "account_hint": 7}}
ALLOWED = [("https://accounts.example", "ferb/email/password"), ("https://accounts.example:443/x", "ferb/email/password"),
           ("https://mail.example:444", "ferb/email/password"), ("", "ferb/email/password"),
           ("http://accounts.example", "ferb/email/password"), ("https://x.example", "bare"),
           ("https://x.example:8443/a", "odd"), ("https://x.example:99999", "ferb/email/password")]


def secrets_cases() -> dict[str, Any]:
    specs = secrets.specs_from_config(CONFIG)
    shown = [{"key": s.ref.key, "allowed_origins": s.allowed_origins, "account_hint": s.account_hint}
             for s in specs.values()]

    def allowed(origin: str, key: str) -> Any:
        try:
            return secrets.origin_allowed(origin, specs[key])
        except ValueError:
            return "ValueError"
    store_cases = []
    for name, key, reveal, script in [
            ("present", "ferb/email/password", False, [(0, b'{"present":true}')]),
            ("absent", "ferb/email/password", False, [(0, b'{"present":false}')]),
            ("present fails", "ferb/email/password", False, [FAILED]),
            ("not configured, present", "nothing", False, []),
            ("not configured, reveal", "nothing", True, []),
            ("reveal", "ferb/email/password", True, [(0, b'{"value":"invented password\\n"}')]),
            ("reveal with no value", "bare", True, [(0, b'{}')]),
            ("reveal refused by the Keychain", "bare", True,
             [(1, b'{"error":"keychain","status":-25308,"message":"User interaction is not allowed."}')]),
            ("reveal, not shown", "bare", True, [FAILED, (0, b'{"present":false}')]),
            ("reveal, an invalid reply", "bare", True, [(0, b'[]')])]:
        with Home() as home:
            home.rec.script = list(script)
            os.environ["FERB_HOME"] = str(home.path)        # secrets.py asks the helper for the default home
            try:
                store = secrets.KeychainSecretStore(lambda: specs)
                ref = SecretRef(key)
                if reveal:
                    try:
                        result: dict[str, Any] = {"value_b64": b64(asyncio.run(store._reveal(ref)))}
                    except secrets.SecretUnavailable as e:
                        result = {"unavailable": str(e), "reason": e.reason}
                else:
                    result = {"present": asyncio.run(store.present(ref))}
            finally:
                del os.environ["FERB_HOME"]
            store_cases.append({"name": name, "key": key, "reveal": reveal, "has": store.has(ref), "result": result,
                                "calls": home.rec.taken(), "told": home.told_taken()})
    return {"config": CONFIG, "specs": shown, "store": store_cases,
            "origin_allowed": [{"origin": o, "key": k, "out": allowed(o, k)} for o, k in ALLOWED]}


# ---------- seals ----------

def seal_cases() -> dict[str, Any]:
    pairing = [b"", b"k", bytes(range(32)), bytes(range(255, 155, -1))]
    key = seal.seal_key(bytes(range(32)))
    sealed = []
    for value, command, origin in [("invented fill", "cmd-1", "https://login.example"), ("", "", ""),
                                   ("caf\u00e9 \U0001f510 \u00b7", "c|d", "https://b\u00fccher.example|x"),
                                   ("x" * 300, "cmd-2", "https://mail.google.com/mail/feed/atom")]:
        box = seal.seal(key, value, command, origin)
        assert seal.unseal(key, box, command, origin) == value
        sealed.append({"value": b64(value), "command": command, "origin": origin, "aad": b64(seal.aad(command, origin)),
                       "nonce": box["nonce"], "ct": box["ct"]})
    return {"seal_key": [{"pairing": b64(p), "key": b64(seal.seal_key(p))} for p in pairing], "key": b64(key),
            "sealed": sealed}


# ---------- Chrome profiles (ferb/identity/seed.py) ----------

STATE = {"profile": {"info_cache": {
    "Profile 7": {"name": "FERB", "user_name": "FERB@Example.com"},
    "Default": {"name": "Person 1", "user_name": ""},
    "Profile 2": {"user_name": "owner@example.com"},
    "Profile 3": {"name": "Mixed", "user_name": "owner@example.com"},
    "Profile 4": {"name": "", "user_name": None},
    "Profile 5": {"name": "It's \"odd\"\n", "user_name": "second@example.com"},
}}}
PREFS = {"Profile 7": json.dumps({"account_info": [{"email": "ferb@example.com"}, {}]}),
         "Default": "{broken", "Profile 3": json.dumps({"account_info": [{"email": "Ferb@example.com"}]}),
         "Profile 4": json.dumps({"account_info": ["text"]}), "Profile 5": json.dumps({"other": 1})}
SEED_OPS = [("choose", "FERB@example.com", None), ("choose", "ferb@example.com", "FERB"),
            ("choose", "ferb@example.com", "Profile 7"), ("choose", "ferb@example.com", "Nobody's"),
            ("choose", "nobody@example.com", None), ("choose", "owner@example.com", None),
            ("choose", "owner@example.com", "Mixed"), ("choose", "owner@example.com", "Profile 2"),
            ("choose", "second@example.com", 'It\'s "odd"\n'),
            ("verify", "FERB@example.com", "Profile 7"), ("verify", "ferb@example.com", "Profile 3"),
            ("verify", "ferb@example.com", "Profile 2"), ("verify", "ferb@example.com", "Profile 9"),
            ("verify", "ferb@example.com", "it's"), ("verify", "ferb@example.com", 'a"b\'c\\d\te\x7f')]
BROKEN_STATE = [None, "{broken", "{}", '{"profile": {}}', '{"profile": {"info_cache": {}}}', b"\xff"]


def seed_cases() -> dict[str, Any]:
    def attempt(call: Any) -> dict[str, Any]:
        try:
            return {"ok": call()}
        except seed.SeedError as e:
            return {"seed_error": str(e)}
        except Exception as e:                          # Python lets it through
            return {"raised": type(e).__name__}

    def shown(p: seed.ChromeProfile) -> list[Any]:
        return [p.folder, p.name, list(p.accounts)]
    with tempfile.TemporaryDirectory() as folder:
        data = Path(folder)
        (data / "Local State").write_text(json.dumps(STATE))
        for name, text in PREFS.items():
            (data / name).mkdir()
            (data / name / "Preferences").write_text(text)
        found = seed.profiles(data)
        ops = [{"op": op, "address": address, "name": name,
                "result": attempt(lambda: shown(seed.choose(found, address, name) if op == "choose"
                                                else seed.verify(address, name, data)))}
               for op, address, name in SEED_OPS]
    broken = []
    for state in BROKEN_STATE:
        with tempfile.TemporaryDirectory() as folder:
            if state is not None:
                (Path(folder) / "Local State").write_bytes(state if isinstance(state, bytes) else state.encode())
            broken.append({"state": None if state is None else b64(state),
                           "result": attempt(lambda: [shown(p) for p in seed.profiles(Path(folder))])})
    return {"state": json.dumps(STATE), "preferences": PREFS, "profiles": [shown(p) for p in found], "ops": ops,
            "broken": broken}


# ---------- the unread-mail inventory (ferb/identity/mailbox.py) ----------

def feed(title: str, entries: str) -> str:
    return ('<?xml version="1.0" encoding="UTF-8"?>\n<feed version="0.3" xmlns="http://purl.org/atom/ns#">'
            f"<title>{title}</title><tagline>New messages</tagline><fullcount>2</fullcount>{entries}</feed>")


def entry(ident: str, sender: str, subject: str, summary: str, issued: str, tag: str = "issued") -> str:
    return (f"<entry><title>{subject}</title><summary>{summary}</summary><{tag}>{issued}</{tag}><id>{ident}</id>"
            f"<author><name>Sender</name><email>{sender}</email></author></entry>")


INBOX = "Gmail - Inbox for ferb@example.com"
FEEDS = [
    feed(INBOX, entry("tag:m1", "No-Reply@Shop.Example", "Your code is 482913", "Use 482913 or G-771204 within 10 minutes. "
                      "Order 12-3456 ref A1234 x9999 2026", "2026-11-13T10:00:00Z")
         + entry("tag:m2", "bank@example.com", "Sign-in", "code\u00a0\u0664\u0665\u0666\u0667 and 123 and 0000", "2026-11-13T10:05:00+05:30")),
    feed(INBOX, ""),
    feed("Gmail - Inbox for other@example.com", entry("tag:m3", "a@example.com", "1234", "", "2026-11-13T10:00:00Z")),
    feed("Gmail - Inbox for FERB@Example.com and ferb@example.com", ""),
    feed("Gmail - Inbox for ferb@example.com, x@example.com", ""),
    feed("Gmail - Inbox", ""),
    "<html><body>Sign in</body></html>",
    "<feed><title>unclosed",
    "junk before " + feed(INBOX, entry("tag:m4", "a@example.com", "<b>55555</b> tail 66666", "&lt;7777&gt; &amp; 88888888",
                                       "2026-11-13T10:00:00.123456Z")) + " junk after",
    feed(INBOX, entry("tag:m5", "a@example.com", "1111", "", "2026-11-13T10:00:00Z", tag="modified")),
    feed(INBOX, entry("tag:m6", "a@example.com", "1111", "", "not a date")),
    feed(INBOX, "<entry><title>4321 only</title></entry>"),
    feed(INBOX, entry("tag:m7", "a@example.com", "2222", "", "2026-11-13")),
    feed(INBOX, entry("tag:m8", "a@example.com", "2222", "", "2026-11-13 10:00")),
    feed(INBOX, entry("tag:m9", "a@example.com", "2222", "", "2026-11-13T10:00:00-08:00")),
    feed(INBOX, entry("tag:m10", "a@example.com", "2222", "", "2026-02-30T10:00:00Z")),
    feed(INBOX, entry("tag:m11", "a@example.com", "2222", "", "2026-11-13T10:00:00.5Z")),
    '<feed xmlns="http://www.w3.org/2005/Atom"><title>' + INBOX + "</title></feed>",
    "<feed><title>" + INBOX + "</title></feed>",
    "",
]


def mailbox_cases() -> list[dict[str, Any]]:
    out = []
    for text in FEEDS:
        case: dict[str, Any] = {"feed": b64(text)}
        try:
            title, messages = mailbox.parse_feed(text)
            case["parsed"] = {"title": title, "messages": [
                {"id": m.id, "sender": m.sender, "subject": b64(m.subject), "summary": b64(m.summary),
                 "issued": m.issued.isoformat()} for m in messages]}
        except Exception as e:
            case["parsed"] = {"error": type(e).__name__}

        async def fetch(url: str) -> str:
            return text
        box = mailbox.GmailFeedMailbox(fetch, "FERB@example.com")
        case["signed_in"] = asyncio.run(box.signed_in())

        def inventory() -> dict[str, Any]:
            got = asyncio.run(box.candidates())
            codes = []
            for i, candidate in enumerate(got["candidates"]):
                taken = box._candidates[candidate["candidate"]]
                assert len(candidate["candidate"]) == 24
                codes.append(b64(taken.code))
                candidate["candidate"] = f"T{i}"
            return {"shown": got, "codes": codes}
        case["candidates"] = inventory()
        box.used.add("tag:m1")
        case["candidates_after_m1_used"] = inventory()
        out.append(case)
    return out


# ---------- secrets.list ----------

def tool_cases() -> list[dict[str, Any]]:
    out = []
    with Home() as home:
        OwnerSecrets(home.path).remember("password", ATLAS, "v", sites=["bookslots.example"])
        OwnerSecrets(home.path).set_username(BARE, "Asha Rao", "memory:1")
        home.rec.taken()
        start = (home.path / "owner-secret-handles.json").read_bytes()
        for place, sites, holding in [(PERMIT + "/en-US/niv/login?x=1", [], None), ("http://www.bookslots.example", [], None),
                                      ("  " + ATLAS + "/login ", [], None), (ATLAS, [], ["memory:none"]),
                                      ("https://x.example", ["bookslots.example"], None), (NOTES, [], None),
                                      ("app:", [], None), ("bookslots.example", [], None)]:
            tool = SecretsList(lambda: home.path, lambda ctx, place: sites,
                               lambda: None if holding is None else set(holding))
            result = asyncio.run(tool.run(SecretsList.Input(place=place), None))    # type: ignore[arg-type]
            out.append({"place": place, "sites": sites, "holding": holding, "start": b64(start),
                        "content": result.content, "is_error": result.is_error})
        (home.path / "owner-secret-handles.json").write_text("{not json")
        result = asyncio.run(SecretsList(lambda: home.path).run(SecretsList.Input(place=PERMIT), None))  # type: ignore[arg-type]
        out.append({"place": PERMIT, "sites": [], "holding": None, "start": b64(b"{not json"),
                    "content": result.content, "is_error": result.is_error})
        assert not home.rec.taken()                     # a listing never asks the helper
    return out


def main(argv: list[str]) -> None:
    if argv[:1] == ["unseal"]:
        key, nonce, ct, command, origin = argv[1:6]
        print(b64(seal.unseal(base64.b64decode(key), {"nonce": nonce, "ct": ct}, command, origin)))
        return
    fixture = {
        "made_by": "rust/fixtures/identity/make.py", "python": sys.version.split()[0],
        "unicode": unicodedata.unidata_version, "requirement": REQUIREMENT,
        "constants": {"owner_service": owner_secrets.SERVICE, "ferb_service": secrets.SERVICE,
                      "version": owner_secrets.VERSION, "whole_browser": owner_secrets.WHOLE_BROWSER,
                      "card_kinds": sorted(owner_secrets.CARD_KINDS), "seal_info": seal.INFO.decode(),
                      "feed_url": mailbox.FEED_URL, "tool_name": SecretsList.name,
                      "tool_description": SecretsList.description},
        "pyjson": pyjson_cases(), "nfkc_delimiters": nfkc_delimiters(), "places": places_cases(),
        "helper": helper_cases(), "owner": owner_cases(), "secrets": secrets_cases(), "seals": seal_cases(),
        "seed": seed_cases(), "mailbox": mailbox_cases(), "tool": tool_cases(),
    }
    fixture["seams"] = {"url_host": URL_HOST, "is_browser": IS_BROWSER}     # last: every case above filled them
    out = Path(__file__).with_name("cases.json")
    out.write_text(json.dumps(fixture, indent=1) + "\n")
    print(f"wrote {out}: {len(fixture['helper'])} helper cases, "
          f"{sum(len(f['steps']) for f in fixture['owner']['flows'].values())} owner-secret steps, "
          f"{len(fixture['owner']['files'])} handle files, {len(URL_HOST)} hosts, {len(IS_BROWSER)} bundle ids")


if __name__ == "__main__":
    main(sys.argv[1:])
