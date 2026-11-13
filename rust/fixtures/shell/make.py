"""Fixtures for the Rust port of FERB's shell (rust/crates/ferb-shell), made by the Python shell code (ENG-3).

    cd <repo> && uv run python rust/fixtures/shell/make.py

Runs the real Python code of ferb/act/shell on invented data in a temporary folder (FERB_HOME and HOME point into it;
never ~/.ferb) and writes, next to this file:

- profiles.json: sandbox profile texts (sandbox.py) for many invented protection lists, folder layouts and options,
  each with the folder layout to rebuild, what the protection lists answered (the seam the Rust sandbox reads through),
  and the profile's text or ValueError. Paths are stored with the temporary root as @ROOT@ (and its resolved form as
  @REAL@), so the Rust test rebuilds the same layout in its own temporary folder.
- sockets.json: the ctypes layout of sys/proc_info.h's structs (sizes and field offsets), decoded socket answers
  (recorded ones from tests/support and invented random ones), nettop listings parsed.
- local.json: address normalization, kernel report lines, LocalAccess / direct_ports / local_listeners decisions on
  invented socket readings.
- proxy.json: address and host decisions, URL splits, TLS ClientHello server names, credential parsing, facts
  aggregation, the proxy environment, and whole exchanges: raw invented requests sent to the real EgressProxy, what a
  raw origin received and what the client got back (the Server and Date header lines dropped), with the facts after.
- pty.json, results.json, constants.json: output paging, the send line check, the clean environment, the admin argv,
  tool result composition (tools.py) on a fake manager, and the texts agents see.

Every run writes the same bytes. All data is invented.
"""

import asyncio
import base64
import ctypes
import json
import os
import random
import re
import shutil
import socket
import sys
import tempfile
import threading
import time
from dataclasses import replace
from pathlib import Path
from typing import Any
from urllib.parse import urlsplit

ROOT = Path(tempfile.mkdtemp(prefix="ferbshell", dir="/tmp"))     # its parents are the same on every Mac
REAL = Path(os.path.realpath(ROOT))
os.environ["FERB_HOME"] = str(ROOT / "ferbhome")
os.environ["HOME"] = str(ROOT / "user")
os.environ["TZ"] = "UTC"
REPO = Path(__file__).resolve().parents[3]
sys.path.insert(0, str(REPO))
HERE = Path(__file__).parent

from ferb.act.shell import listeners as listeners_mod  # noqa: E402
from ferb.act.shell import local, manager as manager_mod, proxy as proxy_mod, pty_proc, sandbox as sandbox_mod  # noqa: E402
from ferb.act.shell import sockets, tools  # noqa: E402
from ferb.act.shell.manager import ProcHandle, ProcInfo, ShellManager  # noqa: E402
from ferb.act.shell.proxy import EgressProxy, HelloReader, Unreachable  # noqa: E402
from ferb.act.shell.sandbox import Sandbox  # noqa: E402
from ferb.core.errors import Refused  # noqa: E402
from ferb.core.types import EscapeGrant, Identity, ToolContext  # noqa: E402
from ferb.policy.protection import LocalPort, Protection  # noqa: E402

assert re.fullmatch(r"[A-Za-z0-9/_-]+", str(ROOT)) and re.fullmatch(r"[A-Za-z0-9/_-]+", str(REAL)), (ROOT, REAL)
assert REAL != ROOT and str(ROOT) in str(REAL)


def enc(value: Any) -> Any:
    """Paths (in any text) with the temporary root as placeholders."""
    if isinstance(value, Path):
        value = str(value)
    if isinstance(value, str):
        return value.replace(str(REAL), "@REAL@").replace(str(ROOT), "@ROOT@")
    if isinstance(value, (list, tuple)):
        return [enc(v) for v in value]
    if isinstance(value, dict):
        return {k: enc(v) for k, v in value.items()}
    return value


def write(name: str, data: Any) -> None:
    (HERE / name).write_text(json.dumps(data, indent=1, ensure_ascii=False, sort_keys=False) + "\n")


# ---------------------------------------------------------------- profiles

class Recording:
    """A Protection whose answers to the sandbox are recorded: the seam the Rust sandbox reads through."""

    def __init__(self, p: Protection) -> None:
        self.p = p
        self.forms_now: dict[str, list[str]] = {}
        self.app_data: dict[str, bool] = {}

    def __getattr__(self, name: str) -> Any:
        return getattr(self.p, name)

    def forms_now_(self, path: Path) -> list[Path]:
        got = self.p.forms_now(path)
        self.forms_now[enc(str(path))] = enc([str(f) for f in got])
        return got

    def app_data_(self, path: Path) -> bool:
        got = self.p.app_data(path)
        self.app_data[enc(str(path))] = got
        return got


def view(p: Protection) -> dict[str, Any]:
    entry = lambda e: {"id": e.id, "path": enc(e.path), "typed": enc(e.typed)}  # noqa: E731
    return {"home": enc(p.home), "ferb_home": enc(p.ferb_home), "workspace": enc(p.workspace),
            "uploads": enc(p.uploads), "reads": [entry(e) for e in p.reads], "writes": [entry(e) for e in p.writes],
            "app_entries": enc(p.app_entries()), "service_files": enc(list(p.service_files)),
            "guarded_parents": enc(p.guarded_parents()),
            "local_ports": [{"port": lp.port, "executable": enc(lp.executable)} for lp in p.local_ports],
            "rules": enc(p.rules())}


def build(ops: list[list[str]]) -> None:
    for op in ops:
        target = ROOT / op[1]
        if op[0] == "dir":
            target.mkdir(parents=True, exist_ok=True)
        elif op[0] == "file":
            target.parent.mkdir(parents=True, exist_ok=True)
            target.write_text(op[2])
        elif op[0] == "link":
            target.parent.mkdir(parents=True, exist_ok=True)
            target.symlink_to(op[2].replace("@ROOT@", str(ROOT)))


LAYOUTS: list[dict[str, Any]] = [
    {"name": "basic",
     "ops": [["dir", "basic/home/Documents/repo"], ["dir", "basic/home/Projects"], ["dir", "basic/home/Library/Caches"],
             ["dir", "basic/home/Library/Mobile Documents/com~apple~CloudDocs"], ["dir", "basic/home/.cache"],
             ["dir", "basic/home/.ferb/workspace/uploads"], ["dir", "basic/home/.ssh"], ["dir", "basic/home/aws-real"],
             ["link", "basic/home/.aws", "aws-real"], ["dir", "basic/home/Library/Containers/com.example.app"],
             ["dir", "basic/home/.Trash"], ["file", "basic/home/Library/LaunchAgents/com.ferb.daemon.plist", "<plist/>"],
             ["dir", "basic/home/Applications/FERB.app/Contents/Resources"], ["file", "basic/home/.zshrc", "x"]],
     "home": "basic/home", "ferb": "basic/home/.ferb",
     "never_read": ["~/.ssh", "~/.aws", "~/Library/Containers/com.example.app/Data", "~/.cargo/credentials.toml",
                    "~/Library/Group Containers/group.example", "/Library/Keychains"],
     "never_write": ["~/.ssh", "~/.zshrc", "~/Library/LaunchAgents", "/Library/LaunchDaemons", "~"],
     "local_ports": [{"port": 5432, "executable": "/bin/sh"}, {"port": 6379, "executable": "/usr/bin/true"}],
     "app_bundle": "basic/home/Applications/FERB.app"},
    {"name": "linked",
     "ops": [["dir", "linked/home/Documents"], ["dir", "linked/ferb-real/workspace"],
             ["link", "linked/home/.ferb", "@ROOT@/linked/ferb-real"], ["dir", "linked/home/.gnupg"],
             ["link", "linked/home/Desktop", "Documents"], ["dir", "linked/elsewhere"],
             ["link", "linked/home/.kube", "@ROOT@/linked/elsewhere"]],
     "home": "linked/home", "ferb": "linked/home/.ferb",
     "never_read": ["~/.gnupg", "~/.kube", "~/missing/deeper"], "never_write": ["~/Desktop"],
     "local_ports": [], "app_bundle": None},
    {"name": "odd",
     "ops": [["dir", 'odd/hö me"[x]^\\/Dökümanlar'], ["dir", 'odd/hö me"[x]^\\/.ferb/workspace'],
             ["dir", 'odd/hö me"[x]^\\/se"cret'], ["dir", 'odd/hö me"[x]^\\/br]ack^et'],
             ["dir", "odd/hö me\"[x]^\\/sp ace/x"], ["dir", 'odd/hö me"[x]^\\/Ünï'],
             ["dir", 'odd/hö me"[x]^\\/Library/Group Containers/grp.x/inner']],
     "home": 'odd/hö me"[x]^\\', "ferb": 'odd/hö me"[x]^\\/.ferb',
     "never_read": ['~/se"cret', "~/br]ack^et", "~/sp ace/x", "~/Library/Group Containers/grp.x/inner"],
     "never_write": ["~/Ünï", "~/Dökümanlar"],
     "local_ports": [{"port": 8765, "executable": "/bin/sh"}], "app_bundle": 'odd/hö me"[x]^\\/Gone/FERB.app'},
]


