"""Inspect installed-software facts without launching it or assigning intent. Interpretation belongs to FERB."""
import os
import plistlib
import sqlite3
import subprocess
from pathlib import Path
from typing import Any, Callable


def privacy_facts(bundle_id: str, path: Path, database: Path | None = None) -> dict[str, Any]:
    if database is None:
        return {"user": privacy_facts(bundle_id, path, Path.home() / "Library/Application Support/com.apple.TCC/TCC.db"),
                "system": privacy_facts(bundle_id, path, Path("/Library/Application Support/com.apple.TCC/TCC.db"))}
    try:
        with sqlite3.connect(database.as_uri() + "?mode=ro", uri=True) as db:
            cols = {r[1] for r in db.execute("PRAGMA table_info(access)")}
            if not {"service", "client", "auth_value", "last_modified"} <= cols:
                return {"status": "unsupported_schema"}
            rows = db.execute("SELECT service,client,auth_value,last_modified FROM access WHERE client=? OR client=? ORDER BY service",
                              (bundle_id, str(path))).fetchall()
            return {"status": "read", "grants": [dict(zip(("service", "client", "auth_value", "last_modified"), r)) for r in rows]}
    except sqlite3.Error as error:
        return {"status": "unavailable", "error": type(error).__name__, "requires": "Full Disk Access"}


def associated_jobs(path: Path, folders: list[Path] | None = None) -> list[dict[str, Any]]:
    folders = folders or [Path.home() / "Library/LaunchAgents", Path("/Library/LaunchAgents"), Path("/Library/LaunchDaemons")]
    jobs = []
    for folder in folders:
        for file in sorted(folder.glob("*.plist")):
            try:
                data = plistlib.loads(file.read_bytes())
                args = data.get("ProgramArguments", [])
                program = data.get("Program") or (args[0] if args else "")
                if program and Path(program).is_relative_to(path):
                    jobs.append({"path": str(file), "label": data.get("Label"), "program": program,
                                 "association": "executable inside this app bundle"})
            except (OSError, ValueError, TypeError, plistlib.InvalidFileException):
                continue
    return jobs


def login_items(path: Path, bundle_id: str, run: Callable[..., Any] = subprocess.run) -> dict[str, Any]:
    """Read the OS background-task inventory; match whole record identifiers, never guess installer causation."""
    try:
        result = run(["/usr/bin/sfltool", "dumpbtm"], capture_output=True)
    except OSError as error:
        return {"status": "unavailable", "error": type(error).__name__}
    if result.returncode:
        return {"status": "unavailable", "exit_code": result.returncode}
    records: list[dict[str, str]] = []
    current: dict[str, str] = {}
    for line in result.stdout.decode(errors="replace").splitlines():
        key, sep, value = line.strip().partition(":")
        if key == "UUID" and current:
            records.append(current)
            current = {}
        if sep and key in {"UUID", "Name", "Developer Name", "Team Identifier", "Type", "Flags", "Disposition",
                          "Identifier", "URL", "Executable Path", "Parent Identifier"}:
            current[key] = value.strip()
    if current:
        records.append(current)
    if not records:
        return {"status": "unsupported_or_empty", "detail": "No recognizable background-task records"}
    from urllib.parse import unquote, urlparse
    def associated(record: dict[str, str]) -> bool:
        if record.get("Identifier") == bundle_id or record.get("Parent Identifier") == bundle_id:
            return True
        for key in ("Executable Path", "URL"):
            raw = record.get(key, "")
            candidate = unquote(urlparse(raw).path) if raw.startswith("file:") else raw
            if candidate and Path(candidate).is_absolute() and Path(candidate).is_relative_to(path):
                return True
        return False
    return {"status": "read", "items": [r for r in records if associated(r)],
            "association": "exact app or parent identifier, or path inside app bundle; not proof of installer causation"}


def app_security_facts(path: Path, bundle_id: str, *, run: Callable[..., Any] = subprocess.run,
                       xattr: Callable[..., bytes] = os.getxattr, privacy: Callable[..., Any] = privacy_facts,
                       jobs: Callable[..., Any] = associated_jobs) -> dict[str, Any]:
    def command(args: list[str]) -> tuple[int, bytes, bytes]:
        try:
            r = run(args, capture_output=True)
            return r.returncode, r.stdout, r.stderr
        except OSError as error:
            return -1, b"", type(error).__name__.encode()
    code, out, err = command(["/usr/bin/codesign", "-dv", "--verbose=4", str(path)])
    signature: dict[str, Any] = {"exit_code": code}
    for line in (out + err).decode(errors="replace").splitlines():
        key, sep, value = line.partition("=")
        if sep and key in {"Authority", "TeamIdentifier", "Identifier", "Signature", "Runtime Version"}:
            if key == "Authority":
                signature.setdefault(key, []).append(value)
            else:
                signature[key] = value
    code, out, err = command(["/usr/sbin/spctl", "--assess", "--type", "execute", "--verbose=4", str(path)])
    assessment = {"accepted": code == 0, "exit_code": code, "detail": (out + err).decode(errors="replace")}
    code, out, err = command(["/usr/bin/codesign", "-d", "--entitlements", ":-", str(path)])
    try:
        entitlements = plistlib.loads(out) if code == 0 and out else {}
    except (ValueError, plistlib.InvalidFileException):
        entitlements = {"status": "unreadable"}
    attributes: dict[str, Any] = {}
    for name in ("com.apple.quarantine", "com.apple.metadata:kMDItemWhereFroms"):
        try:
            value = xattr(path, name)
            attributes[name] = plistlib.loads(value) if "WhereFroms" in name and value else value.decode(errors="replace")
        except (OSError, ValueError, plistlib.InvalidFileException):
            attributes[name] = None
    return {"bundle_id": bundle_id, "path": str(path), "signature": signature, "assessment": assessment,
            "entitlements": entitlements, "download": attributes, "privacy": privacy(bundle_id, path),
            "launch_jobs": jobs(path), "login_items": login_items(path, bundle_id, run)}
