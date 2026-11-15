"""The second half of the browser hands' fixtures (driver operations, the tool layer, profiles), for make.py.

    keys.json      the extension's own keyEvent / entersText (protocol.js under node, js_keys.js) on a corpus of key
                   specs, with Python's enters_text beside it: the daemon follows the extension (the other end).
    money.json     ext_bridge.label_amount and page_amounts (GATE-578) on invented labels and page texts.
    mask.json      ferb/act/evidence/mask.py's pure parts: PageState.parse, page_masks, to_image_boxes, and
                   ext_bridge.painted with the image library (Quartz) replaced by a recorder.
"""

import dataclasses
import json
import subprocess
import tempfile
from pathlib import Path
from typing import Any

HERE = Path(__file__).parent
FIXTURES = ["keys", "money", "mask", "mime", "profiles"]          # written by make.py, in this order
REPO = HERE.parents[2]


# ---------- keys ----------

KEY_MODS = ["", "shift+", "cmd+", "Cmd+", "META+", "ctrl+", "alt+", "opt+", "option+", "control+", "command+",
            "cmd+shift+", "Shift+Cmd+", "hyper+", "x+", "constructor+", "__proto__+", "toString+", "Key+"]
KEY_KEYS = ["a", "A", "v", "V", "z", "Z", "1", "!", "é", "É", "ß", "Enter", "enter", "RETURN", "Space", "space", " ",
            "Tab", "Backspace", "delete", "Del", "ArrowLeft", "left", "Up", "F5", "f5", "-", "_", "+", ".", "`", "~", "€",
            "\U0001F600", "ab", " ", "K", "bacKspace", "constructor", "Constructor", "__proto__", "toString",
            "valueOf", "hasOwnProperty", "TOSTRING", "@", "#", "|", "\\", "'", '"', "Escape", "esc", "Home", "pageup",
            "Insert", "F12"]
KEY_EXTRA = [None, "", " ", "+", "++", "a+", "+a", "a++", "cmd++", "cmd+ +v", " cmd + v ", "﻿a", "a﻿", " a",
             "a\x1c", "\x1ca", "　cmd+v", "cmd +v", "cmd+ ", "shift", "cmd", "cmd+", "+cmd", "Cmd+V",
             "cmd+shift+v", "hyper+a", "x+a", "a" * 3, "shift+Space", "cmd+Space", "shift+Enter", "cmd+Enter", "cmd+a",
             "cmd+shift+z", "alt+Backspace", "cmd+ArrowUp", "shift+ArrowLeft", "alt+shift+ArrowLeft", "Escape", "\t", "\n",
             "\r", "İ", "cmd+İ", "Σ", "ctrl+control+v", "meta+command+v"]


def keys_fixture() -> list[dict[str, Any]]:
    from ferb.act.browser import ext_bridge
    specs = KEY_EXTRA + [m + k for m in KEY_MODS for k in KEY_KEYS]
    with tempfile.TemporaryDirectory() as tmp:
        path = Path(tmp) / "specs.json"
        path.write_text(json.dumps(specs))
        r = subprocess.run(["node", str(HERE / "js_keys.js"), str(REPO / "extensions" / "chromium" / "protocol.js"),
                            str(path)], capture_output=True, text=True, timeout=60)
    if r.returncode != 0:
        raise SystemExit(f"node failed: {r.stderr}")
    out = json.loads(r.stdout)
    for case in out:
        case["python"] = ext_bridge.enters_text(case["spec"])
    return out


# ---------- money (GATE-578) ----------

LABELS = [None, "", "Pay ₹248.75", "Pay $5", "Pay $5 or $6", "Pay $5 and $5", "Total: 1,299.00 EUR", "EUR 12", "EUR12",
          "12EUR", "12 EUR", "12 EURO", "USD5", "USD 5", "Rs. 500", "Rs500", "Rs.500.5", "₹ 1,00,000.00", "¥1000", "1000¥",
          "€ 3.999", "3.999 €", "a12 USD", "_12 USD", "x$12", "$12.", "$,12", "$1,", "Buy for 9,99 €", "٣٤ USD",
          "USD ٣٤", "２０ USD", "²3 USD", "$ 5", "$ 5", "$\x1c5", "5 €", "pay £3 now", "₿5", "THB 200 ฿",
          "Pay " + "$1 " * 3, "x" * 290 + " $12345", "x" * 298 + " $1", "Pay CHF 10.5", "kr 10", "10 SEKS", "10 SEK.",
          "SGD-5", "$5USD", "5USD$", "R$ 10", "Rs 5", "RS 5", "rs 5", "₹₹5", "$$5", "USDUSD 5"]