def profile_cases(layout: dict[str, Any], prot: Protection) -> list[dict[str, Any]]:
    home = prot.home
    ids = [e.id for e in (*prot.reads, *prot.writes)]
    errors = ["ferb_state", "network", "admin", "ferb_app", "ferb_service", "read:~/nothing"]
    scan_a = {"outside": [9999, 22], "inside": [3000, 9999, 5432, 4321, 8765]}
    sock = str(ROOT / layout["name"] / 'run/ferb"d.sock')
    pins = [{"port": lp.port, "executable": str(lp.executable)} for lp in prot.local_ports]
    cases: list[dict[str, Any]] = []

    def case(**kw: Any) -> None:
        base = {"lift": None, "apps": False, "direct": [], "report": None, "events_to": None, "folders": [],
                "scan": scan_a, "proxy": 4321, "socket": sock, "app_domains": None}
        base.update(kw)
        cases.append(base)
    for lift in [None, "trustd", *ids, *errors]:
        for apps in (False, True):
            case(lift=lift, apps=apps)
    for scan in ({"outside": [], "inside": []}, scan_a, None, {"outside": None, "inside": [3000]}):
        for proxy in (None, 4321, 3000):
            for apps in (False, True):
                case(scan=scan, proxy=proxy, apps=apps, socket=None if proxy == 3000 else sock)
    case(direct=pins, report=f"ferb p-0011aabb {'0' * 32}")
    case(direct=pins, report=f"ferb p-0011aabb {'0' * 32}", proxy=pins[0]["port"] if pins else 4321)
    case(direct=pins, report=None)
    case(direct=pins, report=f"ferb p-0011aabb {'0' * 32}", apps=True)
    case(direct=pins, report='we"ird\\tag', scan=None)
    for folders in ([str(home / "Documents")], [str(home / "Projects"), "/Volumes/Ext Disk"], ["relative/folder"],
                    [str(home / "Nö ex" / "x")], [str(home) + "/./Documents/"], [str(home / "Desktop")]):
        case(folders=folders)
    case(folders=[str(home / "Documents")], lift=ids[0] if ids else None)
    for events_to in ("com.apple.Notes", 'a"b\\c'):
        case(apps=True, events_to=events_to)
    case(app_domains=["com.example.one", 'x"y'])
    case(app_domains=[], apps=True)
    return cases


def run_profiles() -> dict[str, Any]:
    out = []
    for layout in LAYOUTS:
        build(layout["ops"])
        home, ferb = ROOT / layout["home"], ROOT / layout["ferb"]
        bundle = ROOT / layout["app_bundle"] if layout["app_bundle"] else None
        prot = Protection(home, ferb, layout["never_read"], layout["never_write"], [],
                          local_ports=layout["local_ports"], app_bundle=bundle)
        rec = Recording(prot)
        rec.forms_now = {}
        rec.app_data = {}
        wrapped = type("Wrapped", (), {})()
        for attr in ("home", "ferb_home", "workspace", "uploads", "reads", "writes", "service_files", "local_ports"):
            setattr(wrapped, attr, getattr(prot, attr))
        wrapped.entry = prot.entry
        wrapped.app_entries = prot.app_entries
        wrapped.guarded_parents = prot.guarded_parents
        wrapped.forms_now = rec.forms_now_
        wrapped.app_data = rec.app_data_
        cases = []
        lines: dict[str, int] = {}
        for c in profile_cases(layout, prot):
            scan = c["scan"]
            listeners = (lambda s=scan: None if s is None or s["outside"] is None else set(s["outside"]))
            contained = (lambda s=scan: set(s["inside"]) if s is not None else set())
            sb = Sandbox(wrapped, ROOT / "profiles", lambda p=c["proxy"]: p,  # type: ignore[arg-type]
                         socket=Path(c["socket"]) if c["socket"] else None, listeners=listeners, contained=contained,
                         **({"app_domains": tuple(c["app_domains"])} if c["app_domains"] is not None else {}))
            direct = tuple(LocalPort(d["port"], Path(d["executable"])) for d in c["direct"])
            try:
                text, allowed = sb._profile(c["lift"], c["apps"], direct, c["report"], c["events_to"],
                                            tuple(c["folders"]))
                assert text.endswith("\n")
                result = {"lines": [lines.setdefault(line, len(lines)) for line in enc(text)[:-1].split("\n")],
                          "allowed": [[p.port, enc(p.executable)] for p in allowed]}
            except ValueError as e:
                result = {"error": enc(str(e))}
            cases.append({**enc(c), **result})
        out.append({"name": layout["name"], "ops": layout["ops"], "protection": view(prot), "lines": list(lines),
                    "forms_now": rec.forms_now, "app_data": rec.app_data, "cases": cases})
    rx_cases = ["/plain/path-1_x", 'q"uote', "back\\slash", "car^et", "br]acket", "[open", "dot.name", "sp ace",
                "Dökümanlar", "ü", "日本", "١٢٣", "²", "\u0345", "x\u0301", "tab\tnew\nline", "(paren)", "a*b+c?",
                "$#{}|", "\u00a0nbsp", "Ⅻ", "ǅ"]
    return {"root_note": "@ROOT@ and @REAL@ are the temporary root as written and resolved; a profile's text is its "
                         "layout's `lines` at the case's indices, each followed by a newline",
            "layouts": out,
            "rx": [[s, sandbox_mod._rx(s)] for s in rx_cases],
            "q": [[s, sandbox_mod._q(s)] for s in rx_cases],
            "app_program_rx": [[enc(f), enc(sandbox_mod._app_program_rx(Path(f)))]
                               for f in ("/Applications", "/System/Volumes/Data/Applications",
                                         str(ROOT / "odd" / 'hö me"[x]^\\' / "Applications"))]}


# ---------------------------------------------------------------- sockets

def struct_layout(cls: Any) -> dict[str, Any]:
    fields = []
    for name, *_ in cls._fields_:
        f = getattr(cls, name)
        fields.append([name, f.offset, f.size])
    return {"size": ctypes.sizeof(cls), "align": ctypes.alignment(cls), "fields": fields}


def run_sockets() -> dict[str, Any]:
    layouts = {name: struct_layout(getattr(sockets, name)) for name in
               ("_VinfoStat", "_SockbufInfo", "_InV6", "_InSockinfo", "_TcpSockinfo", "_UnSockinfo", "_Proto",
                "_SocketInfo", "_ProcFileinfo", "_SocketFdinfo", "_FdInfo")}

    def decoded(raw: bytes, pid: int) -> Any:
        try:
            s = sockets.decode(raw, pid)
        except ValueError as e:
            return {"error": str(e)}
        if s is None:
            return None
        return {"proto": s.proto, "local": list(s.local), "remote": list(s.remote) if s.remote else None,
                "state": s.state, "pid": s.pid, "queued": s.queued}
    recorded = json.loads((REPO / "tests" / "support" / "socket_fdinfo.json").read_text())
    cases = [{"name": name, "raw": hexed, "pid": 4242, "decoded": decoded(bytes.fromhex(hexed), 4242)}
             for name, hexed in recorded["records"].items()]
    rng = random.Random(20261113)
    fd = sockets._SocketFdinfo
    off = {n: getattr(sockets._SocketInfo, n).offset + fd.psi.offset for n in
           ("soi_family", "soi_kind", "soi_protocol", "soi_qlen", "soi_proto")}
    ini = sockets._InSockinfo
    tcp_state = sockets._TcpSockinfo.tcpsi_state.offset

    def put(buf: bytearray, at: int, fmt_size: int, value: int, signed: bool = True) -> None:
        buf[at:at + fmt_size] = value.to_bytes(fmt_size, "little", signed=signed)
    for i in range(160):
        buf = bytearray(rng.getrandbits(8) for _ in range(sockets.SOCKET_FDINFO_SIZE))
        if i % 8:
            put(buf, off["soi_family"], 4, rng.choice([2, 30, 30, 2, 1, 2]))
            put(buf, off["soi_kind"], 4, rng.choice([1, 2, 2, 3, 0]))
            put(buf, off["soi_protocol"], 4, rng.choice([17, 6, 17, 0]))
            put(buf, off["soi_qlen"], 2, rng.choice([0, 1, 5, -3, 300]))
            base = off["soi_proto"]
            put(buf, base + tcp_state, 4, rng.choice([0, 1, 4, 10, 11, 99, -1, 5]))
            put(buf, base + ini.insi_vflag.offset, 1, rng.choice([0, 1, 2, 3]), signed=False)
            for field in ("insi_lport", "insi_fport"):
                put(buf, base + getattr(ini, field).offset, 4, rng.choice([0, 0x5000, 0x0100, 0x1f90, 0xFFFF_1F90,
                                                                           rng.getrandbits(31)]), signed=False)
            for field in ("insi_laddr", "insi_faddr"):
                kind = rng.choice(["v4", "zero", "mapped", "linklocal", "random", "loop6"])
                raw = {"v4": bytes(12) + bytes([127, 0, 0, rng.choice([1, 9])]),
                       "zero": bytes(16),
                       "mapped": bytes(10) + b"\xff\xff" + bytes([192, 0, 2, rng.getrandbits(8)]),
                       "linklocal": b"\xfe\x80" + bytes([0, rng.choice([0, 4, 9])]) + bytes(11) + b"\x01",
                       "random": bytes(rng.getrandbits(8) for _ in range(16)),
                       "loop6": bytes(15) + b"\x01"}[kind]
                at = base + getattr(ini, field).offset
                buf[at:at + 16] = raw
        cases.append({"name": f"random {i}", "raw": bytes(buf).hex(), "pid": 100 + i,
                      "decoded": decoded(bytes(buf), 100 + i)})
    cases.append({"name": "short", "raw": bytes(791).hex(), "pid": 1, "decoded": decoded(bytes(791), 1)})
    listing = (REPO / "tests" / "support" / "nettop_listing.txt").read_text()
    texts = [listing, "", "state\nlaunchd.1,,\n", ",state,\ntcp4 *:22<->*:*,Listen,\n",
             ",state,\nevil\ntcp4 *:22<->*:*,Listen,.4242,,\n", ",state,\nlaunchd.1,,\ntcp4 *:22<->*:*,Listen,extra,\n",
             ",state,\nname with spaces.77,,\ntcp6 fe80::1%lo0.8080<->*.*,Listen,\nudp6 ::ffff:127.0.0.1.53<->*.*,,\n"
             "quic4 1.2.3.4:443<->5.6.7.8:9,Established,\ntcp4 1.2.3.4:*<->*:*,Closed,\n",
             ",state,\na.b.c.12,,\ntcp4 01.2.3.4:80<->*:*,Listen,\n",
             ",state,\nx.1,,\ntcp4 1.2.3.4:80<->*:*,WeirdState,\nudp4 0.0.0.0:5353<->*:*,,\n",
             ",state,\nx.1,,\ntcp6 ::.80<->::.0,Listen,\n", ",state,\nx.1,,\ntcp4 1.2.3.4:80<->*:*,Listen,\r\n",
             ",state,\r\nx.1,,\r\ntcp4 1.2.3.4:80<->*:*,Listen,\r\n", ",state,\nx.1,,\ntcp4 *:65536<->*:*,Listen,\n",
             ",state,\nx.1,,\ntcp4 [::1]:80<->*:*,Listen,\n", ",state,\nx.,,\n", ",state,\n.5,,\ntcp4 *:1<->*:*,Listen,\n"]

    def parsed(text: str) -> Any:
        try:
            got = sockets.parse_nettop(text)
        except ValueError as e:
            return {"error": type(e).__name__}
        if got is None:
            return None
        return [[pid, s.proto, list(s.local), list(s.remote) if s.remote else None, s.state, s.pid] for pid, s in got]
    return {"size": sockets.SOCKET_FDINFO_SIZE, "layouts": layouts, "decode": cases,
            "nettop": [{"text": t, "parsed": parsed(t)} for t in texts]}


