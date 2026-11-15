"""Fixture for the change record's Rust port (rust/crates/ferb-fs/src/changes.rs; decision: ENG-3).

Run from the repo root:  TZ=UTC uv run python rust/fixtures/fs/make_changes.py

Drives the real Python `ferb.act.fs.changes` on invented file trees in temporary folders (HOME pointed at each
scenario's own home, never the owner's) and writes changes.json:
- `scenarios`: each a list of steps, file-system operations and calls on a `Changes`, in the order the Python tests of
  tests/test_changes*.py run them; every call's output is recorded, normalized (the temporary root as $ROOT, times
  that are "now" as <now>, inode and device numbers as <id>). The Rust test replays the same steps on its own temporary
  folder and must produce the same outputs. Races are deterministic: Python patches the same moments the tests patch
  (`os.access`, `_skip`, `os.open`, `_read`), Rust sets `ferb_fs::hooks` at the same points.
- `pdf`: PDFKit's reading (`_pdf`) of every PDF the scenarios write, by SHA-256, for the Rust test's PdfText seam.
- `named`: `named()` on command texts; `difflib`: SequenceMatcher and unified_diff on generated sequences.
Writes the same bytes on every run."""

import base64
import contextlib
import dataclasses
import difflib
import hashlib
import io
import json
import os
import random
import re
import shutil
import stat
import subprocess
import sys
import tempfile
import zipfile
from pathlib import Path

REPO = Path(__file__).resolve().parents[3]
sys.path.insert(0, str(REPO))

from ferb.act.fs import changes as cm  # noqa: E402
from ferb.act.fs.changes import Change, Changes, describe, named  # noqa: E402
from ferb.policy.protection import Protection  # noqa: E402

OUT = Path(__file__).with_name("changes.json")
NOW = 1_600_000_000 * 10**9          # an mtime at or after this is "now" (the run's own time)
FIXED = 1_000_000_123 * 10**9 + 456_789   # a fixed mtime the scenarios set (2001)
TOKEN = "rt-OPAQUE-VALUE-123"
PDFS: dict[str, object] = {}

# ---------- documents (deterministic bytes) ----------

W = "http://schemas.openxmlformats.org/wordprocessingml/2006/main"


def _p(text: str) -> str:
    return f'<w:p><w:r><w:t xml:space="preserve">{text}</w:t></w:r></w:p>'


def _table(rows: list[list[str]]) -> str:
    return "<w:tbl>" + "".join("<w:tr>" + "".join(f"<w:tc>{_p(cell)}</w:tc>" for cell in row) + "</w:tr>"
                               for row in rows) + "</w:tbl>"


def docx(body: list, creator: str = "Student Name") -> bytes:
    xml = (f'<?xml version="1.0" encoding="UTF-8" standalone="yes"?><w:document xmlns:w="{W}"><w:body>' +
           "".join(_p(b) if isinstance(b, str) else _table(b) for b in body) + "</w:body></w:document>")
    core = ('<?xml version="1.0" encoding="UTF-8" standalone="yes"?><cp:coreProperties '
            'xmlns:cp="http://schemas.openxmlformats.org/package/2006/metadata/core-properties" '
            f'xmlns:dc="http://purl.org/dc/elements/1.1/"><dc:creator>{creator}</dc:creator></cp:coreProperties>')
    buf = io.BytesIO()
    with zipfile.ZipFile(buf, "w") as z:
        for name, text in (("[Content_Types].xml", "<Types/>"), ("word/document.xml", xml),
                           ("word/header1.xml", f'<w:hdr xmlns:w="{W}">{_p("Course header")}</w:hdr>'),
                           ("docProps/core.xml", core)):
            info = zipfile.ZipInfo(name, date_time=(1980, 1, 1, 0, 0, 0))
            info.compress_type = zipfile.ZIP_DEFLATED
            z.writestr(info, text)
    return buf.getvalue()


