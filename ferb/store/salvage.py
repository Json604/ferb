"""Own the check of FERB's home before setup or the daemon use what an older FERB left there (owner, 2026-11-01: FERB
reinstalled over old files: what is broken starts new, what isn't is used as it is). Must not delete the owner's data,
judge it by its content, or decide anything about program files.

- The owner's data that can't be read is never deleted. `config.toml` that doesn't parse as TOML, `ferb.db` that SQLite
  reports damaged or not a database, `owner-secret-handles.json` that isn't JSON: each is moved aside in FERB's home with
  a timestamp (`ferb.db.broken-20261101-0215`, its `-wal` and `-shm` under the same name so SQLite still opens the
  copy), FERB starts that file fresh, and the move is kept in `set-aside.json` until the daemon has journaled it
  (`home.set_aside`) and told the owner once in the notch.
- A database a newer FERB migrated isn't damaged: `NewerStore`, nothing changed (setup says so; the daemon exits cleanly
  so launchd doesn't restart it again and again).
- `persona.json` (what FERB saw on this Mac; FERB scans again) is FERB's own state: deleted when it isn't JSON.
- Cheap: SQLite's quick_check at each daemon start, in a worker thread (it reads the whole file; 0.5 s for 1.4 GB
  measured); the full integrity_check only to confirm a failed quick_check, and once per new build (the build whose
  full check passed is kept in run/store-checked.json, FERB's own state: lost, the check runs once more).
- An error that isn't damage (the file locked, unreadable, a disk error) is raised as it is: never a reason to move
  anything.
Program files (the secret helper, the extension, native-host files, the service record and the old LaunchAgent, the
`ferb` link) are rewritten by every setup (ferb/device/install.py); the socket, sandbox profiles and recording frames
are cleared by the daemon's start."""

import json
import os
import sqlite3
from datetime import datetime
from pathlib import Path
from typing import Any, Callable
from urllib.parse import quote

import tomlkit

from ferb.store.db import BUSY_TIMEOUT_MS, NewerStore, known_schema, newer_than_build

PENDING = "set-aside.json"                   # moves the owner hasn't been told about yet (the daemon tells, then clears)
CHECKED = Path("run") / "store-checked.json"  # the build whose full integrity check passed
STORE, CONFIG, SECRET_NAMES, PERSONA = "ferb.db", "config.toml", "owner-secret-handles.json", "persona.json"
STORE_COMPANIONS = ("-wal", "-shm")          # SQLite's own files beside the database, moved with it
# What the owner reads for each file FERB may set aside
WHAT = {STORE: "its database", CONFIG: "its settings file (config.toml)",
        SECRET_NAMES: "its list of saved secret names (the secrets stay in your Keychain)"}
SQLITE_CORRUPT, SQLITE_NOTADB = 11, 26       # the primary result codes that mean damage, not a passing problem
FOUND_SHOWN = 5                              # problems a check reports (and FERB keeps as the reason)

Now = Callable[[], datetime]


def this_build() -> str:
    """This FERB's build, for the once-per-build full check: its version, FERB.app's build number (a checkout's: none)
    and the newest schema it knows."""
    import ferb
    return f"{ferb.__version__}+{os.environ.get('FERB_APP_BUILD') or 'checkout'}+{known_schema()}"


def check_home(home: Path, build: str = "", now: Now = datetime.now) -> list[dict[str, Any]]:
    """The daemon's check at each start, before it opens anything: the database first (a newer one raises `NewerStore`
    before anything is changed), then the owner's files. Returns what it set aside now."""
    moved = check_store(home, build, now)
    return moved + check_files(home, now)


def check_files(home: Path, now: Now = datetime.now) -> list[dict[str, Any]]:
    """config.toml and the saved secret names, moved aside when they can't be parsed; persona.json deleted then.
    Setup runs this before it reads config.toml (the daemon's start too). Returns what it set aside."""
    moved = []
    config = home / CONFIG
    if config.is_file():
        try:
            tomlkit.parse(config.read_text())
        except ValueError as e:                  # tomlkit's ParseError, or text that isn't UTF-8
            moved.append(set_aside(home, CONFIG, f"it doesn't parse ({_short(e)})", now))
    for name in (SECRET_NAMES, PERSONA):
        path = home / name
        if not path.is_file():
            continue
        try:
            json.loads(path.read_text())
        except ValueError as e:
            if name == PERSONA:
                path.unlink()                    # FERB's own: it scans this Mac again
            else:
                moved.append(set_aside(home, name, f"it isn't JSON ({_short(e)})", now))
    return moved


def check_store(home: Path, build: str = "", now: Now = datetime.now) -> list[dict[str, Any]]:
    """ferb.db: kept when SQLite's check finds nothing; set aside with its WAL when it finds damage; `NewerStore` when a
    newer FERB migrated it. `build`: this FERB's build, which runs the full check once."""
    path = home / STORE
    if not path.is_file() or path.stat().st_size == 0:
        return []                                # nothing yet: Database makes it
    full = bool(build) and _checked(home) != build
    found = damage(path, full)
    if found is not None:
        return [set_aside(home, STORE, found, now, STORE_COMPANIONS)]
    if full:
        _mark_checked(home, build, now)
    return []