# ---------------------------------------------------------------- local

class FakeTime:
    def __init__(self) -> None:
        self.t = 1000.0
        self.sleeps: list[float] = []

    def monotonic(self) -> float:
        return self.t

    def sleep(self, s: float) -> None:
        self.sleeps.append(s)
        self.t += s


def run_local() -> dict[str, Any]:
    ips = ["127.0.0.1", "::1", "::ffff:127.0.0.1", "::ffff:7f00:1", "fe80::1%lo0", "2001:db8::1", "2001:DB8:0:0:1::1",
           "0:0:0:0:0:0:0:1", "1:0:0:2:0:0:0:3", "1:0:0:0:2:0:0:3", "0.0.0.0", "::", "::ffff:0.0.0.0",
           "1:2:3:4:5:6:7::", "::1.2.3.4", "1::2:3:4:5:6:7", "a:b:c:d:e:f:0:0"]
    ip_cases = []
    for ip in ips:
        try:
            ip_cases.append([ip, local._ip(ip)])
        except ValueError:
            ip_cases.append([ip, None])
    TAG = f"ferb p-aaaa1111 {'a' * 32}"
    messages = [f"Sandbox: psql(4242) allow network-outbound remote:*:8765\n{TAG}",
                f"3 duplicate reports for Sandbox: psql(4242) allow network-outbound remote:*:8765\n{TAG}",
                f"1 duplicate report for Sandbox: redis cli(1) allow network-outbound localhost:6379\n{TAG}",
                f"Sandbox: curl(4245) deny(1) network-outbound remote:*:8765\n{TAG}",
                f"Sandbox: ps(ql)(4242) allow network-outbound remote:*:5432\n{TAG}",
                f"Sandbox: psql(4242) allow network-outbound remote:*:5432\nferb p-aaaa1111",
                f"  Sandbox: psql(4242) allow network-outbound remote:*:5432\n{TAG}  \n",
                f"Sandbox: psql(4242) allow network-outbound a:1:2\n{TAG}",
                f"Sandbox: psql(4242) allow network-outbound a:1:2x\n{TAG}",
                f"Sandbox: psql(4242) allow network-outbound :٨٧٦٥\n{TAG}",
                f"٣ duplicate reports for Sandbox: p(1) allow network-outbound x:12\n{TAG}",
                f"Sandbox: psql(4242) allow network-outbound remote:*:5432\n{TAG}x",
                f"Sandbox: psql(4242) allow network-outbound remote:*:5432\nferb p-AAAA {'a' * 32}",
                f"Sandbox: psql(4242) allow network-outbound remote (x):5432\n{TAG}",
                f"Sandbox: (4242) allow network-outbound remote:*:5432\n{TAG}",
                f"Sandbox: a\x1cb(4242) allow network-outbound r\x1c:1\n{TAG}",
                f"Sandbox: psql(4242) allow network-outbound remote:*:5432\n{TAG}\n",
                f"\x1cSandbox: psql(4242) allow network-outbound remote:*:5432\n{TAG}\x85",
                f"Sandbox: psql(42a) allow network-outbound remote:*:5432\n{TAG}",
                f"Sandbox: psql(4242) allow network-outbound remote:*:\n{TAG}",
                f"Sandbox: x\ty(1) allow network-outbound remote:*:99\n{TAG}"]
    reports = []
    for m in messages:
        mt = local._REPORT.fullmatch(m.strip())
        reports.append([m, None if mt is None else [mt.group(1) and int(mt.group(1)), mt.group(2), int(mt.group(3)),
                                                     mt.group(4)]])

    def row(local_: list[Any], remote: Any, state: str, pid: Any = 123, queued: int = 0) -> local.TcpSocket:
        return local.TcpSocket(tuple(local_), tuple(remote) if remote else None, state, pid, queued)

    def rows_json(rows: Any) -> Any:
        return None if rows is None else [[list(r.local), list(r.remote) if r.remote else None, r.state, r.pid,
                                           r.queued] for r in rows]
    scenarios: list[dict[str, Any]] = []
    L = ["127.0.0.1", 8765]
    C = ["127.0.0.1", 54321]
    specs = [
        ("exact peer, wrong exe", [[8765, "/usr/bin/python3"]], "connected", ["127.0.0.1", 8765], C,
         [[row(L, None, "LISTEN"), row(L, C, "ESTABLISHED", 456)]], {"123": "/usr/bin/python3", "456": "/bin/sh"}, {}),
        ("before_connect ok", [[8765, "/usr/bin/python3"]], "before", ["127.0.0.1", 8765], None,
         [[row(L, None, "LISTEN")]], {"123": "/usr/bin/python3"}, {}),
        ("before_connect none", [[8765, "/bin/x"]], "before", ["127.0.0.1", 8765], None, [None], {}, {}),
        ("before_connect empty", [[8765, "/bin/x"]], "before", ["127.0.0.1", 8765], None, [[]], {}, {}),
        ("before_connect unknown pid", [[8765, "/bin/x"]], "before", ["127.0.0.1", 8765], None,
         [[row(L, None, "LISTEN", None)]], {}, {}),
        ("before_connect any address", [[8765, "/bin/x"]], "before", ["::ffff:127.0.0.1", 8765], None,
         [[row(["*", 8765], None, "LISTEN"), row(["::", 8765], None, "LISTEN", 7), row(["10.0.0.1", 8765], None,
                                                                                         "LISTEN", 8)]],
         {"123": "/bin/x", "7": "/bin/x", "8": "/bin/sh"}, {}),
        ("before_connect exe unknown", [[8765, "/bin/x"]], "before", ["127.0.0.1", 8765], None,
         [[row(L, None, "LISTEN")]], {}, {}),
        ("queued then accepted", [[8765, "/usr/bin/python3"]], "connected", ["127.0.0.1", 8765], C,
         [[row(L, None, "LISTEN", queued=1)], [row(L, None, "LISTEN", queued=1)],
          [row(L, None, "LISTEN"), row(L, C, "ESTABLISHED")]], {"123": "/usr/bin/python3"}, {}),
        ("visible listener, nothing", [[8765, "/usr/bin/python3"]], "connected", ["127.0.0.1", 8765], C,
         [[row(L, None, "LISTEN")]], {"123": "/usr/bin/python3"}, {}),
        ("queued elsewhere", [[8765, "/usr/bin/python3"]], "connected", ["127.0.0.1", 8765], C,
         [[row(["127.0.0.1", 9999], None, "LISTEN", queued=1)]], {"123": "/usr/bin/python3"}, {}),
        ("no listener", [[8765, "/usr/bin/python3"]], "connected", ["127.0.0.1", 8765], C, [[]], {}, {}),
        ("mid accept contained", [], "connected", ["127.0.0.1", 9876], C,
         [[row(["127.0.0.1", 9876], None, "LISTEN")],
          [row(["127.0.0.1", 9876], None, "LISTEN"), row(["127.0.0.1", 9876], C, "ESTABLISHED")]],
         {"123": "/usr/bin/python3"}, {"123": True}),
        ("mid accept outside", [], "connected", ["127.0.0.1", 9876], C,
         [[row(["127.0.0.1", 9876], None, "LISTEN")],
          [row(["127.0.0.1", 9876], None, "LISTEN"), row(["127.0.0.1", 9876], C, "ESTABLISHED")]],
         {"123": "/usr/bin/python3"}, {"123": False}),
        ("never accepts", [[8765, "/usr/bin/python3"]], "connected", ["127.0.0.1", 8765], C,
         [[row(L, None, "LISTEN", queued=1)]], {"123": "/usr/bin/python3"}, {}),
        ("v6 mapped peer", [[8765, "/bin/x"]], "connected", ["::ffff:127.0.0.1", 8765], ["::ffff:127.0.0.1", 54321],
         [[row(L, C, "ESTABLISHED")]], {"123": "/bin/x"}, {}),
        ("snapshot fails", [[8765, "/bin/x"]], "connected", ["127.0.0.1", 8765], C, [None], {}, {}),
        ("unconfigured contained", [[8765, "/bin/x"]], "connected", ["127.0.0.1", 9876], C,
         [[row(["127.0.0.1", 9876], None, "LISTEN"), row(["127.0.0.1", 9876], C, "ESTABLISHED")]],
         {"123": "/usr/bin/python3"}, {"123": True}),
        ("two held rows", [[8765, "/bin/x"]], "connected", ["127.0.0.1", 8765], C,
         [[row(L, C, "ESTABLISHED", 5), row(L, C, "ESTABLISHED", 6)]], {"5": "/bin/x", "6": "/bin/y"}, {}),
    ]
    for name, pins, op, target, client, readings, exes, contained in specs:
        ft = FakeTime()
        calls: list[int] = []

        def snapshot(complete: bool = False, readings: Any = readings, calls: list[int] = calls) -> Any:
            calls.append(1)
            return readings[min(len(calls), len(readings)) - 1]   # the last reading repeats
        local.tcp_snapshot = snapshot  # type: ignore[assignment]
        local.executable_of = lambda pid, exes=exes: Path(exes[str(pid)]) if str(pid) in exes else None  # type: ignore
        local.contained = lambda pid, h, w, c=contained: c.get(str(pid), False)  # type: ignore[assignment]
        local.time = ft  # type: ignore[assignment]
        prot = Protection(ROOT / "user", ROOT / "user" / ".ferb", [], [], [],
                          local_ports=[{"port": p, "executable": e} for p, e in pins])
        access = local.LocalAccess(prot)
        try:
            if op == "before":
                access.before_connect(tuple(target))
                result: Any = {"ok": None}
            else:
                peer = access.connected(tuple(target), tuple(client))
                result = {"ok": [peer.address, peer.executable]}
        except local.LocalRefused as e:
            result = {"refused": str(e)}
        scenarios.append({"name": name, "pins": [[p, os.path.realpath(e)] for p, e in pins], "op": op,
                          "target": target, "client": client, "readings": [rows_json(r) for r in readings],
                          "executables": exes, "contained": contained, "result": result, "reads": len(calls),
                          "sleeps": ft.sleeps})
    local.time = time  # type: ignore[assignment]
    direct = []
    for name, pins, rows, exes in [
        ("match", [[8765, "/usr/bin/python3"]], [row(L, None, "LISTEN", 123)], {"123": "/usr/bin/python3"}),
        ("other exe", [[8765, "/usr/bin/python3"]], [row(L, None, "LISTEN", 123)], {"123": "/bin/sh"}),
        ("one of two", [[8765, "/usr/bin/python3"]], [row(L, None, "LISTEN", 123), row(L, None, "LISTEN", 456)],
         {"123": "/usr/bin/python3", "456": "/bin/sh"}),
        ("unknown pid", [[8765, "/usr/bin/python3"]], [row(L, None, "LISTEN", None)], {}),
        ("no listener", [[8765, "/usr/bin/python3"]], [row(L, C, "ESTABLISHED", 123)], {"123": "/usr/bin/python3"}),
        ("unreadable", [[8765, "/usr/bin/python3"]], None, {}),
        ("no pins", [], [row(L, None, "LISTEN", 123)], {"123": "/x"}),
        ("two pins", [[8765, "/usr/bin/python3"], [5432, "/bin/sh"]],
         [row(L, None, "LISTEN", 123), row(["*", 5432], None, "LISTEN", 9)], {"123": "/usr/bin/python3", "9": "/bin/sh"}),
    ]:
        local.tcp_snapshot = lambda complete=False, rows=rows: rows if complete else None  # type: ignore
        local.executable_of = lambda pid, exes=exes: Path(exes[str(pid)]) if str(pid) in exes else None  # type: ignore
        prot = Protection(ROOT / "user", ROOT / "user" / ".ferb", [], [], [],
                          local_ports=[{"port": p, "executable": e} for p, e in pins])
        got = local.direct_ports(prot)
        direct.append({"name": name, "pins": [[p, os.path.realpath(e)] for p, e in pins], "rows": rows_json(rows),
                       "executables": exes, "allowed": [[p.port, str(p.executable)] for p in got]})
    S = sockets.Socket
    listen_cases = []
    for name, socks, contained in [
        ("mixed", [S("tcp", ("127.0.0.1", 3000), None, "LISTEN", 11), S("tcp", ("*", 22), None, "LISTEN", None),
                   S("udp", ("*", 5353), None, "", 12), S("udp", ("127.0.0.1", 6000), ("127.0.0.1", 1), "", 13),
                   S("tcp", ("127.0.0.1", 3000), ("127.0.0.1", 5), "ESTABLISHED", 14),
                   S("tcp", ("::1", 3001), None, "LISTEN", 15), S("tcp", ("*", 3001), None, "LISTEN", 11),
                   S("tcp", ("*", 0), None, "LISTEN", 11)], {"11": True, "12": False, "15": False}),
        ("all inside", [S("tcp", ("127.0.0.1", 8000), None, "LISTEN", 1)], {"1": True}),
        ("none", [], {}),
        ("unreadable", None, {})]:
        sockets.complete = lambda socks=socks: socks  # type: ignore[assignment]
        listeners_mod.contained = lambda pid, h, w, c=contained: c.get(str(pid), False)  # type: ignore
        got = listeners_mod.local_listeners(ROOT, ROOT)
        listen_cases.append({"name": name, "sockets": None if socks is None else
                             [[s.proto, list(s.local), list(s.remote) if s.remote else None, s.state, s.pid, s.queued]
                              for s in socks], "contained": contained,
                             "result": None if got is None else [sorted(got[0]), sorted(got[1])]})
    return {"ip": ip_cases, "reports": reports, "access": scenarios, "direct": direct, "listeners": listen_cases,
            "accept_wait": local.ACCEPT_WAIT, "settle_reads": local.SETTLE_READS, "stream": local.STREAM}