PAGES = [[], ["", None], ["Total $5", "Pay $5", "Fee $ 5"], ["a $1 b $2 c $1"], ["€ 1", "€1", "€  1"],
         ["Subtotal 1,000.00 INR\nTax 18.00 INR\nTotal 1,018.00 INR"], [" ".join(f"${i}" for i in range(250))],
         ["$5 USD", "5 USD", "5 USD"], ["no money here 12 34"], ["Pay ₹248.75"] * 3]


def money_fixture() -> dict[str, Any]:
    from ferb.act.browser.ext_bridge import label_amount, page_amounts
    return {"label_amount": [{"label": label, "amount": label_amount(label)} for label in LABELS],
            "page_amounts": [{"texts": texts, "amounts": list(page_amounts(texts))} for texts in PAGES]}


# ---------- masking (L13): the geometry only; Quartz is the platform's ----------

def _state(top: str = "doc", viewport: Any = (0, 0, 1280, 900), items: Any = ()) -> dict[str, Any]:
    return {"top": top, "viewport": list(viewport), "items": [dict(i) if isinstance(i, dict) else i for i in items]}


def _item(key: str, rect: Any, paint: bool = True, child: Any = None) -> dict[str, Any]:
    out = {"key": key, "rect": list(rect), "paint": paint}
    if child is not None:
        out["child"] = child
    return out


PARSE = [None, [], {}, {"top": "doc"}, {"top": 1, "viewport": [0, 0, 1, 1], "items": []},
         _state(), _state(viewport=(0, 0, 0, 900)), _state(viewport=(0, 0, 10)), _state(viewport=(0, 0, 10, True)),
         _state(viewport=(1.5, 2, 3.25, 4)),
         _state(items=[_item("field|doc|0", (1, 2, 3, 4))]),
         _state(items=[{"key": "k", "rect": [1, 2, 3, 4], "paint": 1}]),
         _state(items=[{"key": "k", "rect": [1, 2, 3, 4], "paint": True, "child": 5}]),
         _state(items=[{"key": "k", "rect": [1, 2, 3], "paint": True}]),
         _state(items=["x"]), _state(items=[_item("frame|doc|1", (0, 0, 100, 100), False, "child-doc")])]

A, B, C = (10, 10, 50, 20), (0, 0, 200, 200), (20, 15, 5, 5)
PAIRS = [
    (_state(), _state()),
    (_state(items=[_item("f|1", A)]), _state(items=[_item("f|1", A)])),
    (_state(top="doc"), _state(top="doc2")),
    (_state(viewport=(0, 0, 1280, 900)), _state(viewport=(0, 1, 1280, 900))),
    (_state(items=[_item("f|1", A)]), _state(items=[_item("f|1", (11, 10, 50, 20))])),
    (_state(items=[_item("f|1", A, False)]), _state(items=[_item("f|1", A, False)])),
    (_state(items=[_item("f|1", A, False, "x")]), _state(items=[_item("f|1", A, False, "y")])),
    (_state(items=[_item("f|1", A, False)]), _state(items=[_item("f|1", A, True)])),
    (_state(items=[_item("fr|1", B, True), _item("f|2", C)]), _state(items=[_item("fr|1", B, True)])),
    (_state(items=[_item("f|2", C)]), _state()),
    (_state(), _state(items=[_item("f|2", C, False)])),
    (_state(items=[_item("b|1", B), _item("a|1", A)]), _state(items=[_item("a|1", A), _item("b|1", B)])),
]
BOXES = [([A, B, C], (0, 0, 1280, 900), (2560, 1800)), ([A], (5, 5, 0, 10), (100, 100)),
         ([(-5, -5, 10, 10), (1275, 895, 10, 10), (0, 0, 0, 5)], (0, 0, 1280, 900), (1280, 900)),
         ([(0.3, 0.7, 10.2, 3.3)], (0.1, 0.2, 333.3, 222.2), (1000, 667)), ([(100, 100, 5, 5)], (0, 0, 50, 50), (10, 10))]