def refuse_newer(home: Path) -> None:
    """Setup's check, before it installs anything: `NewerStore` when a newer FERB migrated the database. Reads only the
    schema version; a database that can't be read is the daemon's to check."""
    try:
        with _open(home / STORE) as conn:
            _newer(conn)
    except (sqlite3.Error, FileNotFoundError):
        return


def damage(path: Path, full: bool = False) -> str | None:
    """What SQLite finds wrong with the database (its own words), or None when nothing. quick_check; integrity_check
    when `full`, and to confirm a failed quick_check. Raises `NewerStore`, and any error that isn't damage."""
    try:
        with _open(path) as conn:
            found = _check(conn, "quick_check")
            if found is not None or full:
                found = _check(conn, "integrity_check")
            if found is None:
                _newer(conn)
            return found
    except sqlite3.DatabaseError as e:
        if (getattr(e, "sqlite_errorcode", 0) or 0) & 0xFF in (SQLITE_CORRUPT, SQLITE_NOTADB):
            return f"SQLite: {_short(e)}"
        raise


def set_aside(home: Path, name: str, why: str, now: Now = datetime.now,
              companions: tuple[str, ...] = ()) -> dict[str, Any]:
    """Move `name` (and its companions, under the same new name) aside in FERB's home, never over another file, and
    record it for the owner. Returns the record."""
    when = now()
    base = f"{name}.broken-{when:%Y%m%d-%H%M}"
    target, n = base, 1
    while any((home / (target + suffix)).exists() for suffix in ("", *companions)):
        n += 1
        target = f"{base}-{n}"
    os.rename(home / name, home / target)
    for suffix in companions:
        if (home / (name + suffix)).exists():
            os.rename(home / (name + suffix), home / (target + suffix))
    entry = {"file": name, "moved_to": str(home / target), "why": why, "at": when.isoformat(timespec="seconds")}
    _write_pending(home, pending(home) + [entry])
    return entry


def pending(home: Path) -> list[dict[str, Any]]:
    """The moves the owner hasn't been told about yet (a record that can't be read is FERB's own: read as none)."""
    try:
        data = json.loads((home / PENDING).read_text())
    except (OSError, ValueError):
        return []
    return [e for e in data if isinstance(e, dict) and isinstance(e.get("moved_to"), str)] if isinstance(data, list) else []


def told(home: Path, entries: list[dict[str, Any]]) -> None:
    """These moves were journaled and the owner told: they leave the record."""
    done = {e["moved_to"] for e in entries}
    left = [e for e in pending(home) if e["moved_to"] not in done]
    if left:
        _write_pending(home, left)
    else:
        (home / PENDING).unlink(missing_ok=True)


def notice(entry: dict[str, Any]) -> str:
    """What the owner reads, once, about one move."""
    what = WHAT.get(str(entry.get("file")), str(entry.get("file")))
    return (f"FERB couldn't read {what}: {entry.get('why')}. It moved the file aside to {entry['moved_to']} (nothing "
            "was deleted) and started a new one.")


def describe(entries: list[dict[str, Any]]) -> list[str]:
    """Setup's lines for what it set aside."""
    return [f"{entry['file']} couldn't be read ({entry['why']}); moved aside to {entry['moved_to']}, starting a new one."
            for entry in entries]


# ---------- internals ----------

class _open:
    """A read-only connection to an existing database (never creates one), closed after the block: it never
    checkpoints the WAL into the file or deletes it, so a file set aside keeps exactly what it held."""

    def __init__(self, path: Path) -> None:
        if not path.is_file():
            raise FileNotFoundError(path)
        self.conn = sqlite3.connect(f"file:{quote(str(path))}?mode=ro", uri=True, timeout=BUSY_TIMEOUT_MS / 1000)

    def __enter__(self) -> sqlite3.Connection:
        return self.conn

    def __exit__(self, *exc: object) -> None:
        self.conn.close()


def _check(conn: sqlite3.Connection, pragma: str) -> str | None:
    rows = [str(r[0]) for r in conn.execute(f"PRAGMA {pragma}({FOUND_SHOWN})")]
    return None if rows == ["ok"] else f"SQLite's {pragma.replace('_', ' ')}: " + "; ".join(rows)[:400]


def _newer(conn: sqlite3.Connection) -> None:
    if conn.execute("SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'schema_version'").fetchone() is None:
        return
    newest, known = int(conn.execute("SELECT MAX(version) FROM schema_version").fetchone()[0] or 0), known_schema()
    if newest > known:
        raise NewerStore(newer_than_build(newest, known))


def _checked(home: Path) -> str | None:
    try:
        data = json.loads((home / CHECKED).read_text())
    except (OSError, ValueError):
        return None
    return str(data.get("build")) if isinstance(data, dict) else None


def _mark_checked(home: Path, build: str, now: Now) -> None:
    from ferb.config import write_private
    path = home / CHECKED
    write_private(path.parent, path, json.dumps({"build": build, "at": now().isoformat(timespec="seconds")}) + "\n")


def _write_pending(home: Path, entries: list[dict[str, Any]]) -> None:
    from ferb.config import write_private
    write_private(home, home / PENDING, json.dumps(entries, ensure_ascii=False, indent=2) + "\n")


def _short(e: BaseException) -> str:
    return (" ".join(str(e).split()) or type(e).__name__)[:200]
