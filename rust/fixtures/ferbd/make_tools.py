"""Records the Python tools ferbd ports itself (no other crate held them): the abilities catalog (`ferb/abilities`),
`memory.read` (`ferb/orchestrator/evidence.py` MemoryRead), `about.read`'s run (`ferb/daemon/about.py` AboutRead) and
`skills.read` (`ferb/orchestrator/skills.py`, whose reader ferb-orchestrator holds; the tool's result is composed in
ferbd). Writes tools.json beside this file.

Run from the repo root:  uv run python rust/fixtures/ferbd/make_tools.py

Nothing here launches a program: these tools only read FERB's package folder, a temporary store and a temporary skills
folder. Time is FakeClock (2026-01-01T09:00:00Z), ids SequentialIds("id"), all data invented. The temporary folder is
removed at the end. The memory scenario's rows are stored so the Rust test can put them into a daemon's store and ask
memory.read through the real binary.
"""

import asyncio
import json
import os
import shutil
import sqlite3
import sys
import tempfile
from datetime import datetime, timedelta, timezone
from pathlib import Path
from typing import Any

REPO = Path(__file__).resolve().parents[3]
HERE = Path(__file__).resolve().parent
ROOT = Path(tempfile.mkdtemp(prefix="ferbdtools-"))
os.environ["FERB_HOME"] = str(ROOT / "home")
os.environ["HOME"] = str(ROOT / "user")
(ROOT / "user").mkdir()
sys.dont_write_bytecode = True
sys.path.insert(0, str(REPO))

from ferb.abilities.catalog import AbilityCatalog  # noqa: E402
from ferb.abilities.tools import AbilitiesList, AbilitiesRead  # noqa: E402
from ferb.clock.clock import FakeClock  # noqa: E402
from ferb.core.errors import FerbError  # noqa: E402
from ferb.core.ids import SequentialIds  # noqa: E402
from ferb.daemon.about import AboutRead  # noqa: E402
from ferb.memory import items, learning  # noqa: E402
from ferb.memory.search import MemorySearch  # noqa: E402
from ferb.orchestrator.evidence import MemoryRead  # noqa: E402
from ferb.orchestrator.skills import Skills, SkillsRead  # noqa: E402
from ferb.store.db import Database  # noqa: E402


def run(coro: Any) -> Any:
    return asyncio.new_event_loop().run_until_complete(coro)


def result(tool: Any, args: dict[str, Any]) -> dict[str, Any]:
    """What the tool returned, or what it raised (FERB's errors by class and words)."""
    try:
        r = run(tool.run(tool.Input(**args), None))
    except FerbError as e:
        return {"raised": type(e).__name__, "message": str(e)}
    return {"content": r.content, "is_error": r.is_error, "read_sources": sorted(map(list, r.read_sources))}


# ---------- abilities ----------

CATALOG = json.loads((REPO / "rust/fixtures/catalog/tools.json").read_text())
ALL = [t["name"] for t in CATALOG["tools"]]
abilities = []
for label, installed in (("all", ALL), ("no_browser_wait", [n for n in ALL if n != "browser.wait"]), ("none", [])):
    cat = AbilityCatalog(capabilities=lambda installed=installed: installed)
    abilities.append({"installed": label, "list": result(AbilitiesList(cat), {}),
                      "read": {name: result(AbilitiesRead(cat), {"name": name})
                               for name in ("browser_verification", "google_docs", "../google_docs", "nope")}})

# ---------- about.read ----------

DOC = {"version": {"ferb": "0.28.0"}, "models": {"planner": "codex gpt-x"}, "health": {"db_bytes": 12}}


async def document() -> dict[str, Any]:
    return DOC


about = [{"args": a, "out": result(AboutRead(document), a)}
         for a in ({}, {"section": "version"}, {"section": "cost"}, {"section": "bogus"})]

# ---------- skills.read ----------

skills_dir = ROOT / "home" / "skills"
(skills_dir / "packing").mkdir(parents=True)
(skills_dir / "packing" / "SKILL.md").write_text(
    "---\nname: Packing list\ndescription: \"How the owner packs for a trip\"\nother: x\n---\nTake socks.\r\nTake a hat.\n")