def mask_fixture() -> dict[str, Any]:
    from ferb.act.browser import ext_bridge
    from ferb.act.evidence import mask
    from ferb.core.errors import FerbError

    def parsed(raw: Any) -> Any:
        s = mask.PageState.parse(raw)
        return None if s is None else {"top": s.top, "viewport": list(s.viewport),
                                       "items": [{"key": i.key, "rect": list(i.rect), "paint": i.paint, "child": i.child}
                                                 for i in s.items]}

    def masks(before: Any, after: Any) -> Any:
        try:
            rects, viewport = mask.page_masks(mask.PageState.parse(before), mask.PageState.parse(after))
            return {"rects": [list(r) for r in rects], "viewport": list(viewport)}
        except FerbError as e:
            return {"error": type(e).__name__, "message": str(e)}

    calls: list[Any] = []
    real = (mask.size, mask.mask_secret_fields)

    def size(image: bytes) -> tuple[int, int]:
        if image.startswith(b"bad"):
            raise ValueError("not an image")
        return 1280 * 2, 900 * 2

    def paint(image: bytes, boxes: list[Any]) -> bytes:
        calls.append([image.decode(), [list(b) for b in boxes]])
        if image.startswith(b"unpaintable"):
            raise RuntimeError("no")
        return b"painted:" + image
    mask.size, mask.mask_secret_fields = size, paint                  # type: ignore[assignment]
    try:
        painted = []
        replies = [{"withheld": "the page is a PDF"}, {}, {"mask": "x"}, {"mask": {"before": _state()}},
                   {"mask": {"before": _state(), "after": _state()}},
                   {"mask": {"before": _state(items=[_item("f|1", A)]), "after": _state(items=[_item("f|1", A)])}},
                   {"mask": {"before": _state(items=[_item("f|1", A)]), "after": _state(items=[_item("f|1", C)])}},
                   {"mask": {"before": _state(items=[_item("f|1", (2000, 2000, 5, 5))]),
                             "after": _state(items=[_item("f|1", (2000, 2000, 5, 5))])}}]
        for image in (b"jpeg", b"bad", b"unpaintable"):
            for reply in replies:
                calls.clear()
                try:
                    out: Any = {"ok": ext_bridge.painted(image, reply).decode()}
                except FerbError as e:
                    out = {"error": type(e).__name__, "message": str(e)}
                painted.append({"image": image.decode(), "reply": reply, "out": out, "calls": list(calls)})
    finally:
        mask.size, mask.mask_secret_fields = real                     # type: ignore[assignment]
    return {"parse": [{"raw": r, "state": parsed(r)} for r in PARSE],
            "page_masks": [{"before": b, "after": a, "out": masks(b, a)} for b, a in PAIRS],
            "to_image_boxes": [{"rects": [list(r) for r in rs], "bounds": list(bd), "size": list(sz),
                                "boxes": [list(x) for x in mask.to_image_boxes(rs, bd, sz)]} for rs, bd, sz in BOXES],
            "painted": painted}


# ---------- a file's type for an upload through the page (mimetypes.guess_type, Python's own table) ----------

def mime_table() -> None:
    """src/mime_table.rs: Python's built-in mimetypes tables (strict types, suffixes, encodings), which
    `mimetypes.guess_type` reads besides the Mac's /etc/apache2/mime.types (not read: see src/mime.rs)."""
    import mimetypes
    import sys

    def pairs(name: str, d: dict[str, str]) -> str:
        body = "".join(f"    ({json.dumps(k)}, {json.dumps(v)}),\n" for k, v in sorted(d.items()))
        return f"pub const {name}: [(&str, &str); {len(d)}] = [\n{body}];\n"
    text = ("//! Generated by rust/fixtures/browser/hands.py from Python " + sys.version.split()[0]
            + "'s mimetypes defaults. Do not edit.\n\n"
            + pairs("TYPES", mimetypes._types_map_default) + "\n"            # type: ignore[attr-defined]
            + pairs("SUFFIXES", mimetypes._suffix_map_default) + "\n"        # type: ignore[attr-defined]
            + pairs("ENCODINGS", mimetypes._encodings_map_default))          # type: ignore[attr-defined]
    (REPO / "rust" / "crates" / "ferb-browser" / "src" / "mime_table.rs").write_text(text)