# ---------------------------------------------------------------- proxy

def hello(sni: str | None, *, version: bytes = b"\x03\x03", extra: list[tuple[int, bytes]] | None = None,
          session: bytes = b"", ciphers: int = 2, no_ext: bool = False) -> bytes:
    """A TLS ClientHello record, built by hand (deterministic)."""
    exts = b""
    for etype, data in (extra or []):
        exts += etype.to_bytes(2, "big") + len(data).to_bytes(2, "big") + data
    if sni is not None:
        name = sni.encode()
        entry = b"\x00" + len(name).to_bytes(2, "big") + name
        sn = len(entry).to_bytes(2, "big") + entry
        exts += b"\x00\x00" + len(sn).to_bytes(2, "big") + sn
    body = version + bytes(range(32)) + bytes([len(session)]) + session
    body += (ciphers * 2).to_bytes(2, "big") + b"\x13\x01" * ciphers + b"\x01\x00"
    if not no_ext:
        body += len(exts).to_bytes(2, "big") + exts
    hs = b"\x01" + len(body).to_bytes(3, "big") + body
    return b"\x16\x03\x01" + len(hs).to_bytes(2, "big") + hs


def records(h: bytes, cut: int) -> bytes:
    """The hello's handshake bytes re-framed in records of `cut` bytes."""
    payload = h[5:]
    return b"".join(b"\x16\x03\x03" + len(payload[i:i + cut]).to_bytes(2, "big") + payload[i:i + cut]
                    for i in range(0, len(payload), cut))