(skills_dir / "packing" / "notes.txt").write_text("Weekend: one bag.\n")
(skills_dir / "plain").mkdir()
(skills_dir / "plain" / "SKILL.md").write_text("No front matter here.\n")
(ROOT / "outside.txt").write_text("not a skill\n")
os.symlink(ROOT / "outside.txt", skills_dir / "packing" / "link.txt")
reader = SkillsRead(Skills(skills_dir))
skills = [{"args": a, "out": result(reader, a)}
          for a in ({"skill": "packing"}, {"skill": "packing", "file": "notes.txt"}, {"skill": "plain"},
                    {"skill": "packing", "file": "link.txt"}, {"skill": "packing", "file": "../plain/SKILL.md"},
                    {"skill": "nope"}, {"skill": "../packing"})]
skills_files = {"packing/SKILL.md": (skills_dir / "packing" / "SKILL.md").read_text(),
                "packing/notes.txt": (skills_dir / "packing" / "notes.txt").read_text(),
                "plain/SKILL.md": (skills_dir / "plain" / "SKILL.md").read_text()}

# ---------- memory.read ----------

db = Database(ROOT / "ferb.db")
db.migrate()
clock = FakeClock()
ids = SequentialIds("id")
memory = items.Memory(db, clock, ids, lambda topic, data: None)
learn = learning.Learning(db, clock, lambda *a, **k: None, lambda s: s)


def save(kind: str, key: str, content: str, origin: str, **kw: Any) -> str:
    sources = kw.pop("sources", [("journal", f"j-{key}")])
    saved = memory.save(kind, kw.pop("subject", "owner"), key, content, origin, sources, **kw)
    clock.advance(60)
    return saved.item.id


dentist = save("fact", "dentist", "My dentist is Dr. Rao, on Tuesdays", "owner_stated")
tea = save("preference", "tea", "Likes green tea in the afternoon", "owner_stated")
bike = save("fact", "bike", "The bicycle chain was oiled last week", "inferred", about="the blue bicycle")
shop = save("fact", "shop", "The corner shop closes at eight", "third_party", subject="place:corner-shop")
long = save("fact", "trip_notes", "Trip notes: " + "pack light and early, " * 40, "owner_stated")
gone = save("fact", "old_phone", "The old phone number ended in 42", "owner_stated")
memory.retire(gone, "the owner said so")
week = save("fact", "where", "In Pune this week", "owner_stated")
memory.expire_at(week, clock.now() + timedelta(days=3))
forgotten = save("fact", "secret_plan", "A plan to forget", "owner_stated")
memory.forget(forgotten)
clock.advance(60)
# what the daemon journals for a forget (`memory.forgot`, the count only), and one made through another path
db.conn.execute("INSERT INTO journal (id, at, kind, data_json, origin) VALUES (?, ?, ?, ?, ?)",
                ("id-j1", "2026-01-01T09:30:00+00:00", "memory.forgot", '{"items": 2}', "ferb"))
db.conn.execute("INSERT INTO journal (id, at, kind, data_json, origin) VALUES (?, ?, ?, ?, ?)",
                ("id-j2", "2026-01-01T09:31:00+00:00", "memory.forgot", '{"items": 1, "via": "import"}', "ferb"))
db.conn.commit()


async def search(query: str) -> list[str]:
    return [h.data.id for h in MemorySearch(db, clock).facts(memory, query, limit=100)]


reader = MemoryRead(lambda: memory, source_origins=learn._source_origins, search=search)
memory_calls = []
for a in ({}, {"limit": 2}, {"after": 2, "limit": 2}, {"after": 50}, {"query": "dentist"},
          {"query": "Tuesdays dentist Rao"}, {"query": "bicycle"}, {"query": "the a of"}, {"query": "zebra"},
          {"item_id": dentist}, {"item_id": gone}, {"item_id": "nope"},
          {"item_id": long, "offset": 10, "max_chars": 1000}, {"forgotten": True}):
    full = MemoryRead.Input(**a).model_dump(mode="json")
    memory_calls.append({"args": full, "out": result(reader, a)})



def rows() -> dict[str, Any]:
    conn = sqlite3.connect(ROOT / "ferb.db")
    out = {}
    names = [r[0] for r in conn.execute("SELECT name FROM sqlite_master WHERE type = 'table' ORDER BY rowid")]
    # memory's own tables (not the search index, which its triggers rebuild), the journal, forgotten fingerprints
    for t in names:
        if not (t.startswith("memory_") or t in ("journal", "forgotten_refs")) or "fts" in t:
            continue
        cur = conn.execute(f'SELECT * FROM "{t}"')
        cols = [d[0] for d in cur.description]
        found = [list(r) for r in cur.fetchall()]
        if found:
            out[t] = {"columns": cols, "rows": found}
    conn.close()
    return out