def pdf(lines: list[str], annots: list[tuple[str, str]] = ()) -> bytes:
    """A one-page PDF whose text layer holds `lines` (tests/support/documents.py `pdf`), with annotations
    (subtype, contents) drawn over its first line (the live case's "correction")."""
    text = " ".join(f"({line}) Tj 0 -20 Td" for line in lines)
    stream = f"BT /F1 12 Tf 72 720 Td {text} ET".encode()
    n_annots = len(annots)
    refs = " ".join(f"{6 + i} 0 R" for i in range(n_annots))
    page = (b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 4 0 R "
            b"/Resources << /Font << /F1 5 0 R >> >>" + (f" /Annots [{refs}]".encode() if annots else b"") + b" >>")
    objects = [b"<< /Type /Catalog /Pages 2 0 R >>", b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>", page,
               b"<< /Length %d >>\nstream\n" % len(stream) + stream + b"\nendstream",
               b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>"]
    for subtype, contents in annots:
        objects.append(f"<< /Type /Annot /Subtype /{subtype} /Rect [72 712 272 732] /Contents ({contents}) "
                       f"/DA (/Helv 12 Tf 0 g) >>".encode())
    out, offsets = bytearray(b"%PDF-1.4\n"), []
    for i, body in enumerate(objects, 1):
        offsets.append(len(out))
        out += b"%d 0 obj\n" % i + body + b"\nendobj\n"
    xref = len(out)
    out += b"xref\n0 %d\n0000000000 65535 f \n" % (len(objects) + 1)
    out += b"".join(b"%010d 00000 n \n" % o for o in offsets)
    out += b"trailer\n<< /Size %d /Root 1 0 R >>\nstartxref\n%d\n%%%%EOF\n" % (len(objects) + 1, xref)
    data = bytes(out)
    got = cm._pdf(data)
    PDFS[hashlib.sha256(data).hexdigest()] = None if got is None else [got[0], [list(a) for a in got[1]]]
    return data


def b64(data: bytes) -> str:
    return base64.b64encode(data).decode()


# ---------- normalizing ----------

def norm_text(text: str, root: str) -> str:
    text = text.replace(root, "$ROOT")
    return re.sub(r", modified (\d{4})-\d\d-\d\dT\d\d:\d\d:\d\d[+-]\d\d:\d\d(?::\d\d)?",
                  lambda m: ", modified <now>" if int(m.group(1)) >= 2020 else m.group(0), text)


def norm_value(v, root: str):
    if isinstance(v, str):
        return norm_text(v, root)
    if isinstance(v, list):
        return [norm_value(x, root) for x in v]
    if isinstance(v, dict):
        return {norm_text(k, root): norm_value(x, root) for k, x in v.items()}
    return v


def norm_change(c: Change, root: str) -> dict:
    d = norm_value(dataclasses.asdict(c), root)
    if isinstance(d["mtime_ns"], int) and d["mtime_ns"] >= NOW:
        d["mtime_ns"] = "<now>"
    return d


def norm_index(text: str | None, root: str) -> str | None:
    if text is None:
        return None
    data = json.loads(text)
    for f in data["files"].values():
        f["ino"], f["dev"] = "<id>", "<id>"
        if f["mtime_ns"] >= NOW:
            f["mtime_ns"] = "<now>"
    for listing in data["folders"].values():
        for name, e in listing.items():
            listing[name] = [e[0], e[1], "<now>" if e[2] >= NOW else e[2], "<id>"]
    return norm_text(json.dumps(data), root)


def record_text(store: Changes, task: str) -> str | None:
    rec = store._records.get(task)
    if rec is None:
        return None
    return json.dumps({"task_id": rec.task_id, "tree_id": rec.tree_id, "n": rec.n, "folders": rec.folders,
                       "named": sorted(rec.named), "files": {k: dataclasses.asdict(v) for k, v in rec.files.items()}})


def snapshot(top: Path, root: str) -> list:
    """A folder's tree: relative name, kind, size, content hash or link target, link count, mode bits."""
    out = []
    for dirpath, dirnames, filenames in os.walk(top):
        dirnames.sort()
        for name in sorted(dirnames + filenames):
            p = Path(dirpath) / name
            st = p.lstat()
            rel = str(p.relative_to(top))
            if stat.S_ISLNK(st.st_mode):
                out.append([rel, "link", norm_text(os.readlink(p), root)])
            elif stat.S_ISDIR(st.st_mode):
                out.append([rel, "dir", stat.S_IMODE(st.st_mode)])
            elif stat.S_ISREG(st.st_mode):
                data = p.read_bytes()
                if name == cm.INDEX:          # absolute paths and inode numbers: hashed as normalized
                    data = norm_index(data.decode(), root).encode()
                out.append([rel, "file", st.st_size if name != cm.INDEX else len(data),
                            hashlib.sha256(data).hexdigest(), st.st_nlink, stat.S_IMODE(st.st_mode)])
            else:
                out.append([rel, "other"])
    return out


# ---------- running a scenario ----------

def run(sc: dict) -> list:
    tmp = Path(tempfile.mkdtemp()).resolve()
    root = str(tmp)
    sub = lambda s: s.replace("$ROOT", root)                    # noqa: E731
    at = lambda rel: tmp / rel                                 # noqa: E731
    home = at(sc.get("home", "home"))
    home.mkdir(parents=True, exist_ok=True)
    os.environ["HOME"] = str(home)
    cfg = sc.get("store", {})

    def make_store() -> Changes:
        closed = cfg.get("closed")
        if closed == "protection":
            protection = Protection(home, at(cfg["ferb"]), cfg.get("never_read", ["~/.ssh"]), [], [])
            fn = lambda p: protection.read_denied(p) is not None     # noqa: E731
        elif isinstance(closed, dict):
            part = closed["part"]
            fn = lambda p: part in p.parts                            # noqa: E731
        else:
            fn = None
        return Changes(at(cfg.get("root", "changes")), closed=fn, home=home)

    store = make_store()
    outputs = []
    try:
        for step in sc["steps"]:
            op = step.get("op")
            if op is not None:
                fs_op(step, at, sub)
                continue
            outputs.append(call(step, store, at, sub, root, make_store))
            if step["call"] == "restart":
                store = make_store()
    finally:
        for dirpath, dirnames, filenames in os.walk(tmp):
            for name in dirnames + filenames:
                p = Path(dirpath) / name
                with contextlib.suppress(OSError):
                    if not p.is_symlink():
                        os.chflags(p, 0)
                        if p.is_dir():
                            os.chmod(p, 0o755)
                        else:
                            os.chmod(p, 0o644)
        shutil.rmtree(tmp, ignore_errors=True)
    return outputs


def fs_op(step: dict, at, sub) -> None:
    op = step["op"]
    p = at(step["path"]) if "path" in step else None
    if op == "mkdir":
        p.mkdir(parents=True, exist_ok=True)
    elif op == "write":
        p.write_text(step["text"])
    elif op == "append":
        with open(p, "a") as f:
            f.write(step["text"])
    elif op == "bytes":
        p.write_bytes(base64.b64decode(step["b64"]))
    elif op == "symlink":
        p.symlink_to(sub(step["target"]))
    elif op == "link":
        os.link(at(step["target"]), p)
    elif op == "rename":
        os.rename(at(step["from"]), at(step["to"]))
    elif op == "replace":
        os.replace(at(step["from"]), at(step["to"]))
    elif op == "unlink":
        p.unlink()
    elif op == "chmod":
        os.chmod(p, step["mode"])
    elif op == "chflags":
        os.chflags(p, step["flags"])
    elif op == "mkfifo":
        os.mkfifo(p)
    elif op == "utime":
        os.utime(p, ns=(step["ns"], step["ns"]), follow_symlinks=False)
    elif op == "many":                       # dirs × files: dir_name and file_name with {i} and {j}
        for i in range(step.get("dirs", 1)):
            d = p / step.get("dir_name", "").format(i=i) if step.get("dir_name") else p
            d.mkdir(parents=True, exist_ok=True)
            for j in range(step.get("files", 0)):
                (d / step["file_name"].format(i=i, j=j)).write_text(step.get("text", "x"))
    elif op == "sh":
        subprocess.run(sub(step["command"]), shell=True, cwd=at(step["cwd"]), check=True, capture_output=True)
    else:
        raise ValueError(op)


@contextlib.contextmanager
def race(spec: dict | None, store: Changes, at, sub):
    if spec is None:
        yield
        return
    kind = spec["kind"]
    undo = []
    if kind == "access_swap":
        n, target = at(spec["path"]), at(spec["target"])
        real_access = os.access

        def access(p, mode, *a, **k):
            if Path(str(p)) == n and not n.is_symlink():
                n.unlink()
                n.symlink_to(target)
            return real_access(p, mode, *a, **k)
        os.access = access
        undo.append(lambda: setattr(os, "access", real_access))
    elif kind == "skip_swap":
        work = at(spec["path"])
        real_skip = store._skip
        calls = []

        def skip(p):
            calls.append(p)
            r = real_skip(p)
            if p == work and calls.count(work) == spec["nth"] and not work.is_symlink():
                os.rename(work, at(spec["rename_to"]))
                work.symlink_to(at(spec["link"]))
            return r
        store._skip = skip
        undo.append(lambda: delattr(store, "_skip"))
    elif kind == "open_swap":
        names = [sub(n) for n in spec["names"]]
        d = at(spec["dir"])
        real_open = os.open

        def opened(path, flags, *a, dir_fd=None, **k):
            fd = real_open(path, flags, *a, dir_fd=dir_fd, **k)
            mine = sys._getframe(1).f_code.co_filename == cm.__file__
            if mine and flags & os.O_DIRECTORY and str(path) in names and not d.is_symlink():
                os.rename(d, at(spec["rename_to"]))
                d.symlink_to(at(spec["link"]))
            return fd
        os.open = opened

        def back():
            os.open = real_open
            if d.is_symlink():
                d.unlink()
                os.rename(at(spec["rename_to"]), d)
        undo.append(back)
    elif kind == "read_swap":
        target, d = at(spec["path"]), at(spec["dir"])
        real_read = cm._read

        def read(path, closed, limit=None):
            if Path(path) == target and not d.is_symlink():
                os.rename(d, at(spec["rename_to"]))
                d.symlink_to(at(spec["link"]))
                try:
                    return real_read(path, closed, limit)
                finally:
                    d.unlink()
                    os.rename(at(spec["rename_to"]), d)
            return real_read(path, closed, limit)
        cm._read = read
        undo.append(lambda: setattr(cm, "_read", real_read))
    else:
        raise ValueError(kind)
    try:
        yield
    finally:
        for u in undo:
            u()


def paths_of(step: dict, at, sub, home: Path) -> tuple[list[Path], list[str] | None]:
    if "named" in step:
        found = named(sub(step["named"]["text"]), at(step["named"]["cwd"]), home)
        return found, [str(p) for p in found]
    return [at(p) if not p.startswith("/") else Path(sub(p)) for p in step["paths"]], None


def call(step: dict, store: Changes, at, sub, root: str, make_store) -> object:
    c = step["call"]
    if c == "before":
        paths, names = paths_of(step, at, sub, store.home)
        with race(step.get("race"), store, at, sub):
            counts = store.before(step["task"], step["tree"], paths)
        out = {"counts": counts, "record": norm_index(record_text(store, step["task"]), root)}
        if names is not None:
            out["named"] = norm_value(names, root)
        return out
    if c == "changes":
        with race(step.get("race"), store, at, sub):
            found = store.changes(step["task"], step.get("tree"))
        with race(step.get("describe_race"), store, at, sub):
            texts = [store.describe(x) for x in found]
        module = [describe(x) for x in found]
        return [{"change": norm_change(x, root), "describe": norm_text(t, root), "module": norm_text(m, root)}
                for x, t, m in zip(found, texts, module)]
    if c == "describe_change":
        ch = Change(**{k: (sub(v) if isinstance(v, str) else v) for k, v in step["change"].items()})
        return norm_text(describe(ch), root)
    if c == "reset":
        return norm_value(store.reset(step["task"]), root)
    if c == "forget":
        return norm_value(store.forget(step["tree"]), root)
    if c == "sweep":
        keep = set(step["keep"])
        return norm_value(store.sweep(keep=lambda t: t in keep), root)
    if c == "made_in_tree":
        return store.made_in_tree(step["tree"], at(step["path"]))
    if c == "index":
        f = store._folder(step["tree"], step["task"]) / cm.INDEX
        return norm_index(f.read_text() if f.exists() else None, root)
    if c == "index_mtime":
        return (store._folder(step["tree"], step["task"]) / cm.INDEX).stat().st_mtime_ns
    if c == "set_index_mtime":
        f = store._folder(step["tree"], step["task"]) / cm.INDEX
        os.utime(f, ns=(step["ns"], step["ns"]))
        return None
    if c == "snapshot":
        return snapshot(at(step["dir"]), root)
    if c == "restart":
        return None
    if c == "probe_read":
        got = cm._read(at(step["path"]), None)
        return None if got is None else b64(got)
    if c == "probe_same":
        return cm._same(at(step["a"]), at(step["b"]))
    if c == "text_of":
        got = cm.text_of(at(step["path"]))
        return None if got is None else [norm_text(got[0], root), [list(a) for a in got[1]]]
    raise ValueError(c)


# ---------- scenarios (from tests/test_changes*.py) ----------

COVER = [["Student Name", "Student Name"], ["Student ID", "ID"]]
TABLE_10 = [["ID", "Requirement", "Priority"], ["R1", "Show the queue position", "Must"]]
ASSIGNMENT = docx(["Human Computer Interaction", COVER, "Table 10. Requirements", TABLE_10])
PROTECTED = {"root": "home/.ferb/changes", "closed": "protection", "ferb": "home/.ferb"}


def protected_home() -> list:
    return [{"op": "mkdir", "path": "home/.ferb/codex-home"},
            {"op": "write", "path": "home/.ferb/codex-home/auth.json", "text": '{"refresh_token": "%s"}' % TOKEN},
            {"op": "mkdir", "path": "home/.ssh"},
            {"op": "write", "path": "home/.ssh/id_ed25519", "text": "PRIVATE KEY " + TOKEN}]


def scenarios() -> list:
    s: list[dict] = []
    add = lambda name, steps, **kw: s.append({"name": name, "steps": steps, **kw})   # noqa: E731
    asg = "HCI ASSIGNMENT/Student_Name_ID_HCI_Assignment1.docx"
    live = ("set -eu\nf='$ROOT/" + asg + "'\nw=$(mktemp -d)\ncd \"$w\" && unzip -qq \"$f\"\n"
            "perl -0pi -e 's{<w:t xml:space=\"preserve\">ID</w:t>}{<w:t xml:space=\"preserve\">2031xy04417</w:t>}g' "
            "word/document.xml\nrm \"$f\" && zip -q -X -r \"$f\" .\n")
    add("live_perl_replace", [
        {"op": "mkdir", "path": "HCI ASSIGNMENT"}, {"op": "bytes", "path": asg, "b64": b64(ASSIGNMENT)},
        {"call": "before", "task": "t.step.one", "tree": "t", "named": {"text": live, "cwd": "."}},
        {"op": "sh", "cwd": ".", "command": live},
        {"call": "changes", "task": "t.step.one"}])
    made = "HCI ASSIGNMENT/Sam_2031xy04417_HCI_Assignment1.docx"
    add("corrected_copy_beside_original", [
        {"op": "mkdir", "path": "HCI ASSIGNMENT"}, {"op": "bytes", "path": asg, "b64": b64(ASSIGNMENT)},
        {"call": "before", "task": "t.step.one", "tree": "t", "paths": ["HCI ASSIGNMENT", asg, made]},
        {"op": "bytes", "path": made, "b64": b64(docx(
            ["Human Computer Interaction", [["Student Name", "Sam"], ["Student ID", "2031xy04417"]],
             "Table 10. Requirements", [["2031xy04417", "Requirement", "Priority"], TABLE_10[1]]], creator="Sam"))},
        {"call": "before", "task": "t.step.one", "tree": "t", "paths": [asg]},
        {"op": "unlink", "path": asg},
        {"call": "changes", "task": "t.step.one"},
        {"call": "snapshot", "dir": "changes"}])
    doc = "NP/StudentID_StudentName_NP_Assignment.pdf"
    lines = ["Student Name: StudentName", "Student ID: IdNumber"]
    add("pdf_covered_by_boxes", [
        {"op": "mkdir", "path": "NP"}, {"op": "bytes", "path": doc, "b64": b64(pdf(lines))},
        {"call": "before", "task": "t.step.two", "tree": "t", "paths": [doc]},
        {"op": "bytes", "path": "NP/covered.pdf", "b64": b64(pdf(lines, [("Square", ""), ("FreeText", "Sam")]))},
        {"op": "replace", "from": "NP/covered.pdf", "to": doc},
        {"call": "changes", "task": "t.step.two"},
        {"call": "text_of", "path": doc}])
    add("new_pdf_named", [
        {"op": "mkdir", "path": "NP"},
        {"call": "before", "task": "t.a", "tree": "t", "paths": ["NP/made.pdf"]},
        {"op": "bytes", "path": "NP/made.pdf", "b64": b64(pdf(["A new page"], [("FreeText", "Note é")]))},
        {"call": "changes", "task": "t.a"}])
    add("same_bytes_no_change", [
        {"op": "write", "path": "notes.txt", "text": "same\n"},
        {"call": "before", "task": "t", "tree": "t", "paths": ["notes.txt"]},
        {"op": "write", "path": "notes.txt", "text": "same\n"},
        {"call": "changes", "task": "t"}])
    add("plain_edit_and_new_file", [
        {"op": "write", "path": "http_client.c", "text": "int fuction(void);\nint main(void);\n"},
        {"call": "before", "task": "t", "tree": "t", "paths": ["http_client.c"]},
        {"op": "write", "path": "http_client.c", "text": "int function(void);\nint main(void);\n"},
        {"op": "write", "path": "out.log", "text": "built\n"},
        {"op": "utime", "path": "out.log", "ns": FIXED},
        {"call": "changes", "task": "t"}])
    add("closed_store_and_trash", [
        {"op": "mkdir", "path": "secrets"}, {"op": "write", "path": "secrets/keys.txt", "text": "k"},
        {"op": "mkdir", "path": ".Trash"}, {"op": "write", "path": ".Trash/old.txt", "text": "o"},
        {"call": "before", "task": "t", "tree": "t", "paths": ["secrets/keys.txt", ".Trash/old.txt", "secrets"]}],
        store={"closed": {"part": "secrets"}})
    add("restart_and_forget", [
        {"op": "write", "path": "a.txt", "text": "one\n"},
        {"call": "before", "task": "t.step.a", "tree": "t", "paths": ["a.txt"]},
        {"call": "index", "task": "t.step.a", "tree": "t"},
        {"op": "write", "path": "a.txt", "text": "two\n"},
        {"call": "restart"},
        {"call": "changes", "task": "t.step.a", "tree": "t"},
        {"call": "made_in_tree", "tree": "t", "path": "a.txt"},
        {"call": "made_in_tree", "tree": "t", "path": "b.txt"},
        {"call": "forget", "tree": "t"},
        {"call": "snapshot", "dir": "changes"}])
    add("loop_round_reset", [
        {"op": "write", "path": "a.txt", "text": "one\n"},
        {"call": "before", "task": "t.loop.x", "tree": "t", "paths": ["a.txt"]},
        {"op": "write", "path": "a.txt", "text": "two\n"},
        {"call": "reset", "task": "t.loop.x"},
        {"call": "before", "task": "t.loop.x", "tree": "t", "paths": ["a.txt"]},
        {"op": "write", "path": "a.txt", "text": "three\n"},
        {"call": "changes", "task": "t.loop.x"}])
    for what in ("removed", "created", "modified", "renamed"):
        add(f"describe_no_copy_{what}", [
            {"call": "describe_change", "change": {"path": "$ROOT/gone.bin", "what": what,
                                                   "note": "FERB kept no copy from before (it wasn't named)"}},
            {"call": "describe_change", "change": {"path": "$ROOT/gone.bin", "what": what, "content": False,
                                                   "size": 1234567, "mtime_ns": FIXED}},
            {"call": "describe_change", "change": {"path": "$ROOT/gone.bin", "what": what, "content": False,
                                                   "named": True, "source": "$ROOT/was.bin"}},
            {"call": "describe_change", "change": {"path": "$ROOT/gone.bin", "what": what, "content": False,
                                                   "original": True, "source": "$ROOT/was.bin", "size": 5,
                                                   "note": cm.UNCHANGED}}])
    # round 3: history files by identity
    for i, (rel, name) in enumerate([
            (".zsh_sessions/ABC.history", "w/h.txt"), (".python_history", ".PYTHON_HISTORY"),
            (".local/share/fish/fish_history", "w/fish.txt"), (".psql_history", "w/p.txt"), (".lesshst", "w/l.txt"),
            (".histfile", "w/hf.txt"), (".bash_sessions/X.history", ".bash_sessions/X.history"),
            (".sh_history", ".Sh_History"), (".history", "w/tcsh.txt")]):
        steps = [{"op": "mkdir", "path": str(Path("home", rel).parent)},
                 {"op": "write", "path": f"home/{rel}", "text": "ls\n"}]
        if name.startswith("w/"):
            steps += [{"op": "mkdir", "path": "home/w"}, {"op": "link", "path": f"home/{name}", "target": f"home/{rel}"}]
        steps += [{"call": "before", "task": "t.a", "tree": "t", "paths": [f"home/{name}"]},
                  {"op": "append", "path": f"home/{rel}", "text": "mysql -phunter2-SECRET\n"},
                  {"call": "changes", "task": "t.a"}]
        add(f"history_by_identity_{i}", steps)
    add("walk_only_through_checked_handle", protected_home()[:1] + [
        {"op": "mkdir", "path": "home/.ssh"}, {"op": "write", "path": "home/.ssh/id_ed25519_work_github", "text": "k"},
        {"op": "mkdir", "path": "home/Documents/proj"},
        {"call": "before", "task": "t.a", "tree": "t", "paths": ["home/Documents/proj"]},
        {"op": "mkdir", "path": "home/Documents/proj/.ssh/sub"},
        {"op": "write", "path": "home/Documents/proj/.ssh/mine.txt", "text": "x"},
        {"op": "write", "path": "home/Documents/proj/.ssh/sub/deeper.txt", "text": "y"},
        {"call": "changes", "task": "t.a",
         "race": {"kind": "open_swap", "names": [".ssh", "$ROOT/home/Documents/proj/.ssh"],
                  "dir": "home/Documents/proj", "rename_to": "home/Documents/proj.real", "link": "home"}}],
        store=PROTECTED)
    add("fact_limits_across_new_folders", [
        {"op": "mkdir", "path": "proj"}, {"op": "write", "path": "proj/package.json", "text": "{}"},
        {"call": "before", "task": "t.s", "tree": "t", "named": {"text": "cd $ROOT/proj && make", "cwd": "."}},
        {"op": "many", "path": "proj", "dirs": 200, "dir_name": "dir{i}", "files": 30, "file_name": "f{j}.txt"},
        {"call": "changes", "task": "t.s"}])
    add("flat_new_files_share_a_page", [
        {"op": "mkdir", "path": "proj"},
        {"call": "before", "task": "t.s", "tree": "t", "paths": ["proj"]},
        {"op": "many", "path": "proj", "files": 6000, "file_name": "f{j}.txt"},
        {"call": "changes", "task": "t.s"}])
    add("new_named_docx_whole", [
        {"op": "mkdir", "path": "w"},
        {"call": "before", "task": "t.a", "tree": "t", "paths": ["w/submission.docx"]},
        {"op": "bytes", "path": "w/submission.docx",
         "b64": b64(docx([f"paragraph {i} of the thesis" for i in range(1500)] + ["UNREQUESTED LINE AT THE END"]))},
        {"call": "changes", "task": "t.a"}])
    add("renamed_named_file_diffed", [
        {"op": "mkdir", "path": "w"}, {"op": "write", "path": "w/notes.txt", "text": "Student ID: ID\nline two\n"},
        {"call": "before", "task": "t.a", "tree": "t", "named": {
            "text": "cd $ROOT/w && mv notes.txt notes-final.txt && sed -i '' s/ID$/2024/ notes-final.txt",
            "cwd": "w"}},
        {"op": "rename", "from": "w/notes.txt", "to": "w/notes-final.txt"},
        {"op": "write", "path": "w/notes-final.txt", "text": "Student ID: 2024\nline two\n"},
        {"call": "changes", "task": "t.a"}])
    add("pure_rename", [
        {"op": "mkdir", "path": "w"}, {"op": "write", "path": "w/a.txt", "text": "same\n"},
        {"call": "before", "task": "t.a", "tree": "t", "paths": ["w/a.txt", "w/b.txt"]},
        {"op": "rename", "from": "w/a.txt", "to": "w/b.txt"},
        {"call": "changes", "task": "t.a"}])
    body = "".join(f"paragraph {i} of the thesis text\n" for i in range(40))
    add("corrected_copy_other_name", [
        {"op": "mkdir", "path": "w"}, {"op": "write", "path": "w/thesis.txt", "text": body + "Student ID: ID\n"},
        {"call": "before", "task": "t.a", "tree": "t", "paths": ["w/thesis.txt", "w/submission.txt"]},
        {"op": "write", "path": "w/submission.txt",
         "text": body.replace("paragraph 7 ", "paragraph SEVEN ") + "Student ID: 2024\n"},
        {"call": "changes", "task": "t.a"}])
    add("named_file_unreadable", [
        {"op": "mkdir", "path": "w"}, {"op": "write", "path": "w/log.txt", "text": "a\n"},
        {"op": "chmod", "path": "w/log.txt", "mode": 0o200},
        {"call": "before", "task": "t.a", "tree": "t", "paths": ["w/log.txt"]},
        {"op": "append", "path": "w/log.txt", "text": "b\n"},
        {"call": "changes", "task": "t.a"},
        {"op": "chmod", "path": "w/log.txt", "mode": 0o600}])
    add("fifo_never_blocks", [
        {"op": "mkfifo", "path": "n.txt"}, {"op": "write", "path": "x", "text": "x"},
        {"call": "probe_read", "path": "n.txt"}, {"call": "probe_same", "a": "n.txt", "b": "x"},
        {"call": "probe_same", "a": "x", "b": "x"}, {"call": "probe_read", "path": "x"},
        {"op": "write", "path": "doc.txt", "text": "x\n"},
        {"call": "before", "task": "t.a", "tree": "t", "paths": ["doc.txt"]},
        {"op": "unlink", "path": "doc.txt"}, {"op": "mkfifo", "path": "doc.txt"},
        {"call": "changes", "task": "t.a"}])
    add("moved_into_new_folder", [
        {"op": "mkdir", "path": "w"}, {"op": "write", "path": "w/notes.txt", "text": "a\nb\n"},
        {"call": "before", "task": "t.a", "tree": "t", "named": {
            "text": "cd $ROOT/w && mkdir done && mv notes.txt done/notes.txt", "cwd": "w"}},
        {"op": "mkdir", "path": "w/done"}, {"op": "rename", "from": "w/notes.txt", "to": "w/done/notes.txt"},
        {"call": "changes", "task": "t.a"}])
    add("case_only_rename", [
        {"op": "mkdir", "path": "w"}, {"op": "write", "path": "w/notes.txt", "text": "a\n"},
        {"call": "before", "task": "t.a", "tree": "t", "paths": ["w/notes.txt"]},
        {"op": "rename", "from": "w/notes.txt", "to": "w/Notes.txt"},
        {"call": "changes", "task": "t.a"}])
    add("named_file_past_walk_limit", [
        {"op": "mkdir", "path": "w"},
        {"call": "before", "task": "t.a", "tree": "t", "paths": ["w", "w/out/zz/result.txt"]},
        {"op": "mkdir", "path": "w/out/zz"},
        {"op": "many", "path": "w/out", "files": cm.WALK_MAX + 10, "file_name": "f{j}.txt"},
        {"op": "write", "path": "w/out/zz/result.txt", "text": "the answer\n"},
        {"call": "changes", "task": "t.a"}])
    add("empty_new_folders_count", [
        {"op": "mkdir", "path": "proj"},
        {"call": "before", "task": "t.s", "tree": "t", "paths": ["proj"]},
        {"op": "many", "path": "proj", "dirs": cm.WALK_MAX + 100, "dir_name": "e{i}"},
        {"call": "changes", "task": "t.s"}])
    add("deep_new_folders", [
        {"op": "mkdir", "path": "proj"},
        {"call": "before", "task": "t.s", "tree": "t", "paths": ["proj"]},
        {"op": "mkdir", "path": "proj/" + "/".join(f"d{i}" for i in range(40))},
        {"op": "write", "path": "proj/" + "/".join(f"d{i}" for i in range(20)) + "/mid.txt", "text": "m"},
        {"op": "write", "path": "proj/" + "/".join(f"d{i}" for i in range(40)) + "/deep.txt", "text": "d"},
        {"call": "changes", "task": "t.s"}])
    # round 4
    for i, (command, backup) in enumerate([
            ("sed -i.bak 's/ID$/2024/' notes.txt", "notes.txt.bak"),
            ("perl -pi.bak -e 's/ID$/2024/' notes.txt", "notes.txt.bak"),
            ("mv notes.txt notes.txt.orig && sed 's/ID$/2024/' notes.txt.orig > notes.txt", "notes.txt.orig")]):
        add(f"edit_keeps_backup_{i}", [
            {"op": "mkdir", "path": "w"}, {"op": "write", "path": "w/notes.txt", "text": "Student ID: ID\nline two\n"},
            {"call": "before", "task": "t.a", "tree": "t", "named": {"text": f"cd '$ROOT/w' && {command}", "cwd": "w"}},
            {"op": "sh", "cwd": "w", "command": command},
            {"call": "changes", "task": "t.a"}])
    add("in_place_no_backup", [
        {"op": "mkdir", "path": "w"}, {"op": "write", "path": "w/notes.txt", "text": "Student ID: ID\nline two\n"},
        {"call": "before", "task": "t.a", "tree": "t",
         "named": {"text": "cd '$ROOT/w' && perl -pi -e 's/ID$/2024/' notes.txt", "cwd": "w"}},
        {"op": "sh", "cwd": "w", "command": "perl -pi -e 's/ID$/2024/' notes.txt"},
        {"call": "changes", "task": "t.a"}])
    add("backup_changed_too", [
        {"op": "mkdir", "path": "w"}, {"op": "write", "path": "w/notes.txt", "text": "Student ID: ID\nline two\n"},
        {"call": "before", "task": "t.a", "tree": "t", "paths": ["w/notes.txt", "w/old.txt"]},
        {"op": "rename", "from": "w/notes.txt", "to": "w/old.txt"},
        {"op": "append", "path": "w/old.txt", "text": "added to the backup\n"},
        {"op": "write", "path": "w/notes.txt", "text": "Student ID: 2024\nline two\n"},
        {"call": "changes", "task": "t.a"}])
    for i, (link, target) in enumerate([(".zsh_history", "dotfiles/zsh_history"),
                                        (".zsh_sessions/ABC.history", "sync/ABC.history"),
                                        (".local/share/fish/fish_history", "dotfiles/fish_history")]):
        add(f"history_through_link_{i}", [
            {"op": "mkdir", "path": str(Path("home", target).parent)},
            {"op": "write", "path": f"home/{target}", "text": "ls\n"},
            {"op": "mkdir", "path": str(Path("home", link).parent)},
            {"op": "symlink", "path": f"home/{link}", "target": f"$ROOT/home/{target}"},
            {"call": "before", "task": "t.a", "tree": "t", "paths": [f"home/{target}"]},
            {"op": "append", "path": f"home/{link}", "text": "echo TYPED-BY-OWNER\n"},
            {"call": "changes", "task": "t.a"}])
    add("history_link_named", [
        {"op": "mkdir", "path": "home/dotfiles"}, {"op": "write", "path": "home/dotfiles/zsh_history", "text": "ls\n"},
        {"op": "symlink", "path": "home/.zsh_history", "target": "$ROOT/home/dotfiles/zsh_history"},
        {"call": "before", "task": "t.a", "tree": "t", "paths": ["home/.zsh_history", "home/notes.txt"]},
        {"op": "append", "path": "home/.zsh_history", "text": "echo TYPED-BY-OWNER\n"},
        {"call": "changes", "task": "t.a"}])
    closed_home = [{"op": "mkdir", "path": "home/.ferb/codex-home"},
                   {"op": "write", "path": "home/.ferb/codex-home/auth.json",
                    "text": f'{{"refresh_token": "{TOKEN}"}}\n{{"account": "me"}}\n'}]
    add("sibling_read_through_checked_fd", closed_home + [
        {"op": "mkdir", "path": "home/Documents/w"},
        {"op": "write", "path": "home/Documents/w/auth.json", "text": '{"refresh_token": "decoy"}\n{"account": "me"}\n'},
        {"call": "before", "task": "t.a", "tree": "t", "paths": ["home/Documents/w/auth2.json"]},
        {"op": "write", "path": "home/Documents/w/auth2.json",
         "text": '{"refresh_token": "decoy"}\n{"account": "me"}\n{"x": 1}\n'},
        {"call": "changes", "task": "t.a",
         "describe_race": {"kind": "read_swap", "path": "home/Documents/w/auth.json", "dir": "home/Documents/w",
                           "rename_to": "home/Documents/w.real", "link": "home/.ferb/codex-home"}}],
        store=PROTECTED)
    tbody = "".join(f"line {i} of the thesis\n" for i in range(20))
    add("siblings_listed_through_fd", closed_home + [
        {"op": "mkdir", "path": "home/Documents/w"}, {"op": "write", "path": "home/Documents/w/thesis.txt", "text": tbody},
        {"call": "before", "task": "t.a", "tree": "t", "paths": ["home/Documents/w/thesis-final.txt"]},
        {"op": "write", "path": "home/Documents/w/thesis-final.txt", "text": tbody + "one more line\n"},
        {"call": "changes", "task": "t.a"}], store=PROTECTED)
    add("stop_fact_every_listed_folder", [
        {"op": "mkdir", "path": "a"}, {"op": "mkdir", "path": "b"},
        {"call": "before", "task": "t.s", "tree": "t", "paths": ["a", "b"]},
        {"op": "mkdir", "path": "a/out"},
        {"op": "many", "path": "a/out", "files": cm.WALK_MAX + 10, "file_name": "f{j}.txt"},
        {"op": "mkdir", "path": "b/build"}, {"op": "write", "path": "b/build/x.txt", "text": "x"},
        {"call": "changes", "task": "t.s"}])
    # protection (round 1)
    for i, target in enumerate([".ferb/codex-home/auth.json", ".ssh/id_ed25519"]):
        add(f"named_swapped_for_link_{i}", protected_home() + [
            {"op": "mkdir", "path": "home/Documents"},
            {"op": "write", "path": "home/Documents/notes.txt", "text": "my notes\n"},
            {"call": "before", "task": "t.step.a", "tree": "t", "paths": ["home/Documents/notes.txt"]},
            {"op": "unlink", "path": "home/Documents/notes.txt"},
            {"op": "symlink", "path": "home/Documents/notes.txt", "target": f"$ROOT/home/{target}"},
            {"call": "changes", "task": "t.step.a"}], store=PROTECTED)
    add("new_links_never_read", protected_home() + [
        {"op": "mkdir", "path": "home/Documents"},
        {"call": "before", "task": "t.step.a", "tree": "t", "paths": ["home/Documents", "home/Documents/report.txt"]},
        {"op": "symlink", "path": "home/Documents/report-old.txt", "target": "$ROOT/home/.ssh/id_ed25519"},
        {"op": "symlink", "path": "home/Documents/leak.txt", "target": "$ROOT/home/.ferb/codex-home/auth.json"},
        {"op": "write", "path": "home/Documents/report.txt", "text": "PRIVATE KEY rt-O report\n"},
        {"call": "changes", "task": "t.step.a"}], store=PROTECTED)
    add("command_naming_closed_store", protected_home() + [
        {"op": "mkdir", "path": "home/work"},
        {"call": "before", "task": "t.step.a", "tree": "t",
         "named": {"text": "cat ~/.ssh/id_ed25519 > x; ls ~/.ferb", "cwd": "home/work"}},
        {"op": "write", "path": "home/.ssh/id_ed25519", "text": "changed " + TOKEN},
        {"call": "changes", "task": "t.step.a"}], store=PROTECTED)
    add("cd_home_history_fact", protected_home() + [
        {"op": "write", "path": "home/.zsh_history", "text": "ls\n"},
        {"call": "before", "task": "t.step.a", "tree": "t", "named": {"text": "cd ~ && ls", "cwd": "home/work"}},
        {"op": "write", "path": "home/.zsh_history", "text": "ls\nmysql -p hunter2-secret\n"},
        {"op": "bytes", "path": "home/downloaded.pdf", "b64": b64(b"%PDF-1.4 owner's own download")},
        {"call": "changes", "task": "t.step.a"}], store=PROTECTED)
    add("history_named_exactly", protected_home() + [
        {"op": "write", "path": "home/.zsh_history", "text": "ls\n"},
        {"call": "before", "task": "t.step.a", "tree": "t", "named": {"text": "tail ~/.zsh_history", "cwd": "home"}},
        {"op": "write", "path": "home/.zsh_history", "text": "ls\nexport TOKEN=hunter2-secret\n"},
        {"call": "changes", "task": "t.step.a"}], store=PROTECTED)
    add("declared_file_under_new_folder", protected_home() + [
        {"op": "mkdir", "path": "home/Desktop"},
        {"call": "before", "task": "t.step.a", "tree": "t", "paths": ["home/Desktop/report/summary.txt"]},
        {"op": "mkdir", "path": "home/Desktop/report"},
        {"op": "write", "path": "home/Desktop/report/summary.txt", "text": "the summary\n"},
        {"call": "changes", "task": "t.step.a"},
        {"call": "made_in_tree", "tree": "t", "path": "home/Desktop/report/summary.txt"},
        {"call": "made_in_tree", "tree": "t", "path": "home/Desktop/other.txt"}], store=PROTECTED)
    add("files_copied_into_made_folder", protected_home() + [
        {"op": "mkdir", "path": "home/work"}, {"op": "write", "path": "home/work/a", "text": "alpha\n"},
        {"call": "before", "task": "t.step.a", "tree": "t", "named": {"text": "mkdir -p out && cp a out/",
                                                                        "cwd": "home/work"}},
        {"op": "mkdir", "path": "home/work/out"}, {"op": "write", "path": "home/work/out/a", "text": "alpha\n"},
        {"call": "changes", "task": "t.step.a"}], store=PROTECTED)
    inv = [{"op": "mkdir", "path": "home/Downloads"}]
    for i in range(40):
        inv.append({"op": "bytes", "path": f"home/Downloads/invoice-{i:03d}.docx",
                    "b64": b64(docx([f"Invoice {i}", "Amount due"]))})
    add("pairing_reads_few", protected_home() + inv + [
        {"op": "bytes", "path": "home/Downloads/HCI_Assignment1.docx",
         "b64": b64(docx(["Human Computer Interaction", "Student ID", "ID"]))},
        {"call": "before", "task": "t.step.a", "tree": "t", "paths": ["home/Downloads/Sam_HCI_Assignment1.docx"]},
        {"op": "bytes", "path": "home/Downloads/Sam_HCI_Assignment1.docx",
         "b64": b64(docx(["Human Computer Interaction", "Student ID", "2031xy04417"]))},
        {"call": "changes", "task": "t.step.a"}], store=PROTECTED)
    add("resuming_tree_survives_sweep", [
        {"op": "write", "path": "a.txt", "text": "one\n"},
        {"call": "before", "task": "keep.step.a", "tree": "keep", "paths": ["a.txt"]},
        {"call": "restart"},
        {"call": "before", "task": "gone.step.a", "tree": "gone", "paths": ["a.txt"]},
        {"op": "write", "path": "a.txt", "text": "two\n"},
        {"call": "restart"},
        {"call": "sweep", "keep": ["keep"]},
        {"call": "changes", "task": "keep.step.a", "tree": "keep"},
        {"call": "changes", "task": "gone.step.a", "tree": "gone"},
        {"call": "snapshot", "dir": "changes"}])
    add("locked_clone_removable", [
        {"op": "write", "path": "locked.txt", "text": "locked\n"},
        {"op": "chflags", "path": "locked.txt", "flags": stat.UF_IMMUTABLE},
        {"call": "before", "task": "t.step.a", "tree": "t", "paths": ["locked.txt"]},
        {"call": "snapshot", "dir": "changes"},
        {"call": "forget", "tree": "t"},
        {"call": "snapshot", "dir": "changes"},
        {"op": "chflags", "path": "locked.txt", "flags": 0}])
    add("index_written_on_change_only", [
        {"op": "write", "path": "a.txt", "text": "one\n"},
        {"call": "before", "task": "a.step.x", "tree": "a", "paths": ["a.txt"]},
        {"call": "set_index_mtime", "task": "a.step.x", "tree": "a", "ns": FIXED},
        {"call": "before", "task": "a.step.x", "tree": "a", "paths": ["a.txt"]},
        {"call": "index_mtime", "task": "a.step.x", "tree": "a"},
        {"op": "write", "path": "b.txt", "text": "b\n"},
        {"call": "before", "task": "a.step.x", "tree": "a", "paths": ["b.txt"]},
        {"call": "index", "task": "a.step.x", "tree": "a"}])
    # races
    secret = protected_home()[:2]
    add("access_swap_never_cloned", secret + [
        {"op": "mkdir", "path": "home/.ssh"},
        {"op": "mkdir", "path": "home/Documents"}, {"op": "write", "path": "home/Documents/n.txt", "text": "plain\n"},
        {"call": "before", "task": "t.a", "tree": "t", "paths": ["home/Documents/n.txt"],
         "race": {"kind": "access_swap", "path": "home/Documents/n.txt", "target": "home/.ferb/codex-home/auth.json"}},
        {"call": "snapshot", "dir": "home/.ferb/changes"},
        {"op": "unlink", "path": "home/Documents/n.txt"},
        {"op": "write", "path": "home/Documents/n.txt", "text": "the worker's text\n"},
        {"call": "changes", "task": "t.a"}], store=PROTECTED)
    add("folder_swapped_between_check_and_listing", secret + [
        {"op": "mkdir", "path": "home/.ssh"},
        {"op": "write", "path": "home/.ssh/id_ed25519_work_github", "text": "PRIVATE KEY " + TOKEN},
        {"op": "mkdir", "path": "home/Documents/proj"},
        {"call": "before", "task": "t.a", "tree": "t", "paths": ["home/Documents/proj"],
         "race": {"kind": "skip_swap", "path": "home/Documents/proj", "nth": 2,
                  "rename_to": "home/Documents/proj.real", "link": "home/.ssh"}},
        {"op": "unlink", "path": "home/Documents/proj"},
        {"op": "rename", "from": "home/Documents/proj.real", "to": "home/Documents/proj"},
        {"call": "changes", "task": "t.a"}], store=PROTECTED)
    for i, how in enumerate(["~/.ZSH_HISTORY", "~/.Zsh_History", "data volume", "hard link"]):
        steps = secret + [{"op": "mkdir", "path": "home/.ssh"}, {"op": "write", "path": "home/.zsh_history",
                                                                 "text": "ls\n"}]
        if how == "data volume":
            before = {"call": "before", "task": "t.a", "tree": "t", "paths": ["/System/Volumes/Data$ROOT/home/.zsh_history"]}
        elif how == "hard link":
            steps.append({"op": "link", "path": "home/h2", "target": "home/.zsh_history"})
            before = {"call": "before", "task": "t.a", "tree": "t", "paths": ["home/h2"]}
        else:
            before = {"call": "before", "task": "t.a", "tree": "t", "named": {"text": f"tail {how}", "cwd": "home"}}
        add(f"history_spelled_{i}", steps + [before,
                                             {"op": "write", "path": "home/.zsh_history",
                                              "text": "ls\nmysql -phunter2-SECRET\n"},
                                             {"call": "changes", "task": "t.a"}], store=PROTECTED)
    add("named_file_replaced_by_link", secret + [
        {"op": "mkdir", "path": "home/.ssh"}, {"op": "write", "path": "home/report.txt", "text": "the owner's report\n"},
        {"call": "before", "task": "t.a", "tree": "t", "paths": ["home/report.txt"]},
        {"op": "unlink", "path": "home/report.txt"},
        {"op": "symlink", "path": "home/report.txt", "target": "$ROOT/home/.ferb/codex-home/auth.json"},
        {"call": "changes", "task": "t.a"}], store=PROTECTED)
    add("npm_install_new_folder", [
        {"op": "mkdir", "path": "proj"}, {"op": "write", "path": "proj/package.json", "text": "{}"},
        {"call": "before", "task": "t.s", "tree": "t", "named": {"text": "cd $ROOT/proj && npm install", "cwd": "."}},
        {"op": "many", "path": "proj/node_modules", "dirs": 60, "dir_name": "pkg{i}/lib", "files": 100,
         "file_name": "f{j}.js"},
        {"call": "changes", "task": "t.s"}])
    chapters = [{"op": "mkdir", "path": "docs"}]
    for i in range(10):
        chapters.append({"op": "bytes", "path": f"docs/chapter{i}.docx",
                         "b64": b64(docx([f"Chapter {i}", "Student ID", "ID"]))})
    fixed = []
    for i in range(10):
        fixed.append({"op": "bytes", "path": f"docs/chapter{i}-fixed.docx",
                      "b64": b64(docx([f"Chapter {i}", "Student ID", "2031xy04417"]))})
    add("pairing_in_bulk", chapters + [
        {"call": "before", "task": "t.a", "tree": "t",
         "paths": [f"docs/chapter{i}.docx" for i in range(10)] + [f"docs/chapter{i}-fixed.docx" for i in range(10)]},
        *fixed, {"call": "changes", "task": "t.a"}])
    add("text_preview_cut", [
        {"op": "mkdir", "path": "w"},
        {"call": "before", "task": "t.a", "tree": "t", "paths": ["w/long.txt", "w/crlf.txt", "w/bin.dat"]},
        {"op": "write", "path": "w/long.txt", "text": "".join(f"line {i} é\n" for i in range(3000))},
        {"op": "bytes", "path": "w/crlf.txt", "b64": b64(b"one\r\ntwo\rthree\x0bfour\x1cfive\xe2\x80\xa8six")},
        {"op": "bytes", "path": "w/bin.dat", "b64": b64(b"abc\x00def")},
        {"call": "changes", "task": "t.a"}])
    add("edit_crlf_and_binary_diff", [
        {"op": "mkdir", "path": "w"},
        {"op": "bytes", "path": "w/crlf.txt", "b64": b64(b"one\r\ntwo\r\nthree\r\n")},
        {"op": "bytes", "path": "w/bin.dat", "b64": b64(b"abc\x00def")},
        {"op": "write", "path": "w/folder-later", "text": "f"},
        {"call": "before", "task": "t.a", "tree": "t", "paths": ["w/crlf.txt", "w/bin.dat", "w/folder-later"]},
        {"op": "bytes", "path": "w/crlf.txt", "b64": b64(b"one\r\nTWO\r\nthree\r\nfour\r\n")},
        {"op": "bytes", "path": "w/bin.dat", "b64": b64(b"abc\x00xyz!")},
        {"op": "unlink", "path": "w/folder-later"}, {"op": "mkdir", "path": "w/folder-later"},
        {"call": "changes", "task": "t.a"}])
    add("listed_folder_removed_and_replaced", [
        {"op": "mkdir", "path": "a"}, {"op": "mkdir", "path": "b"}, {"op": "mkdir", "path": "elsewhere"},
        {"op": "write", "path": "a/x.txt", "text": "x"},
        {"call": "before", "task": "t.a", "tree": "t", "paths": ["a", "b"]},
        {"op": "unlink", "path": "a/x.txt"}, {"op": "sh", "cwd": ".", "command": "rmdir a"},
        {"op": "sh", "cwd": ".", "command": "rmdir b"}, {"op": "symlink", "path": "b", "target": "$ROOT/elsewhere"},
        {"call": "changes", "task": "t.a"}])
    return s


# ---------- named() and difflib ----------

def named_cases() -> list:
    texts = [
        "set -eu\nbase='$ROOT/HCI ASSIGNMENT'\nsrc=\"$base/Student_Name_ID_HCI_Assignment1.docx\"\n"
        "out=\"$base/Sam_2031xy04417_HCI_Assignment1.docx\"\nwork=$(mktemp -d)\n"
        "unzip -qq \"$src\" -d \"$work\"\nperl -0pi -e 's{x}{y}g' \"$work/word/document.xml\"\n",
        'set p to POSIX file "/Users/k/a b/x.pdf"',
        "python3 -c \"open('/tmp/x.txt','w').write('hi')\"",
        "cd ~/proj && sed -i '' 's/a/b/' notes.txt src/*.c",
        "echo hello world",
        "cp a.txt b.txt; mv 'my file.docx' \"other file.docx\"",
        "export OUT=~/out; local L=/tmp/l; readonly R=\"$OUT/r\"; declare D=$R/d; cat $D/f.txt ${OUT}/g.md",
        "X=1 Y=$X/z; ls $Y/a.txt $NOPE/b.txt `pwd`/c.txt $(pwd)/d.txt",
        "cd /a; cd b; cd ../c && touch x.log; cd \"/q r\" && touch y.log",
        "tar -czf --file=/tmp/out.tgz --dir:/srv/data (cd /x && ls) foo,/bar/baz.txt",
        "rm -rf ~/build/*.o /tmp/[ab]* ?x.txt *.txt dir/sub?/x",
        "echo 'it''s' \"esc\\\"aped/x.txt\" plain\\ name.txt",
        "a=~/x b=~ c=~/y/$a; ls $b/one.txt $c/two.txt",
        "find . -name '*.py' | xargs grep -l TODO > report.csv",
        "osascript -e 'tell application \"Finder\" to open POSIX file \"/Users/k/Desktop/n.pdf\"'",
        "cat <<EOF > /tmp/h.txt\nhello\nEOF",
        "ln -s /etc/hosts hosts.link && ls -la ./hosts.link ../up.txt .hidden",
        "x" * 1100 + "/y.txt " + "/" + "a" * 1030,
        "VAR=\"$(date)\"; echo $VAR/x.txt",
        "  cd   '/tmp/space dir'  ;  vi  file.TXT2 note.toolongext",
        "python - <<'PY'\nopen(\"/Users/k/out.json\",\"w\")\nopen('~/rel.txt')\nPY",
        "cp ~root/x.txt ~nobodyhere/y.txt",
        "echo é > /tmp/é.txt; ls\u00a0/tmp/nbsp.txt\u2003x.txt",
        "a=1;b=$a$a;c=$b$b;d=$c$c;e=$d$d;f=$e$e; echo /p/$f.txt",
    ]
    out = []
    root = "/fixed/root"
    for t in texts:
        text = t.replace("$ROOT", root)
        found = [str(p) for p in named(text, Path("/fixed/cwd"), Path("/fixed/home"))]
        words = [m.group(1) if m.group(1) is not None else (m.group(2) if m.group(2) is not None else m.group(3))
                 for m in cm._TOKENS.finditer(text)]
        ordered = not any((e := cm._EMBEDDED.search(w)) and e.group(1) != w for w in words if w)
        # Python iterates {word, embedded path} as a hash-seeded set: an unordered case is kept sorted
        out.append({"text": text, "cwd": "/fixed/cwd", "home": "/fixed/home",
                    "found": found if ordered else sorted(found), "ordered": ordered})
    return out


def difflib_cases() -> list:
    """Ratios as `repr(float)` text: a JSON reader may round a long float differently."""
    rnd = random.Random(20261113)
    out = []
    vocab = ["alpha", "beta", "gamma", "delta", "", "x", "y", "  indented", "the end"]
    for n in (0, 1, 3, 10, 40, 199, 200, 250, 400):
        for _ in range(3):
            a = [rnd.choice(vocab) for _ in range(n)]
            b = list(a)
            for _ in range(max(1, n // 5)):
                op = rnd.random()
                if op < 0.4 and b:
                    b[rnd.randrange(len(b))] = rnd.choice(vocab) + str(rnd.randrange(5))
                elif op < 0.7:
                    b.insert(rnd.randrange(len(b) + 1), rnd.choice(vocab))
                elif b:
                    del b[rnd.randrange(len(b))]
            m = difflib.SequenceMatcher(None, a, b, autojunk=False)
            j = difflib.SequenceMatcher(None, a, b)
            out.append({"a": a, "b": b, "blocks": [list(x) for x in j.get_matching_blocks()],
                        "ratio": repr(j.ratio()), "ratio_nojunk": repr(m.ratio()), "quick": repr(m.quick_ratio()),
                        "real_quick": repr(m.real_quick_ratio()),
                        "diff2": list(difflib.unified_diff(a, b, "before", "now", lineterm="", n=2)),
                        "diff3": list(difflib.unified_diff(a, b, "before", "now", lineterm="", n=3))})
    words = ["chapter1", "Chapter1-fixed", "thesis", "thesis-final", "invoice-007", "hci_assignment1",
             "sam_hci_assignment1", "", "a", "notes", "notes-final", "é-ü", "x" * 250]
    for a in words:
        for b in words:
            out.append({"sa": a, "sb": b, "ratio": repr(difflib.SequenceMatcher(None, a, b).ratio())})
    lines = ["a\r\nb\rc\n\nd", "", "\n", "x\x0by\x0cz\x1cw\x1dv\x1eu\x85t\u2028s\u2029r", "end\r", "\r\n\r\n"]
    for t in lines:
        out.append({"text": t, "lines": t.splitlines()})
    return out


def main() -> None:
    os.environ["TZ"] = "UTC"
    saved_home = os.environ.get("HOME")
    results = []
    try:
        for sc in scenarios():
            results.append({**sc, "outputs": run(sc)})
    finally:
        if saved_home is not None:
            os.environ["HOME"] = saved_home
    data = {"about": __doc__.splitlines()[0], "scenarios": results, "pdf": dict(sorted(PDFS.items())),
            "named": named_cases(), "difflib": difflib_cases(),
            "constants": {"WALK_MAX": cm.WALK_MAX, "FOLDER_FACTS": cm.FOLDER_FACTS, "WALK_DEPTH": cm.WALK_DEPTH}}
    OUT.write_text(json.dumps(data, indent=1, ensure_ascii=False) + "\n")
    print(f"{len(results)} scenarios, {len(data['named'])} named, {len(data['difflib'])} difflib -> {OUT}")


if __name__ == "__main__":
    main()