MIME_NAMES = ["a.pdf", "A.PDF", "x.tar.gz", "x.tgz", "x.svgz", "x.gz", "x.GZ", ".pdf", "noext", "a.b.json", "a.",
              "résumé.docx", "photo.JPG", "clip.mp4", "data.csv", "x.unknownext", "x.Z", "x.tar.Z", "a:b.txt",
              "data:text/plain,hi", "x.br", "x.tbz2", "notes.md", "page.html", "a.mjs", "a.webp", "a.heic", "..",
              "a.txt.gz.gz"]


# an invented mime.types file, read as mimetypes.init reads /etc/apache2/mime.types (later lines win)
MIME_FILE = ("# comment line\napplication/x-invented\tinv invx\n\napplication/vnd.openxmlformats-officedocument."
             "wordprocessingml.document\tdocx\ntext/x-override pdf # a comment after\nbad-line-without-suffixes\n"
             "image/x-two\tjpg\t# trailing\n  text/indented   md  \n#text/x-hidden hid\nx/y z#notcomment\n")


def mime_fixture() -> dict[str, Any]:
    import mimetypes
    with tempfile.TemporaryDirectory() as tmp:
        path = Path(tmp) / "mime.types"
        path.write_text(MIME_FILE)
        bare, read = mimetypes.MimeTypes(filenames=()), mimetypes.MimeTypes(filenames=(str(path),))
    names = MIME_NAMES + ["a.inv", "a.INVX", "a.hid", "a.z", "a.z#notcomment", "a.zz"]
    return {"file": MIME_FILE, "defaults": [{"name": n, "type": bare.guess_type(n)[0]} for n in names],
            "with_file": [{"name": n, "type": read.guess_type(n)[0]} for n in names]}


# ---------- profiles, the resolver, the extension's install requests (profiles.py, resolver.py, autoinstall.py) ----------

STORE = "plbfkifcmgkgnlfgikdeaaglgncdhkma"
UNPACKED = "jaclnphefmmgmpofnabffahdkbjjompm"


def _prefs(settings: dict[str, Any], **extra: Any) -> str:
    return json.dumps({"extensions": {"settings": settings, **extra}})


TREE: dict[str, str] = {
    "support/Google/Chrome/Local State": json.dumps({"profile": {"last_used": "Profile 1", "info_cache": {
        "Default": {"name": "Person 1", "user_name": "A@Example.TEST"},
        "Profile 1": {"name": "Work", "user_name": "work@example.test"},
        "Profile 3": {"name": "Home"}, "Profile 4": {"name": "Work", "user_name": "other@example.test"}}}}),
    "support/Google/Chrome/Default/Secure Preferences": _prefs({UNPACKED: {"location": 4, "path": "/x/ext"}}),
    "support/Google/Chrome/Default/Preferences": _prefs({UNPACKED: {"manifest": {"version": "0.19.10"}}}),
    "support/Google/Chrome/Profile 1/Preferences": _prefs({STORE: {"location": 1, "path": f"{STORE}/0.19.9_0",
                                                                   "disable_reasons": [1, 8192, True, -3]}}),
    "support/Google/Chrome/Profile 3/Preferences": _prefs({STORE: {"location": 1, "path": f"{STORE}/x_0",
                                                                   "state": 0}},
                                                          external_uninstalls=[]),
    "support/Google/Chrome/Profile 3/Extensions/" + STORE + "/0.19.8_0/manifest.json": "{}",
    "support/Google/Chrome/Profile 3/Extensions/" + STORE + "/0.19.10_0/manifest.json": "{}",
    "support/Google/Chrome/Profile 3/Extensions/" + STORE + "/junk/manifest.json": "{}",
    "support/Google/Chrome/Profile 4/Preferences": _prefs({STORE: {"state": 2, "location": 1, "path": "p"}},
                                                          external_uninstalls=[STORE]),
    "support/BraveSoftware/Brave-Browser/Local State": json.dumps({"profile": {"info_cache": {
        "Default": {"name": "Brave Main"}}}}),
    "support/BraveSoftware/Brave-Browser/Default/Preferences": _prefs({UNPACKED: {"location": True, "path": "p"},
                                                                       STORE: {"location": 1, "path": "q",
                                                                               "disable_reasons": 6}}),
    "support/Microsoft Edge/Local State": json.dumps({"profile": {"info_cache": {"Default": {"name": "Edge"}}}}),
    "home/extension-requests.json": json.dumps(["/not/a/request.json", "$ROOT/support/Vivaldi/External Extensions/"
                                                + STORE + ".json"]),
    "ext/manifest.json": json.dumps({"name": "FERB", "version": "0.19.10", "description": "d", "key": "dev-key",
                                     "manifest_version": 3, "permissions": ["tabs", "é"]}),
    "ext/background.js": "// worker\n",
    "ext/README.md": "notes\n",
    "ext/a.x": "a\n",
    "ext/a/b.js": "b\n",
    "ext/.hidden/x.js": "hidden\n",
    "ext/icons/.DS_Store": "x",
    "ext/icons/i.png": "png",
}
SPOKEN = ["Work", "work", " “Work” ", "chrome work", "chrome:Profile 1", "Google Chrome/Home", "chrome “Home”",
          "home", "work@example.test", "work", "Person 1", "a@example.test", "a", "chrome", "brave", "Brave Browser",
          "brave main", "nope", "", "Profile 3", "​Work​", "my alias", "chrome/Profile 9"]