# ---------- fs.look's decoders (ImageIO, PDFKit: what Rust's src/native.rs reproduces) ----------

import base64  # noqa: E402
import hashlib  # noqa: E402
import struct  # noqa: E402
import zlib  # noqa: E402

from ferb.act.fs import look  # noqa: E402


def png(width: int, height: int, alpha: bool) -> bytes:
    """A PNG of stripes (opaque RGB, or RGBA half transparent), written by hand."""
    def chunk(kind: bytes, data: bytes) -> bytes:
        return struct.pack(">I", len(data)) + kind + data + struct.pack(">I", zlib.crc32(kind + data) & 0xFFFFFFFF)
    rows = []
    for y in range(height):
        px = []
        for x in range(width):
            c = (x * 255 // max(1, width - 1), y * 255 // max(1, height - 1), 90)
            px.append(bytes(c) + (bytes([0 if (x // 50) % 2 else 255]) if alpha else b""))
        rows.append(b"\x00" + b"".join(px))
    ihdr = struct.pack(">IIBBBBB", width, height, 8, 6 if alpha else 2, 0, 0, 0)
    return (b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", ihdr) + chunk(b"IDAT", zlib.compress(b"".join(rows), 9))
            + chunk(b"IEND", b""))


def pdf() -> bytes:
    """Two pages: a letter page with a line of text, then a landscape-rotated one without text."""
    text = b"BT /F1 24 Tf 72 700 Td (Invented page one for FERB) Tj ET"
    objs = [b"<< /Type /Catalog /Pages 2 0 R >>",
            b"<< /Type /Pages /Kids [3 0 R 5 0 R] /Count 2 >>",
            b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 4 0 R "
            b"/Resources << /Font << /F1 6 0 R >> >> >>",
            b"<< /Length %d >>\nstream\n" % len(text) + text + b"\nendstream",
            b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 200] /Rotate 90 >>",
            b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>"]
    out = b"%PDF-1.4\n"
    offsets = []
    for n, o in enumerate(objs, 1):
        offsets.append(len(out))
        out += b"%d 0 obj\n" % n + o + b"\nendobj\n"
    xref = len(out)
    out += b"xref\n0 %d\n0000000000 65535 f \n" % (len(objs) + 1)
    out += b"".join(b"%010d 00000 n \n" % o for o in offsets)
    out += b"trailer\n<< /Size %d /Root 1 0 R >>\nstartxref\n%d\n%%%%EOF\n" % (len(objs) + 1, xref)
    return out


def decoded(fn: Any, *a: Any) -> dict[str, Any]:
    try:
        jpeg, facts = fn(*a)
    except look.Undecodable as e:
        return {"undecodable": str(e)}
    except IndexError as e:
        return {"no_page": e.args[0]}
    return {"facts": facts, "jpeg_bytes": len(jpeg), "jpeg_sha256": hashlib.sha256(jpeg).hexdigest()}


files = {"wide.png": png(3000, 700, False), "small_alpha.png": png(320, 200, True), "doc.pdf": pdf(),
         "text.txt": b"not a picture\n"}
looks = [{"file": "wide.png", "page": None, "out": decoded(look.picture, files["wide.png"], 1568)},
         {"file": "small_alpha.png", "page": None, "out": decoded(look.picture, files["small_alpha.png"], 1568)},
         {"file": "text.txt", "page": None, "out": decoded(look.picture, files["text.txt"], 1568)},
         *({"file": "doc.pdf", "page": n, "out": decoded(look.pdf_page, files["doc.pdf"], n, 1568)} for n in (1, 2, 3, 0))]

fixture = {
    "about": "ferbd's own ports, recorded from the Python on invented data (make_tools.py)",
    "abilities": abilities,
    "about_read": {"document": DOC, "calls": about},
    "skills": {"files": skills_files, "calls": skills},
    "memory": {"rows": rows(), "now": clock.now().isoformat(), "calls": memory_calls},
    "look": {"files": {k: base64.b64encode(v).decode() for k, v in files.items()}, "calls": looks},
}
(HERE / "tools.json").write_text(json.dumps(fixture, indent=1, ensure_ascii=False) + "\n")
shutil.rmtree(ROOT, ignore_errors=True)
print(f"tools.json: {len(abilities)} ability sets, {len(about)} about, {len(skills)} skills, "
      f"{len(memory_calls)} memory.read calls, {len(looks)} decodes")