def run_proxy_pure() -> dict[str, Any]:
    hosts = ["127.0.0.1", "localhost", "LOCALHOST.", "0.0.0.0", "::1", "::ffff:127.0.0.1", "127.8.9.10", "192.0.2.1",
             "2001:db8::1", "fe80::1%lo0", "::", "example.com", "01.2.3.4", "1.2.3", "256.1.1.1", "::ffff:1.2.3.4",
             "[::1]", "", "1.2.3.4%x", "::%", "::1%a%b", "1:2:3:4:5:6:7:8:9", "1::2::3", "1:2:3:4:5:6:7::",
             "::1.2.3.4", "localhost..", "١.٢.٣.٤", "0x7f.0.0.1", "127.1", "::ffff:7f00:0001", "abcd" * 12 + "::1"]
    pure = []
    for h in hosts:
        pure.append({"host": h, "literal": proxy_mod._literal(h), "this_mac": proxy_mod.this_mac(h),
                     "local_host": proxy_mod.local_host(h),
                     "local_addresses": [[fam == socket.AF_INET6, list(sa)] for fam, sa in
                                         proxy_mod.local_addresses(h, 8080)]})
    targets = ["//origin.test:443", "//Origin.TEST:443", "//origin.test", "//origin.test:0", "//origin.test:65536",
               "//origin.test:x", "//[::1]:443", "//[::1", "//[1.2.3.4]:443", "//user@host:22", "//host:٣",
               "//host:+1", "//:443", "//[v1.x]:443", "//ḦOST:9", "//a/b:1", "// host:1", "//\thost:1", "//ho\nst:1",
               "//host:00080", "//host:-1", "//xn--abc.test:1", "//[fe80::1%25lo0]:1", "//ｈost:1", "//h℅st:1",
               "//[::1]x:5", "//host:1:2"]
    connect = []
    for t in targets:
        try:
            parts = urlsplit(t)
            connect.append([t, {"host": parts.hostname, "port": parts.port}])
        except ValueError as e:
            connect.append([t, {"error": type(e).__name__}])
    urls = ["http://origin.test/x", "HTTP://origin.test:8080/a/b?q=1#frag", "https://origin.test/", "/relative",
            "http://user:pw@origin.test:81/p", "http:///nohost", "http://origin.test", "http://origin.test?only=q",
            "http://[2001:db8::1]:8080/x", "http://[2001:db8::1/x", "ftp://x/y", "http://origin.test:bad/",
            "  http://origin.test/lead", "http://or\tigin.test/x", "http://origin.test/a#b?c", "http:origin.test/x",
            "1http://x/", "http+x://h/p", "http://h/p;params?q", "http://ORIGIN.test/%20x"]
    forward = []
    for u in urls:
        try:
            parts = urlsplit(u)
            forward.append([u, {"scheme": parts.scheme, "netloc": parts.netloc, "path": parts.path,
                                "query": parts.query, "host": parts.hostname, "port": parts.port}])
        except ValueError as e:
            forward.append([u, {"error": type(e).__name__}])
    hellos = []
    variants = {"sni": hello("cdn.test"), "upper sni": hello("CDN.Test"), "no sni": hello(None),
                "no extensions": hello(None, no_ext=True), "session": hello("a.test", session=bytes(32)),
                "other ext first": hello("b.test", extra=[(0x0010, b"\x00\x03\x02h2"), (0x002b, b"\x02\x03\x04")]),
                "non ascii sni": hello("\u00e9x.test".encode("latin-1").decode("latin-1")),
                "trailing dot": hello("cdn.test."), "empty sni": hello(""),
                "server hello": b"\x16\x03\x03\x00\x04\x02\x00\x00\x00", "not handshake": b"\x17\x03\x03\x00\x01x",
                "split 7": records(hello("split.test"), 7), "split 1": records(hello("one.test"), 1),
                "truncated": hello("cdn.test")[:40], "header only": b"\x16\x03\x01",
                "bad ext len": hello("x.test")[:-3] + b"\xff\xff\xff"}
    raw_bad = bytearray(hello("bad.test"))
    raw_bad[-9] = 0xC3
    variants["invalid utf8 sni"] = bytes(raw_bad)
    for name, data in variants.items():
        feeds = []
        for step in (1, 3, 64, len(data) or 1):
            r = HelloReader()
            got = None
            for i in range(0, len(data), step):
                got = r.feed(data[i:i + step])
                if got is not None:
                    break
            feeds.append([step, got])
        hellos.append({"name": name, "bytes": data.hex(), "feeds": feeds})
    sni_bodies = []
    for body in [bytes(10), b"\x03\x03" + bytes(33), b"\x03\x03" + bytes(32) + b"\x05abc", hello("q.test")[9:],
                 hello("q.test")[9:-2], b"\x03\x03" + bytes(32) + b"\x00\x00\x02\x13\x01\x01\x00\x00\x09\x00\x00\x00\x05"
                 b"\x00\x03\x00\x00\x09xyz"]:
        sni_bodies.append([body.hex(), proxy_mod._sni_of(body)])
    # credentials: Proxy-Authorization values as the handler's _who reads them
    tokens = {"p-1": "tok-AAAA_bbbb-1234"}
    who_vals = ["Basic " + base64.b64encode(b"p-1:tok-AAAA_bbbb-1234").decode(),
                "basic  " + base64.b64encode(b"p-1:tok-AAAA_bbbb-1234").decode() + " \x1c",
                "BASIC\t" + base64.b64encode(b"p-1:tok-AAAA_bbbb-1234").decode(),
                "Bearer " + base64.b64encode(b"p-1:tok-AAAA_bbbb-1234").decode(),
                "Basic " + base64.b64encode(b"p-1:wrong").decode(), "Basic " + base64.b64encode(b"p-2:x").decode(),
                "Basic !!!", "Basic cC0xOnRvaw", "Basic cC0xOnRvaw=", "Basic cC0x=OnRvaw==",
                "Basic " + base64.b64encode("p-1:tök".encode()).decode(), "Basic " + base64.b64encode(b"\xff\xfe").decode(),
                "Basic", "", "Basic " + base64.b64encode(b"p-1tok").decode(), "Basic cA==", "Basic cB==",
                "Basic " + base64.b64encode(b"p-1:tok-AAAA_bbbb-1234").decode() + "\xa0",
                "Basic é" + base64.b64encode(b"p-1:x").decode(), "Basic Y\nA==", "Basic ====", "Basic cC0xOnRvaw==="]
    who = []
    for v in who_vals:
        scheme, _, raw = v.partition(" ")
        if scheme.lower() != "basic":
            got = None
        else:
            try:
                user, _, token = base64.b64decode(raw.strip(), validate=True).decode().partition(":")
                expected = tokens.get(user)
                try:
                    import hmac
                    got = user if expected is not None and hmac.compare_digest(expected, token) else None
                except TypeError:
                    got = None
            except (ValueError, UnicodeDecodeError):
                got = None
        who.append([v, got])
    # facts aggregation
    p = EgressProxy(lambda *a: (True, ""), excluded=lambda h: h.endswith(".hidden"))
    try:
        for proc in ("p-a", "p-b"):
            p.credentials(proc)
        p._opened("p-a", "b.test", 443)
        p._opened("p-a", "a.test", 80)
        p._opened("p-a", "a.test", 80)
        p._opened("p-a", "x.hidden", 443)
        p._opened("p-a", "y.hidden", 443)
        p._opened("p-a", "localhost", 5432, local_peer=local.LocalPeer("127.0.0.1", "/bin/pg"))
        p._opened("p-a", "localhost", 5432, local_peer=local.LocalPeer("::1", "/bin/pg"))
        p._opened("p-a", "z.hidden", 1, local_peer=local.LocalPeer("127.0.0.1", "/bin/z"))
        p._count("p-a", "a.test", 80, "bytes_in", 100)
        p._count("p-a", "a.test", 80, "bytes_out", 7)
        p._count("p-a", "x.hidden", 443, "bytes_in", 5)
        p._count("p-a", "y.hidden", 443, "bytes_in", 6)
        p._count("p-a", "nope.test", 1, "bytes_in", 6)
        p._note("p-a", "c.test", 443, refused="why one")
        p._note("p-a", "c.test", 443, refused="why one")
        p._note("p-a", "q.hidden", 443, refused="why two")
        p._note("p-a", "r.hidden", 443, refused="why two")
        p._record_sni("p-a", "sni.test", 443)
        p._record_sni("p-a", "b.test", 443)
        facts_a = p.facts("p-a")
        facts_none = p.facts("p-none")
        env = {k: v.replace(p.credentials("p-a"), "@TOKEN@").replace(str(p.port), "@PORT@")
               for k, v in p.env("p-a").items()}
    finally:
        p.close()
    p2 = EgressProxy(lambda *a: (True, ""), local_access=local.LocalAccess(
        Protection(ROOT / "user", ROOT / "user" / ".ferb", [], [], [])))
    try:
        env_local = {k: v.replace(p2.credentials("p-a"), "@TOKEN@").replace(str(p2.port), "@PORT@")
                     for k, v in p2.env("p-a").items()}
    finally:
        p2.close()
    return {"hosts": pure, "connect": connect, "forward": forward, "hellos": hellos, "sni_bodies": sni_bodies,
            "who": {"tokens": tokens, "cases": who}, "facts": {"p-a": facts_a, "none": facts_none},
            "env": env, "env_local": env_local,
            "constants": {"ENDED": proxy_mod.ENDED, "OWN": proxy_mod.OWN, "DEST_REFUSED_NO": proxy_mod.DEST_REFUSED_NO,
                          "DEST_SNI_NONE": proxy_mod.DEST_SNI_NONE, "SNI_MAX": proxy_mod.SNI_MAX,
                          "UNAUTH_TIMEOUT": proxy_mod.UNAUTH_TIMEOUT, "MAX_UNAUTH": proxy_mod.MAX_UNAUTH,
                          "MAX_HEAD": proxy_mod.MAX_HEAD, "HOP": sorted(proxy_mod.HOP)}}


class Origin:
    """One raw origin connection: records the bytes it got, then answers with canned bytes."""

    def __init__(self, mode: str, reply: bytes) -> None:
        self.mode, self.reply = mode, reply
        self.srv = socket.socket()
        self.srv.bind(("127.0.0.1", 0))
        self.srv.listen(4)
        self.port = self.srv.getsockname()[1]
        self.got: bytes | None = None
        self.stop = False
        self.thread = threading.Thread(target=self.serve, daemon=True)
        self.thread.start()

    def serve(self) -> None:
        self.srv.settimeout(0.1)
        deadline = time.monotonic() + 4
        while True:
            try:
                c, _ = self.srv.accept()
                break
            except TimeoutError:
                if self.stop or time.monotonic() > deadline:
                    return
            except OSError:
                return
        c.settimeout(4)
        buf = b""
        try:
            if self.mode == "greet":
                c.sendall(self.reply)
                self.reply = b""
            while True:
                if self.mode == "http" and complete(buf):
                    break
                chunk = c.recv(65536)
                if not chunk:
                    break
                buf += chunk
        except OSError:
            pass
        self.got = buf
        try:
            if self.reply:
                c.sendall(self.reply)
            c.shutdown(socket.SHUT_WR)
            time.sleep(0.05)
        except OSError:
            pass
        c.close()

    def close(self) -> None:
        self.stop = True
        self.thread.join(5)
        self.srv.close()