def profiles_fixture() -> dict[str, Any]:
    from ferb.act.browser import autoinstall, profiles
    from ferb.act.browser.pairing import Binding
    from ferb.act.browser.resolver import Resolver
    from ferb.core.types import Identity
    saved = profiles.has_app
    profiles.has_app = lambda bid: bid in ("com.google.Chrome", "com.brave.Browser")      # type: ignore[assignment]
    try:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            for rel, text in TREE.items():
                p = root / rel
                p.parent.mkdir(parents=True, exist_ok=True)
                p.write_text(text.replace("$ROOT", str(root)))
            support, home = root / "support", root / "home"

            def norm(v: Any) -> Any:
                if isinstance(v, (str, Path)):
                    return str(v).replace(str(root), "$ROOT")
                if isinstance(v, (list, tuple)):
                    return [norm(x) for x in v]
                if isinstance(v, (set, frozenset)):
                    return sorted(norm(x) for x in v)
                if isinstance(v, dict):
                    return {k: norm(x) for k, x in v.items()}
                if dataclasses.is_dataclass(v):
                    return norm(dataclasses.asdict(v))
                return v
            out: dict[str, Any] = {"tree": TREE, "store": STORE, "unpacked": UNPACKED}
            out["installed"] = profiles.installed(support)
            found = {b: profiles.discover(b, profiles.data_dir(b, support)) for b in ("chrome", "brave", "edge", "opera")}
            out["discover"] = norm(found)
            out["last_used"] = {b: profiles.last_used(b, profiles.data_dir(b, support)) for b in ("chrome", "brave", "opera")}
            every = [p for ps in found.values() for p in ps]
            out["with_paired"] = norm(profiles.with_paired(found["chrome"], [("brave", "Profile 7"), ("chrome", "X"),
                                                                             ("edge", "Default"), ("nosuch", "A"),
                                                                             ("brave", "Profile 7")]))
            ids = (UNPACKED, STORE)
            out["entries"] = []
            for p in every:
                d = profiles.data_dir(p.browser, support)
                e = profiles.extension_entry(p, ids, d)
                out["entries"].append(norm({"profile": [p.browser, p.folder], "entry": list(e) if e else None,
                                            "state": profiles.extension_state(p, frozenset(ids), d),
                                            "version": profiles.extension_version(e[1], d / p.folder, e[0]) if e else "",
                                            "reasons": profiles.disable_reasons(e[1]) if e else []}))
            aliases = {"my alias": ("chrome", "Profile 3")}
            out["resolve"] = [norm({"spoken": s, "browser": b, "r": dataclasses.asdict(profiles.resolve(s, every, aliases, b)),
                                    "message": profiles.resolve(s, every, aliases, b).message(s, every)})
                              for s in SPOKEN for b in (None, "chrome")]
            out["shown"] = [profiles.shown(p, label) for p in every for label in (None, "", "Chrome")]
            out["named"] = norm([profiles.named(every, "chrome", "Profile 1"), profiles.named(every, "chrome", "Gone")])
            out["open_command"] = []
            for args in [("chrome", "Profile 1", None, True, True, False), ("chrome", "Profile 1", None, True, False, False),
                         ("chrome", "Profile 1", "https://x/", True, False, False),
                         ("chrome", "Profile 1", "https://x/", False, False, False),
                         ("chrome", "Profile 1", None, True, False, True), ("chrome", "Nope", None, True, False, False)]:
                got: list[Any] = []
                try:
                    profiles.open_known_profile(args[0], args[1], args[2], every, launch=got.append, background=args[3],
                                                is_running=lambda b, r=args[4]: r, window=args[5])
                    res: Any = {"ok": got[0]}
                except ValueError as e:
                    res = {"error": str(e)}
                out["open_command"].append({"args": list(args), "binary": profiles.BROWSERS[args[0]].binary,
                                            "out": norm(res)})
            bindings = [Binding("i1", "chrome", "Profile 1", Identity.OWNER), Binding("i2", "chrome", "Profile 2",
                                                                                      Identity.FERB),
                        Binding("i3", "brave", "Default", Identity.OWNER)]
            out["choose"] = []
            for ferb_prof, default, unseen, profs, binds, named in [
                    (("chrome", "Profile 2"), None, None, every, bindings, n) for n in
                    [None, "Work", "work", "Home", "brave", "nope", "Profile 2", "my alias"]] + [
                    (None, ("chrome", "Profile 1"), None, every, bindings, None),
                    (None, ("chrome", "Profile 3"), None, every, bindings, None),
                    (None, ("chrome", "Gone"), None, every, bindings, None),
                    (None, None, "Full Disk Access is off", [], bindings, "Work"),
                    (None, None, "Full Disk Access is off", [], [], None),
                    (None, None, None, every, [], "Work")]:
                r = Resolver(lambda ps=profs: ps, lambda bs=binds: bs, lambda f=ferb_prof: f, lambda: aliases,
                             lambda d=default: d, lambda u=unseen: u)
                try:
                    c: Any = {"ok": dataclasses.asdict(r.choose(named))}
                except Exception as e:                           # noqa: BLE001
                    c = {"error": type(e).__name__, "message": str(e)}
                out["choose"].append(norm({"identity": ferb_prof, "default": default, "unseen": unseen,
                                           "profiles": profs is every, "bindings": len(binds), "named": named, "out": c}))
            # the install requests
            out["valid_id"] = {i: autoinstall.valid_id(i) for i in (STORE, UNPACKED, "x" * 32, STORE.upper(), STORE + "a")}
            out["native_host_dirs"] = norm(autoinstall.native_host_dirs(support, ["chrome", "brave", "opera", "chrome"]))
            paired = {("chrome", "Profile 1")}
            out["read_states_before"] = norm(autoinstall.read_states(support, STORE, ids, paired))
            out["prepare"] = norm(autoinstall.prepare(support, STORE, ids, paired, home))
            out["record"] = norm(json.loads((home / autoinstall.REQUEST_RECORD).read_text()))
            out["request_text"] = (support / "Google/Chrome/External Extensions" / f"{STORE}.json").read_text()
            states = autoinstall.read_states(support, STORE, ids, None)
            out["read_states"] = norm(states)
            out["describe"] = [norm(autoinstall.describe(st, sid, root / "ext", w, u)) for st, sid, w, u in [
                (states, STORE, [], []), (states, "bad", [], [{"app": "Arc"}]), (states, "", [root / "x.json"], []),
                ([], STORE, [], [{"app": "Comet"}]), (autoinstall.prepare(support, STORE, ids, {("chrome", "Default")}), STORE,
                                                      [], [])]]
            out["withdraw_keep"] = norm(autoinstall.withdraw(home, STORE))
            out["record_after"] = norm(json.loads((home / autoinstall.REQUEST_RECORD).read_text()))
            out["remove_requests"] = norm(autoinstall.remove_requests(home, support, STORE))
            out["record_gone"] = (home / autoinstall.REQUEST_RECORD).exists()
            zipped, notes = autoinstall.package(root / "ext", root / "out")
            import zipfile
            with zipfile.ZipFile(zipped) as z:
                out["package"] = {"zip": norm(zipped), "notes": notes,
                                  "files": [{"name": i.filename, "text": z.read(i).decode()} for i in z.infolist()]}
            return out
    finally:
        profiles.has_app = saved                                                  # type: ignore[assignment]