def complete(buf: bytes) -> bool:
    """A whole HTTP request in `buf` (head, then a Content-Length body or a chunked body ending in 0)."""
    if b"\r\n\r\n" not in buf:
        return False
    head, body = buf.split(b"\r\n\r\n", 1)
    m = re.search(rb"(?im)^content-length:\s*(\d+)", head)
    if m:
        return len(body) >= int(m.group(1))
    if re.search(rb"(?im)^transfer-encoding:.*chunked", head):
        return body.endswith(b"0\r\n\r\n")
    return True


def normalize(raw: bytes) -> str:
    """The proxy's own Server and Date header lines dropped; bytes as latin-1 text."""
    raw = re.sub(rb"Server: BaseHTTP/[^\r\n]*\r\n", b"", raw)
    raw = re.sub(rb"Date: [^\r\n]*\r\n", b"", raw)
    return raw.decode("latin-1")


def allow(proc: str, host: str, port: int) -> tuple[bool, str]:
    if host.endswith(".refused.test"):
        return False, "this destination needs the owner's approval"
    if host == "no.test":
        return False, proxy_mod.DEST_REFUSED_NO
    if host == "blank.test":
        return False, ""
    return True, ""


def wire_scenarios() -> list[dict[str, Any]]:
    big = b"x" * (proxy_mod.MAX_HEAD + 1)        # every chunking ends past MAX_HEAD at the same byte
    ok = b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\nKeep-Alive: timeout=5\r\nX-Up: 1\r\n\r\nhello"
    A = "Proxy-Authorization: @AUTH@\r\n"
    S: list[dict[str, Any]] = [
        {"name": "no credentials", "request": "GET http://origin.test/x HTTP/1.1\r\nHost: origin.test\r\n\r\n"},
        {"name": "wrong token", "request": "GET http://origin.test/x HTTP/1.1\r\nProxy-Authorization: Basic "
                                           + base64.b64encode(b"p-1:nope").decode() + "\r\n\r\n"},
        {"name": "head of 407", "request": "HEAD http://origin.test/x HTTP/1.1\r\n\r\n"},
        {"name": "get", "request": "GET http://origin.test/x?q=1 HTTP/1.1\r\nHost: origin.test\r\nUser-Agent: t/1\r\n"
                                   + A + "Proxy-Connection: keep-alive\r\nAccept: */*\r\nTE: trailers\r\n\r\n",
         "reply": ok},
        {"name": "get with port and userinfo", "request": "GET http://u:p@ORIGIN.test:8080/a%20b HTTP/1.1\r\n" + A
                                                          + "X-Case: Mixed\r\nHOST: ignored\r\n\r\n", "reply": ok},
        {"name": "post content length", "request": "POST http://origin.test/up HTTP/1.1\r\n" + A
                                                   + "Content-Length: 11\r\nContent-Type: text/plain\r\n\r\nhello world",
         "reply": b"HTTP/1.1 201 Created\r\nContent-Length: 2\r\nConnection: keep-alive\r\n\r\nok"},
        {"name": "post chunked", "request": "POST http://origin.test/up HTTP/1.1\r\n" + A
                                            + "Transfer-Encoding: chunked\r\n\r\n2\r\nab\r\n2;ext=1\r\ncd\r\n0\r\n"
                                              "X-Trailer: t\r\n\r\n", "reply": ok},
        {"name": "post bad chunk", "request": "POST http://origin.test/up HTTP/1.1\r\n" + A
                                              + "Transfer-Encoding: chunked\r\n\r\nzz\r\nab\r\n0\r\n\r\n", "reply": ok},
        {"name": "head", "request": "HEAD http://origin.test/h HTTP/1.1\r\n" + A + "\r\n",
         "reply": b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\n"},
        {"name": "expect continue", "request": "PUT http://origin.test/p HTTP/1.1\r\n" + A
                                               + "Expect: 100-continue\r\nContent-Length: 3\r\n\r\nabc", "reply": ok},
        {"name": "interim then final", "request": "GET http://origin.test/i HTTP/1.1\r\n" + A + "\r\n",
         "reply": b"HTTP/1.1 100 Continue\r\nX-A: 1\r\n\r\nHTTP/1.1 103 Early Hints\r\nLink: </s>\r\n\r\n" + ok},
        {"name": "upgrade 101", "request": "GET http://origin.test/ws HTTP/1.1\r\n" + A
                                           + "Connection: Upgrade\r\nUpgrade: websocket\r\nSec-WebSocket-Key: k\r\n\r\n",
         "reply": b"HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\r\nframes",
         "mode": "http"},
        {"name": "upgrade refused", "request": "GET http://origin.test/ws HTTP/1.1\r\n" + A
                                               + "Connection: keep-alive, Upgrade\r\nUpgrade: websocket\r\n\r\n",
         "reply": b"HTTP/1.1 400 Bad\r\nConnection: close\r\n\r\nno"},
        {"name": "upgrade header without connection", "request": "GET http://origin.test/ws HTTP/1.1\r\n" + A
                                                                 + "Upgrade: websocket\r\n\r\n", "reply": ok},
        {"name": "origin closes early", "request": "GET http://origin.test/e HTTP/1.1\r\n" + A + "\r\n",
         "reply": b"HTTP/1.1 200 OK\r\nX-Partial"},
        {"name": "huge head", "request": "GET http://origin.test/big HTTP/1.1\r\n" + A + "\r\n", "reply": big},
        {"name": "lf head", "request": "GET http://origin.test/lf HTTP/1.1\r\n" + A + "\r\n",
         "reply": b"HTTP/1.1 200 OK\nContent-Length: 2\n\nok\r\n\r\ntail"},
        {"name": "refused host", "request": "GET http://a.refused.test/x HTTP/1.1\r\n" + A + "\r\n"},
        {"name": "refused blank why", "request": "GET http://blank.test/x HTTP/1.1\r\n" + A + "\r\n"},
        {"name": "owner said no", "request": "CONNECT no.test:443 HTTP/1.1\r\n" + A + "\r\n"},
        {"name": "own address", "request": "GET http://127.0.0.1:9/x HTTP/1.1\r\n" + A + "\r\n"},
        {"name": "own address v6", "request": "CONNECT [::1]:22 HTTP/1.1\r\n" + A + "\r\n"},
        {"name": "localhost without local access", "request": "GET http://localhost:9/x HTTP/1.1\r\n" + A + "\r\n"},
        {"name": "unresolvable", "request": "GET http://unresolvable.test/x HTTP/1.1\r\n" + A + "\r\n"},
        {"name": "connect refused upstream", "request": "GET http://closed.test/x HTTP/1.1\r\n" + A + "\r\n"},
        {"name": "excluded host", "request": "GET http://x.hidden/x HTTP/1.1\r\n" + A + "\r\n", "reply": ok},
        {"name": "https url", "request": "GET https://origin.test/x HTTP/1.1\r\n" + A + "\r\n"},
        {"name": "relative path", "request": "GET /x HTTP/1.1\r\n" + A + "\r\n"},
        {"name": "bad port", "request": "GET http://origin.test:99999/x HTTP/1.1\r\n" + A + "\r\n"},
        {"name": "connect no port", "request": "CONNECT origin.test HTTP/1.1\r\n" + A + "\r\n"},
        {"name": "connect bad bracket", "request": "CONNECT [::1:443 HTTP/1.1\r\n" + A + "\r\n"},
        {"name": "connect tunnel", "request": "CONNECT Origin.Test:443 HTTP/1.1\r\nHost: origin.test:443\r\n" + A
                                              + "\r\nPING plain bytes\r\n", "reply": b"PONG", "mode": "tunnel",
         "shutdown": True},
        {"name": "tunnel server first", "request": "CONNECT svc.test:25 HTTP/1.1\r\n" + A + "\r\nHELO\r\n",
         "reply": b"220 greet\r\n", "mode": "greet", "shutdown": True},
        {"name": "sni match", "request": "CONNECT cdn.test:443 HTTP/1.1\r\n" + A + "\r\n", "after": hello("cdn.test"),
         "reply": b"server-hello", "mode": "tunnel", "shutdown": True, "sni": True},
        {"name": "sni trailing dot", "request": "CONNECT cdn.test:443 HTTP/1.1\r\n" + A + "\r\n",
         "after": hello("CDN.test."), "reply": b"s", "mode": "tunnel", "shutdown": True, "sni": True},
        {"name": "sni other allowed", "request": "CONNECT cdn.test:443 HTTP/1.1\r\n" + A + "\r\n",
         "after": hello("other.test"), "reply": b"s", "mode": "tunnel", "shutdown": True, "sni": True},
        {"name": "sni refused", "request": "CONNECT cdn.test:443 HTTP/1.1\r\n" + A + "\r\n",
         "after": hello("secret.refused.test"), "reply": b"s", "mode": "tunnel", "shutdown": True, "sni": True},
        {"name": "sni split records", "request": "CONNECT cdn.test:443 HTTP/1.1\r\n" + A + "\r\n",
         "after": records(hello("secret.refused.test"), 9), "reply": b"s", "mode": "tunnel", "shutdown": True,
         "sni": True},
        {"name": "no sni to a name", "request": "CONNECT cdn.test:443 HTTP/1.1\r\n" + A + "\r\n",
         "after": hello(None), "reply": b"s", "mode": "tunnel", "shutdown": True, "sni": True},
        {"name": "no sni to a literal", "request": "CONNECT 192.0.2.7:443 HTTP/1.1\r\n" + A + "\r\n",
         "after": hello(None), "reply": b"s", "mode": "tunnel", "shutdown": True, "sni": True},
        {"name": "not a hello", "request": "CONNECT cdn.test:443 HTTP/1.1\r\n" + A + "\r\n",
         "after": b"\x16\x03\x03\x00\x04\x02\x00\x00\x00", "reply": b"s", "mode": "tunnel", "shutdown": True,
         "sni": True},
        {"name": "sni not checked", "request": "CONNECT cdn.test:443 HTTP/1.1\r\n" + A + "\r\n",
         "after": hello("elsewhere.test"), "reply": b"s", "mode": "tunnel", "shutdown": True},
        {"name": "hello cut short", "request": "CONNECT cdn.test:443 HTTP/1.1\r\n" + A + "\r\n",
         "after": hello("cdn.test")[:30], "reply": b"s", "mode": "tunnel", "shutdown": True, "sni": True},
        {"name": "garbage line", "request": "GARBAGE\r\n\r\n"},
        {"name": "four words", "request": "GET a b HTTP/1.1\r\n\r\n"},
        {"name": "http 2", "request": "GET http://origin.test/ HTTP/2.0\r\n\r\n"},
        {"name": "bad version", "request": "GET http://origin.test/ HTTP/1.x\r\n\r\n"},
        {"name": "bad version quote", "request": "GET http://origin.test/ HTTP/'\xe9\x85\r\n\r\n"},
        {"name": "unsupported method", "request": "TRACE http://origin.test/ HTTP/1.1\r\n\r\n"},
        {"name": "lowercase method", "request": "get http://origin.test/ HTTP/1.1\r\n\r\n"},
        {"name": "http 0.9", "request": "GET /\r\n\r\n"},
        {"name": "http 0.9 post", "request": "POST /\r\n\r\n"},
        {"name": "http 1.0", "request": "GET http://origin.test/ten HTTP/1.0\r\n" + A + "\r\n", "reply": ok},
        {"name": "too many headers", "request": "GET http://origin.test/ HTTP/1.1\r\n"
                                                + "".join(f"X-{i}: v\r\n" for i in range(100)) + "\r\n"},
        {"name": "ninety nine headers", "request": "GET http://origin.test/n HTTP/1.1\r\n" + A
                                                   + "".join(f"X-{i}: v\r\n" for i in range(98)) + "\r\n", "reply": ok},
        {"name": "folded and odd headers", "request": "GET http://origin.test/f HTTP/1.1\r\n" + A
                                                      + "X-Fold: one\r\n two\r\nX-Latin: caf\xe9\r\n:empty\r\n"
                                                        "X-Last: z\r\n\r\n", "reply": ok},
        {"name": "header without colon ends headers", "request": "GET http://origin.test/c HTTP/1.1\r\n" + A
                                                                 + "Bad Header: x\r\nX-After: y\r\n\r\n", "reply": ok},
        {"name": "empty line", "request": "\r\n"},
        {"name": "double slash path", "request": "GET //origin.test/x HTTP/1.1\r\n" + A + "\r\n"},
        {"name": "ipv6 literal", "request": "GET http://[2001:db8::7]:8080/v6 HTTP/1.1\r\n" + A + "\r\n", "reply": ok},
    ]
    return S


def run_wire() -> list[dict[str, Any]]:
    out = []
    closed = socket.socket()
    closed.bind(("127.0.0.1", 0))
    closed_port = closed.getsockname()[1]
    closed.close()
    reports: list[Any] = []
    for sc in wire_scenarios():
        origin = Origin(sc.get("mode", "http"), sc.get("reply", b""))

        def resolve(host: str, port: int, origin: Origin = origin) -> list[Any]:
            if host == "unresolvable.test":
                raise Unreachable("couldn't resolve the name (gaierror)")
            if host == "closed.test":
                return [(socket.AF_INET, ("127.0.0.1", closed_port))]
            return [(socket.AF_INET, ("127.0.0.1", origin.port))]
        p = EgressProxy(allow, resolve=resolve, excluded=lambda h: h.endswith(".hidden"),
                        inspect_sni=(lambda *a: True) if sc.get("sni") else None,
                        on_report=lambda proc, rec: reports.append(rec))
        try:
            token = p.credentials("p-1")
            auth = "Basic " + base64.b64encode(f"p-1:{token}".encode()).decode()
            raw = sc["request"].replace("@AUTH@", auth).encode("latin-1") + sc.get("after", b"")
            c = socket.create_connection(("127.0.0.1", p.port), timeout=5)
            c.sendall(raw)
            if sc.get("shutdown"):
                time.sleep(0.2)
                c.shutdown(socket.SHUT_WR)
            got = b""
            try:
                while chunk := c.recv(65536):
                    got += chunk
            except OSError:
                pass
            c.close()
            time.sleep(0.05)
            origin.close()
            facts = p.facts("p-1")
            reports.clear()
            p.forget("p-1")
            deadline = time.monotonic() + 3
            while not reports and p.pending("p-1") and time.monotonic() < deadline:
                time.sleep(0.02)
            report = reports[0] if reports else None
        finally:
            p.close()
        request = sc["request"]
        if len(request) > 4000:
            request = {"head": request[:200], "repeat": request[200:201], "count": len(request) - 400,
                       "tail": request[-200:]}
            assert request["head"] + request["repeat"] * request["count"] + request["tail"] == sc["request"]
        reply = sc.get("reply", b"")
        out.append({"name": sc["name"], "request": request, "after": sc.get("after", b"").hex(),
                    "shutdown": bool(sc.get("shutdown")), "sni": bool(sc.get("sni")), "mode": sc.get("mode", "http"),
                    "reply": {"repeat": "x", "count": len(reply)} if reply == b"x" * len(reply) and reply else
                    reply.decode("latin-1"),
                    "client_got": normalize(got), "origin_got": None if origin.got is None else
                    origin.got.decode("latin-1"), "facts": facts, "report": report})
    return out


# ---------------------------------------------------------------- pty, manager, tools

def run_pty() -> dict[str, Any]:
    logs = {"ascii": b"hello world\n", "multi": "aé漢😀b\n".encode(), "invalid": b"ok \xff\xfe and \xe6\xbc end",
            "cut": "漢字".encode()[:-1], "empty": b""}
    reads = []
    for name, data in logs.items():
        log = ROOT / f"{name}.log"
        log.write_bytes(data)
        pp = pty_proc.PtyProc(["x"], ROOT, {}, log, str)
        pp.total = len(data)
        for since in range(-1, len(data) + 2):
            for limit in (None, 0, 1, 2, 3, 5, 100):
                text, offset, _ = pp.read(since, limit)
                reads.append([name, since, limit, text, offset])
    sends = []
    for raw, limit in [(b"a" * 1023 + b"\n", 1024), (b"a" * 1024 + b"\n", 1024), (b"abc", 4), (b"abcd", 4),
                       (b"x\n" * 3, 3), (b"ab\ncd", 3), (b"ab\nc", 3), (b"", 1), (b"\n", 1), (b"x", 1),
                       (("é" * 600 + "\n").encode(), 1024), (b"", 0)]:
        *full, last = raw.split(b"\n")
        refused = any(len(line) + 1 > limit for line in full) or len(last) > limit - 1
        sends.append([raw.hex(), limit, refused])
    environ = {"PATH": "/usr/bin:/bin:/usr/sbin:/bin", "HOME": str(ROOT / "user"), "USER": "invented",
               "LANG": "en_US.UTF-8", "SHELL": "/bin/zsh", "TMPDIR": str(ROOT / "tmp"), "SECRET_KEY": "nope",
               "FERB_RUN_TOKEN": "nope"}
    (ROOT / "user" / ".local" / "bin").mkdir(parents=True, exist_ok=True)
    saved = dict(os.environ)
    envs = []
    try:
        for env in (environ, {k: v for k, v in environ.items() if k != "PATH"}):
            os.environ.clear()
            os.environ.update(env)
            envs.append({"environ": enc(env), "clean": enc(manager_mod.clean_env(None)),
                         "with_sandbox": enc(manager_mod.clean_env(type("S", (), {"env": staticmethod(Sandbox.env)})()))})
    finally:
        os.environ.clear()
        os.environ.update(saved)
    # the admin argv (sudo, then the same sandbox), as PtyProc.start builds it
    seen: dict[str, Any] = {}

    class Stop(Exception):
        pass

    def popen(argv: list[str], **kw: Any) -> Any:
        seen["argv"] = argv
        raise Stop
    real = pty_proc.subprocess.Popen
    pty_proc.subprocess.Popen = popen  # type: ignore[assignment]
    try:
        pp = pty_proc.PtyProc(["/usr/bin/sandbox-exec", "-f", "/p/x.sb", "/bin/sh", "-c", "id -u"], ROOT,
                              {"PATH": "/usr/bin", "A": "b c"}, ROOT / "admin.log", str, admin_secret="pw")
        try:
            asyncio.run(pp.start())
        except Stop:
            pass
    finally:
        pty_proc.subprocess.Popen = real  # type: ignore[assignment]
    argv = seen["argv"]
    tty = argv[argv.index("ferb-admin") + 1]
    admin = [a.replace(tty, "@TTY@") for a in argv]
    return {"reads": reads, "logs": {k: v.hex() for k, v in logs.items()}, "sends": sends, "env": envs,
            "admin": enc(admin), "admin_supervisor": pty_proc.ADMIN_SUPERVISOR,
            "cert_bundle_exists": os.path.isfile(sandbox_mod.CERT_BUNDLE)}


class FakeShells:
    """What tools.py asks of ShellManager, scripted."""

    def __init__(self, sc: dict[str, Any]) -> None:
        self.sc = sc
        self.told: list[str] = []
        self.sandbox = type("SB", (), {"proxy_port": staticmethod(lambda: sc.get("proxy_port"))})() \
            if sc.get("sandbox", True) else None

    async def start(self, cmd: str, cwd: Path, task: str, background: bool, **kw: Any) -> ProcHandle:
        if self.sc.get("start_refused"):
            raise Refused(self.sc["start_refused"])
        return ProcHandle("p-1", task)

    def handle(self, pid: str, task: str) -> ProcHandle:
        return ProcHandle(pid, task)

    def info(self, h: ProcHandle) -> ProcInfo:
        i = self.sc["info"]
        return ProcInfo("p-1", h.owner_task, "cmd", "/w", 0.0, i["running"], i["exit_code"], False,
                        capture_error=i.get("capture_error"), sandboxed=True, read_path=i.get("read_path"),
                        sandbox_ms=i.get("sandbox_ms"), stopped=i.get("stopped"))

    def exit_told(self, h: ProcHandle) -> None:
        self.told.append(h.id)

    async def read(self, h: ProcHandle, since: int = 0, limit: int | None = None) -> tuple[str, int, int]:
        r = self.sc["read"]
        return r["text"], r["offset"], r["total"]

    async def tail(self, h: ProcHandle, limit: int) -> tuple[str, int, int]:
        r = self.sc["read"]
        return r["text"], r["offset"], r["total"]

    async def wait(self, h: ProcHandle, timeout: float) -> int | None:
        return self.sc["info"]["exit_code"]

    def network(self, h: ProcHandle) -> dict[str, Any]:
        return json.loads(json.dumps(self.sc["network"]))

    async def send(self, h: ProcHandle, text: str) -> tuple[int, int]:
        if self.sc.get("send_refused"):
            raise Refused(self.sc["send_refused"])
        return tuple(self.sc["send"])  # type: ignore[return-value]

    async def kill(self, h: ProcHandle) -> None:
        if self.sc.get("kill_refused"):
            raise Refused(self.sc["kill_refused"])

    def list(self) -> list[ProcInfo]:
        return [ProcInfo(p["id"], p["task"], p["command"], "/w", 0.0, p["running"], p["exit_code"], p["background"])
                for p in self.sc["list"]]


def run_results() -> list[dict[str, Any]]:
    net0 = {"destinations": [], "refused": []}
    net1 = {"destinations": [{"host": "b.test", "port": 443, "bytes_out": 1, "bytes_in": 2, "connections": 1},
                             {"host": "2001:db8::1", "port": 80, "bytes_out": 0, "bytes_in": 0, "connections": 1},
                             {"host": "b.test", "port": 80, "bytes_out": 0, "bytes_in": 0, "connections": 1},
                             {"host": "[::1]", "port": 80, "bytes_out": 0, "bytes_in": 0, "connections": 1}],
            "refused": [{"host": "c.test", "port": 443, "why": "other", "count": 1},
                        {"host": "n.test", "port": 443, "why": proxy_mod.DEST_REFUSED_NO, "count": 2}]}
    infos = [{"running": True, "exit_code": None, "sandbox_ms": 12.5},
             {"running": False, "exit_code": 0, "sandbox_ms": 3.0},
             {"running": False, "exit_code": 1, "capture_error": "OSError (errno 5): shell output capture failed; "
                                                                 "output is incomplete"},
             {"running": False, "exit_code": -9, "stopped": "the task's download budget is used up",
              "read_path": "/h/.aws"},
             {"running": False, "exit_code": 2, "read_path": "/h/.ssh"}]
    reads = [{"text": "out", "offset": 3, "total": 3},
             {"text": "x: Operation not permitted\n", "offset": 27, "total": 9000},
             {"text": "tail", "offset": 9000, "total": 9000}]
    out = []
    n = 0
    for tool in ("shell.run", "shell.start", "shell.wait", "shell.read"):
        for info in infos:
            for read in reads:
                for net in (net0, net1):
                    n += 1
                    if n % 3 and tool in ("shell.start",):
                        continue
                    sc = {"info": info, "read": read, "network": net}
                    out.append({"tool": tool, "scenario": sc, "args": {"command": "true", "id": "p-1", "since": 0},
                                **call(tool, sc, {"command": "true", "id": "p-1", "since": 0, "timeout_s": 5})})
    extra = [("shell.send", {"send": [5, 5]}), ("shell.send", {"send": [2, 9]}),
             ("shell.send", {"send_refused": "You have taken over this process."}),
             ("shell.kill", {}), ("shell.kill", {"kill_refused": "no process p-9 for this task"}),
             ("shell.list", {"list": [{"id": "p-1", "task": "t", "command": "a", "running": True, "exit_code": None,
                                       "background": False},
                                      {"id": "p-2", "task": "other", "command": "b", "running": False, "exit_code": 0,
                                       "background": True},
                                      {"id": "p-3", "task": "t", "command": "c", "running": False, "exit_code": 3,
                                       "background": True}]}),
             ("shell.run", {"start_refused": "working folder refused: x", "info": infos[0], "read": reads[0],
                            "network": net0}),
             ("shell.rules", {"proxy_port": 4321}), ("shell.rules", {"proxy_port": None}),
             ("shell.rules", {"sandbox": False})]
    for tool, sc in extra:
        out.append({"tool": tool, "scenario": sc, **call(tool, sc, {"id": "p-1", "text": "hello\n", "command": "true"})})
    return out


def call(tool: str, sc: dict[str, Any], raw: dict[str, Any]) -> dict[str, Any]:
    shells = FakeShells(sc)
    cls = {"shell.run": tools.ShellRun, "shell.start": tools.ShellStart, "shell.wait": tools.ShellWait,
           "shell.read": tools.ShellRead, "shell.send": tools.ShellSend, "shell.kill": tools.ShellKill,
           "shell.list": tools.ShellList, "shell.rules": tools.ShellRules}[tool]
    if tool == "shell.rules":
        prot = Protection(ROOT / "user", ROOT / "user" / ".ferb", ["~/.ssh"], ["~/.zshrc"], [],
                          local_ports=[{"port": 5432, "executable": "/bin/sh"}])
        cap = cls(shells, prot)  # type: ignore[call-arg]
    else:
        cap = cls(shells)  # type: ignore[call-arg]
    fields = cap.Input.model_fields
    args = cap.Input(**{k: v for k, v in raw.items() if k in fields})
    ws = str(ROOT / "user" / ".ferb" / "workspace")
    ctx = ToolContext(task_id="t", worker_id="w", request_id="r", identity=Identity.OWNER, scopes={"shell"},
                      device_id="d", span_id=None, workspace=ws, owner_roots=[ws], untrusted_roots=[])
    os.makedirs(ctx.workspace, exist_ok=True)
    r = asyncio.run(cap.run(args, ctx))
    return {"content": enc(r.content), "is_error": r.is_error, "read_sources": [list(s) for s in r.read_sources],
            "told": shells.told}


def run_constants() -> dict[str, Any]:
    s = sandbox_mod
    return {k: getattr(s, k) for k in ("SANDBOX_EXEC", "SECURITY_SERVER", "CERT_BUNDLE", "NESTED_SANDBOX",
                                       "APP_REFUSED", "NO_APPS", "NETWORK_FACT", "NETWORK_SERVICES", "TRUST_SERVICES",
                                       "APP_SERVICES", "OSASCRIPT", "APPS_LOCKDOWN", "APP_PREFERENCE_DOMAINS",
                                       "WINDOW_SERVER", "VOLUMES", "DATA_VOLUME", "SCOPE_FACT", "SCOPE_REFUSED",
                                       "TRASHES")} | {
        "CLEAN_ENV_KEYS": manager_mod.CLEAN_ENV_KEYS, "SEND_WAIT_S": manager_mod.SEND_WAIT_S,
        "WATCH_S": manager_mod.WATCH_S, "TAIL": tools.TAIL, "OUTPUT_ORIGIN": tools.OUTPUT_ORIGIN,
        "descriptions": {c.name: c.description for c in (tools.ShellRun, tools.ShellStart, tools.ShellWait,
                                                          tools.ShellRead, tools.ShellSend, tools.ShellKill,
                                                          tools.ShellList, tools.ShellRules)}}


def main() -> None:
    try:
        write("constants.json", run_constants())
        write("profiles.json", run_profiles())
        write("sockets.json", run_sockets())
        write("local.json", run_local())
        write("proxy.json", {**run_proxy_pure(), "wire": run_wire()})
        write("pty.json", run_pty())
        write("results.json", run_results())
    finally:
        shutil.rmtree(ROOT, ignore_errors=True)


if __name__ == "__main__":
    main()
