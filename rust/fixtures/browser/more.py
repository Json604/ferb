"""The rest of the browser hands' fixtures, for make.py (ENG-3): what entry 28 of rust/JOURNAL.md left not started.

    profiles_mac.json  profiles.py's Mac half as logic: Browser.binary's memory of Launch Services' answers, the
                       `pgrep -f` pattern `running` builds, the unsupported-browser scan (HAND-415) over invented app
                       bundles whose Info.plist is written by plistlib (XML and binary), with Launch Services and
                       pgrep replaced by recorders.
    uploads.json       uploads.py's Uploads: staging copies through folder handles, the races the Python tests make
                       (a folder swapped for a link mid-copy, a link planted at the copy's name, a link swapped in
                       after the copy), cleanup by tree, tab, collect and sweep, with every file left on disk.
    downloads.json     downloads.py's Downloads driven with a scripted extension peer (the Python tests' fake
                       extension's download_part rules), a fake clock, fake free space and the real store: every call
                       to the peer, every published event, every stored row and every file left, per scenario.

Paths under the temporary root are written "$ROOT"; random ids come from make.py's counter.
"""

import asyncio
import base64
import os
import plistlib
import shutil
import stat
import tempfile
from pathlib import Path
from typing import Any

HERE = Path(__file__).parent


def _norm(v: Any, root: Path) -> Any:
    if isinstance(v, (str, Path)):
        return str(v).replace(str(root), "$ROOT")
    if isinstance(v, (list, tuple)):
        return [_norm(x, root) for x in v]
    if isinstance(v, dict):
        return {k: _norm(x, root) for k, x in v.items()}
    return v


def _tree(root: Path) -> list[Any]:
    """Every entry under root, sorted: [relative path, kind, bytes (base64) or link target]."""
    out: list[Any] = []
    if not os.path.lexists(root):
        return out
    for dirpath, dirs, files in os.walk(root, followlinks=False):
        for name in sorted(dirs + files):
            p = Path(dirpath) / name
            rel = str(p.relative_to(root))
            if p.is_symlink():
                out.append([rel, "link", os.readlink(p).replace(str(root), "$ROOT")])
            elif p.is_dir():
                out.append([rel, "dir", stat.S_IMODE(p.lstat().st_mode)])
            elif os.access(p, os.R_OK):
                out.append([rel, "file", base64.b64encode(p.read_bytes()).decode()])
            else:
                out.append([rel, "file", None])
    return sorted(out)


# ---------- profiles.py's Mac half ----------

APPS: dict[str, Any] = {
    "Applications/Comet.app": ("xml", {"CFBundleIdentifier": "ai.perplexity.comet", "CFBundleDisplayName": "Comet",
                                       "CFBundleName": "Comet Browser",
                                       "CFBundleDocumentTypes": [{"LSItemContentTypes": ["org.chromium.extension"]}]}),
    "Applications/Thorium.app": ("binary", {"CFBundleIdentifier": "org.chromium.Thorium", "CFBundleName": "Thorium",
                                            "UTExportedTypeDeclarations": [{"UTTypeIdentifier": "org.chromium.extension"},
                                                                           "stray"]}),
    "Applications/Electron App.app": ("xml", {"CFBundleIdentifier": "com.example.electron", "CFBundleName": "Electron",
                                              "CFBundleDocumentTypes": [{"LSItemContentTypes": ["public.html"]}]}),
    "Applications/Google Chrome.app": ("binary", {"CFBundleIdentifier": "COM.google.chrome",
                                                  "CFBundleDocumentTypes": [{"LSItemContentTypes":
                                                                             ["org.chromium.extension"]}]}),
    "home/.Trash/Old.app": ("xml", {"CFBundleIdentifier": "com.example.old",
                                    "UTImportedTypeDeclarations": [{"UTTypeIdentifier": "org.chromium.extension"}]}),
    "Applications/NoPlist.app": None,
    "Applications/Dup.app": ("xml", {"CFBundleIdentifier": "AI.Perplexity.Comet", "CFBundleName": "Dup",
                                     "CFBundleDocumentTypes": [{"LSItemContentTypes": ["org.chromium.extension"]}]}),
    "Applications/Stemmed.v2.app": ("xml", {"CFBundleIdentifier": "com.example.stemmed",
                                            "UTImportedTypeDeclarations": [
                                                {"UTTypeIdentifier": "org.chromium.extension"}]}),
    "Applications/IntId.app": ("binary", {"CFBundleIdentifier": 42, "CFBundleDisplayName": "", "CFBundleName": "Int App",
                                          "CFBundleDocumentTypes": [{"LSItemContentTypes":
                                                                     {"org.chromium.extension": 1}}]}),
    "Applications/Broken.app": "not a plist at all",
    "Applications/NotDict.app": ("xml", ["org.chromium.extension"]),
    "Applications/zeta.app": ("xml", {"CFBundleIdentifier": "com.example.zeta", "CFBundleName": "zeta",
                                      "CFBundleDocumentTypes": ["skip", {"LSItemContentTypes": []},
                                                                {"LSItemContentTypes": ["x", "org.chromium.extension"]}]}),
    "Applications/Str.app": ("xml", {"CFBundleIdentifier": "com.example.str", "CFBundleName": "Str",
                                     "CFBundleDocumentTypes": "org.chromium.extension",
                                     "UTExportedTypeDeclarations": {"UTTypeIdentifier": "org.chromium.extension"}}),
    "Applications/BoolId.app": ("binary", {"CFBundleIdentifier": True, "CFBundleName": "Bool",
                                           "CFBundleDocumentTypes": [{"LSItemContentTypes":
                                                                      ["org.chromium.extension"]}]}),
    "Applications/Falsy.app": ("xml", {"CFBundleIdentifier": False, "CFBundleName": "Falsy",
                                       "CFBundleDocumentTypes": [{"LSItemContentTypes": ["org.chromium.extension"]}]}),
    "Applications/Weird.app": ("xml", {"CFBundleIdentifier": "com.example.weird", "CFBundleName": "Weird",
                                       "UTExportedTypeDeclarations": 5,
                                       "CFBundleDocumentTypes": [{"LSItemContentTypes": ["org.chromium.extension"]}]}),
    "Applications/Weird2.app": ("xml", {"CFBundleIdentifier": "com.example.weird2", "CFBundleName": "Weird Two",
                                        "CFBundleDocumentTypes": [{"LSItemContentTypes": True}]}),
}


def _write_apps(root: Path) -> dict[str, Any]:
    files: dict[str, Any] = {}
    for rel, spec in APPS.items():
        app = root / rel
        (app / "Contents").mkdir(parents=True)
        if spec is None:
            continue
        if isinstance(spec, str):
            data = spec.encode()
        else:
            fmt, value = spec
            data = plistlib.dumps(value, fmt=plistlib.FMT_XML if fmt == "xml" else plistlib.FMT_BINARY)
        (app / "Contents" / "Info.plist").write_bytes(data)
        files[rel] = base64.b64encode(data).decode()
    return files


def profiles_mac_fixture() -> dict[str, Any]:
    from ferb.act.browser import profiles
    saved = (profiles.locate, profiles.handlers, profiles.subprocess.run, dict(profiles._located))
    try:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp).resolve()
            out: dict[str, Any] = {"apps": _write_apps(root)}
            plain = [rel for rel in APPS if not rel.startswith("Applications/Weird")]
            runs = [("listed", plain), ("type_error", plain + ["Applications/Weird.app"]),
                    ("type_error_inner", plain + ["Applications/Weird2.app"]), ("raises", None), ("empty", [])]
            out["unsupported"] = []
            for name, listed in runs:
                def handlers(listed: Any = listed) -> list[Path]:
                    if listed is None:
                        raise RuntimeError("Launch Services unavailable")
                    return [root / rel for rel in listed]
                profiles.handlers = handlers                                       # type: ignore[assignment]
                try:
                    got: Any = {"ok": profiles.unsupported()}
                except Exception as e:                                           # noqa: BLE001
                    got = {"error": type(e).__name__, "message": str(e)}
                out["unsupported"].append({"name": name, "handlers": listed, "out": got})
            out["family"] = {}
            for rel, spec in APPS.items():
                info = profiles._info(root / rel)
                try:
                    fam: Any = profiles.chromium_family(info)
                except Exception as e:                                           # noqa: BLE001
                    fam = {"error": type(e).__name__}
                out["family"][rel] = fam
            # Browser.binary's memory of Launch Services' answers
            exe = root / "bin" / "Google Chrome"
            exe.parent.mkdir()
            exe.write_text("#!/bin/sh\n")
            (root / "bin" / "edge-dir").mkdir()
            answers: dict[str, list[Any]] = {
                "com.google.Chrome": [exe, None], "com.brave.Browser": [RuntimeError("no PyObjC")],
                "com.microsoft.edgemac": [root / "bin" / "edge-dir", None]}
            calls: list[str] = []

            def locate(bundle_id: str) -> Path | None:
                calls.append(bundle_id)
                queue = answers.get(bundle_id, [])
                a = queue.pop(0) if queue else None
                if isinstance(a, Exception):
                    raise a
                return a
            profiles.locate = locate                                               # type: ignore[assignment]
            profiles._located.clear()
            steps: list[Any] = []

            def binary(key: str, note: str = "") -> None:
                start = len(calls)
                steps.append({"browser": key, "note": note, "binary": profiles.BROWSERS[key].binary,
                              "calls": calls[start:]})
            binary("chrome")
            binary("chrome", "remembered")
            exe.unlink()
            binary("chrome", "gone: asked again")
            binary("brave")
            binary("edge")
            binary("edge", "not a file: asked again")
            binary("opera")
            out["binary"] = _norm({"answers": {k: [None if a is None else ("raise" if isinstance(a, Exception)
                                                                             else str(a)) for a in v]
                                               for k, v in {"com.google.Chrome": [exe, None],
                                                            "com.brave.Browser": [RuntimeError()],
                                                            "com.microsoft.edgemac": [root / "bin" / "edge-dir", None]
                                                            }.items()},
                                   "steps": steps}, root)
            # running: the pgrep pattern
            profiles.locate = lambda bundle_id: None                               # type: ignore[assignment]
            profiles._located.clear()
            ran: list[list[str]] = []

            class Done:
                def __init__(self, code: int) -> None:
                    self.returncode = code

            def run(args: list[str], **kw: Any) -> Done:
                ran.append(list(args))
                return Done(0 if "Brave" in args[2] or "Canary" in args[2] else 1)
            profiles.subprocess.run = run                                          # type: ignore[assignment]
            out["running"] = []
            for key in profiles.BROWSERS:
                start = len(ran)
                r = profiles.running(key)
                out["running"].append({"browser": key, "running": r, "argv": ran[start:]})
            # _app_installed's check of the bundle Launch Services named
            (root / "home" / ".Trash" / "Old.app").mkdir(parents=True, exist_ok=True)
            (root / "file.app").write_text("x")
            out["app_path_ok"] = {}
            for rel in ["Applications/Comet.app", "home/.Trash/Old.app", "file.app", "missing.app",
                        "Applications/Comet.app/Contents"]:
                p = root / rel
                out["app_path_ok"][rel] = p.is_dir() and ".Trash" not in p.parts
            return out
    finally:
        profiles.locate, profiles.handlers, profiles.subprocess.run = saved[:3]   # type: ignore[assignment]
        profiles._located.clear()
        profiles._located.update(saved[3])


# ---------- uploads.py ----------

MTIME_NS = 1_700_000_000_123_456_789
OPEN_FILE = os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK | os.O_CLOEXEC


def _files(root: Path) -> list[Any]:
    """_tree plus each copy's mode and mtime (a copy keeps the original's)."""
    out = []
    for entry in _tree(root):
        if entry[1] == "file" and entry[0].startswith("ws/uploads/"):
            st = (root / entry[0]).lstat()
            entry = entry + [stat.S_IMODE(st.st_mode), st.st_mtime_ns]
        out.append(entry)
    return out


UPLOAD_SCENARIOS: list[tuple[str, list[dict[str, Any]]]] = [
    ("copy_and_origin", [
        {"write": "Desktop/clg:uni/submission.csv", "data": "id,label\n1,a\n"},
        {"write": "ws/downloads/dl-0123456789ab-report.pdf", "data": "%PDF from a website"},
        {"untrusted": ["$ROOT/ws/downloads", "~/nowhere"]},
        {"stage": {"tree": "tree1", "source": "Desktop/clg:uni/submission.csv"}},
        {"stage": {"tree": "tree1", "source": "ws/downloads/dl-0123456789ab-report.pdf"}},
        {"forget": "tree1"},
    ]),
    ("planted_tree_link", [
        {"write": "Desktop/cv.pdf", "data": "%PDF cv"}, {"mkdir": "ws/uploads"}, {"mkdir": "elsewhere"},
        {"symlink": ["ws/uploads/tree1", "$ROOT/elsewhere"]},
        {"stage": {"tree": "tree1", "source": "Desktop/cv.pdf"}},
    ]),
    ("changed_while_copied", [
        {"write": "a.txt", "data": "now"},
        {"stage": {"tree": "tree1", "source": "a.txt", "sha": "before"}},
    ]),
    ("trees_end_and_start_up", [
        {"write": "a.txt", "data": "x"},
        {"stage": {"tree": "tree1", "source": "a.txt"}}, {"stage": {"tree": "tree2", "source": "a.txt"}},
        {"forget": "tree1"}, {"sweep": 1},
    ]),
    ("links_for_tree_folders", [
        {"victim": 1}, {"mkdir": "ws/uploads/outside"},
        {"symlink": ["ws/uploads/tree1", "$ROOT/Documents/tree1"]},
        {"symlink": ["ws/uploads/outside/tree1", "$ROOT/Documents/tree1"]},
        {"forget": "tree1"},
        {"symlink": ["ws/uploads/tree2", "$ROOT/Documents/tree2"]},
        {"symlink": ["ws/uploads/outside/tree2", "$ROOT/Documents/tree2"]},
        {"symlink": ["ws/uploads/stray", "$ROOT/Documents/tree1/keep.txt"]},
        {"sweep": 1},
    ]),
    ("link_for_outside_folder", [
        {"victim": 1}, {"mkdir": "ws/uploads"}, {"symlink": ["ws/uploads/outside", "$ROOT/Documents"]},
        {"forget": "tree1"}, {"sweep": 1},
    ]),
    ("link_for_uploads_folder", [
        {"victim": 1}, {"mkdir": "ws"}, {"symlink": ["ws/uploads", "$ROOT/Documents"]},
        {"forget": "tree1"}, {"sweep": 1}, {"collect": 1},
    ]),
    *[(f"planted_{planted}", [
        {"write": "downloads/report.pdf", "data": "%PDF"}, {"mkdir": "elsewhere"},
        {"untrusted": ["$ROOT/downloads"]},
        {"mkdir": {"uploads": "ws", "outside": "ws/uploads", "tree1": "ws/uploads/outside"}[planted]},
        {"symlink": [{"uploads": "ws/uploads", "outside": "ws/uploads/outside",
                      "tree1": "ws/uploads/outside/tree1"}[planted], "$ROOT/elsewhere"]},
        {"stage": {"tree": "tree1", "source": "downloads/report.pdf"}},
    ]) for planted in ("uploads", "outside", "tree1")],
    ("no_task_owns_the_tab", [
        {"write": "a.txt", "data": "x"},
        *[x for tree in ("", ".", "..", "a/b", ".hidden", "outside")
          for x in ({"stage": {"tree": tree, "source": "a.txt"}}, {"forget": tree})],
    ]),
    *[(f"folder_swapped_mid_copy_{way.replace(' ', '_')}", [
        {"write": "Desktop/cv.pdf", "data": "%PDF the owner's cv"}, {"mkdir": "Documents"},
        {"race": "swap_folder", "way": way},
        {"stage": {"tree": "tree1", "source": "Desktop/cv.pdf"}},
    ]) for way in ("clone", "byte copy")],
    ("tab_outlives_tree", [
        {"write": "cv.pdf", "data": "%PDF"},
        {"stage": {"tree": "tree1", "source": "cv.pdf", "tab": "chrome/Profile 1#200"}},
        {"stage": {"tree": "tree1", "source": "cv.pdf", "tab": "chrome/Profile 1#7"}},
        {"closed": "chrome/Profile 1#7"}, {"forget": "tree1"}, {"collect": 1},
        {"closed": "chrome/Profile 1#200"}, {"collect": 1}, {"closed": "chrome/Profile 1#9"},
    ]),
    ("several_and_a_failure", [
        {"write": "a.txt", "data": "first"}, {"write": "b.txt", "data": "second"},
        {"stage_all": {"tree": "tree1", "tab": "p#1", "sources": [{"source": "a.txt"}, {"source": "b.txt"}]}},
        {"stage_all": {"tree": "tree1", "tab": "p#2", "sources": [{"source": "a.txt"},
                                                                    {"source": "b.txt", "sha": "before"}]}},
        {"closed": "p#2"}, {"forget": "tree1"}, {"closed": "p#1"},
    ]),
    ("cleanup_names_what_it_couldnt", [
        {"write": "a.txt", "data": "x"},
        {"stage": {"tree": "tree1", "source": "a.txt"}}, {"stage": {"tree": "tree1", "source": "a.txt"}},
        {"lock_copy": 0}, {"forget": "tree1"},
        {"stage": {"tree": "tree2", "source": "a.txt"}}, {"sweep": 1}, {"unlock": 1}, {"sweep": 1},
    ]),
    ("link_at_the_copys_name", [
        {"write": "cv.pdf", "data": "%PDF the owner's cv"}, {"mkdir": "Documents"},
        {"race": "plant_link"}, {"stage": {"tree": "tree1", "source": "cv.pdf"}},
    ]),
    ("link_swapped_in_after_the_copy", [
        {"write": "cv.pdf", "data": "%PDF the owner's cv"}, {"write": "elsewhere.pdf", "data": "%PDF the owner's cv"},
        {"race": "swap_after"}, {"stage": {"tree": "tree1", "source": "cv.pdf"}},
    ]),
    ("closing_a_tab_asks_for_a_collect", [
        {"write": "a.txt", "data": "x"},
        {"stage": {"tree": "tree1", "source": "a.txt", "tab": "p#1"}},
        {"stage": {"tree": "tree2", "source": "a.txt", "tab": "p#2"}},
        {"forget": "tree1"}, {"closed": "p#9"}, {"closed": "p#2"}, {"closed": "p#1"}, {"collect": 1},
    ]),
    ("no_clone_a_byte_copy", [
        {"write": "cv.pdf", "data": "%PDF", "mode": 0o640}, {"race": "no_clone"},
        {"stage": {"tree": "tree1", "source": "cv.pdf"}},
    ]),
    ("identity_then_copy_with_the_lists_check", [
        {"write": "home/docs/cv.pdf", "data": "%PDF mine"}, {"write": "home/.ferb/run/cv.pdf", "data": "secret-token"},
        {"write": "home/notes.txt", "data": "my notes"},
        {"sources": {"paths": ["home/notes.txt", "home/docs/cv.pdf"], "tree": "tree1", "tab": "p#7"}},
        {"rmtree": "home/docs"}, {"symlink": ["home/docs", "$ROOT/home/.ferb/run"]},
        {"sources": {"paths": ["home/docs/cv.pdf"], "tree": "tree1", "tab": "p#7"}},
        {"write": "locked.pdf", "data": "%PDF", "mode": 0},
        {"sources": {"paths": ["locked.pdf"], "tree": "tree1", "tab": "p#7"}},
        {"chmod": ["locked.pdf", 0o600]},
        {"sources": {"paths": ["missing.pdf"], "tree": "tree1", "tab": "p#7"}},
        {"sources": {"paths": ["home/notes.txt"], "tree": "../x", "tab": "p#7"}},
    ]),
]


def uploads_fixture() -> dict[str, Any]:
    import hashlib
    from ferb.act.browser import uploads as up
    from ferb.act.browser.ext_bridge import _TabDriver
    from ferb.act.fs import recovery
    out: list[Any] = []
    real_fclone, real_copy_into, real_fclonefileat = recovery._fclone, recovery.copy_into, recovery._fclonefileat
    for name, steps in UPLOAD_SCENARIOS:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp).resolve()
            uroot = root / "ws" / "uploads"
            untrusted: list[str] = []
            ups = up.Uploads(uroot, lambda: untrusted)
            locked: list[Path] = []
            log: list[Any] = []

            def outcome(fn: Any) -> Any:
                try:
                    return {"ok": fn()}
                except up.CleanupLeft as e:
                    return {"error": "CleanupLeft", "message": str(e), "left": e.left}
                except Exception as e:                                       # noqa: BLE001
                    return {"error": type(e).__name__, "message": str(e)}

            def sha_of(spec: dict[str, Any]) -> str:
                if spec.get("sha") == "before":
                    return hashlib.sha256(b"before").hexdigest()
                return hashlib.sha256((root / spec["source"]).read_bytes()).hexdigest()

            def staged(tree: str, specs: list[dict[str, Any]], tab: str | None) -> Any:
                fds = [os.open(root / s["source"], OPEN_FILE) for s in specs]
                try:
                    return ups.stage_all(tree, [(fd, str((root / s["source"]).resolve()), sha_of(s))
                                               for fd, s in zip(fds, specs)], tab)
                finally:
                    for fd in fds:
                        os.close(fd)
            for step in steps:
                kind = next(iter(step))
                got: Any = None
                if kind == "write":
                    p = root / step["write"]
                    p.parent.mkdir(parents=True, exist_ok=True)
                    p.write_text(step["data"])
                    os.chmod(p, step.get("mode", 0o644))
                    os.utime(p, ns=(MTIME_NS, MTIME_NS))
                elif kind == "mkdir":
                    (root / step["mkdir"]).mkdir(parents=True, exist_ok=True)
                elif kind == "symlink":
                    a, b = step["symlink"]
                    (root / a).symlink_to(b.replace("$ROOT", str(root)))
                elif kind == "rmtree":
                    shutil.rmtree(root / step["rmtree"])
                elif kind == "chmod":
                    os.chmod(root / step["chmod"][0], step["chmod"][1])
                elif kind == "victim":
                    for n in ("tree1", "tree2", "outside"):
                        (root / "Documents" / n).mkdir(parents=True)
                        (root / "Documents" / n / "keep.txt").write_text("the owner's")
                elif kind == "untrusted":
                    untrusted[:] = [u.replace("$ROOT", str(root)) for u in step["untrusted"]]
                elif kind == "race":
                    race = step["race"]

                    def racing(src_fd: int, dir_fd: int, fname: str, way: str = step.get("way", "")) -> None:
                        [leaf] = os.listdir(uroot / "tree1")
                        victim = root / "Documents"
                        if race == "swap_folder":
                            (victim / leaf).mkdir()
                            if way == "byte copy":
                                planted = victim / leaf / fname
                                planted.write_bytes(b"the owner's other file")
                                os.chmod(planted, 0o640)
                                os.utime(planted, ns=(1_600_000_000_000_000_000, 1_600_000_000_000_000_000))
                            os.rename(uroot / "tree1", uroot / "moved")
                            (uroot / "tree1").symlink_to(victim)
                            if way == "byte copy":
                                raise OSError(45, "this volume can't clone")
                        elif race == "plant_link":
                            (uroot / "tree1" / leaf / fname).symlink_to(victim / "planted.pdf")
                        real_fclone(src_fd, dir_fd, fname)

                    def swapping(src_fd: int, dir_fd: int, fname: str) -> str:
                        digest = real_copy_into(src_fd, dir_fd, fname)
                        os.unlink(fname, dir_fd=dir_fd)
                        os.symlink(root / "elsewhere.pdf", fname, dir_fd=dir_fd)
                        return digest
                    if race in ("swap_folder", "plant_link"):
                        recovery._fclone = racing                                 # type: ignore[assignment]
                    elif race == "swap_after":
                        recovery.copy_into = swapping                             # type: ignore[assignment]
                    elif race == "no_clone":
                        recovery._fclonefileat = None
                elif kind == "stage":
                    s = step["stage"]
                    got = outcome(lambda s=s: staged(s["tree"], [s], s.get("tab"))[0])
                elif kind == "stage_all":
                    s = step["stage_all"]
                    got = outcome(lambda s=s: staged(s["tree"], s["sources"], s.get("tab")))
                elif kind == "forget":
                    got = outcome(lambda t=step["forget"]: ups.forget_tree(t))
                elif kind == "closed":
                    got = outcome(lambda t=step["closed"]: ups.closed(t))
                elif kind == "collect":
                    got = outcome(ups.collect)
                elif kind == "sweep":
                    got = outcome(ups.sweep)
                elif kind == "lock_copy":
                    copy = Path(next(e["out"]["ok"] for e in log if e["step"].get("stage")).replace("$ROOT", str(root)))
                    os.chflags(copy, stat.UF_IMMUTABLE)
                    locked.append(copy)
                elif kind == "unlock":
                    for p in locked:
                        if os.path.lexists(p):
                            os.chflags(p, 0)
                elif kind == "sources":
                    s = step["sources"]

                    def closed(path: Path, access: str) -> str | None:
                        return "closed by ferb_state" if "/.ferb/" in str(path) else None

                    def run(s: dict[str, Any] = s) -> Any:
                        ids, given = _TabDriver._sources([str(root / p) for p in s["paths"]],
                                                         (ups, s["tree"], s["tab"]), closed)
                        return {"identities": [{**i, "owner_uid": "$UID"} for i in ids], "given": given}
                    got = outcome(run)
                if kind in ("stage", "stage_all", "sources"):         # a race lasts one staging
                    recovery._fclone, recovery.copy_into = real_fclone, real_copy_into
                    recovery._fclonefileat = real_fclonefileat
                log.append({"step": step, "out": got})
                log[-1] = _norm(log[-1], root)
                log[-1]["files"] = _norm(_files(root), root)
            for p in locked:
                if os.path.lexists(p):
                    os.chflags(p, 0)
            out.append({"name": name, "steps": log})
    recovery._fclone, recovery.copy_into, recovery._fclonefileat = real_fclone, real_copy_into, real_fclonefileat
    return {"mtime_ns": MTIME_NS, "scenarios": out}


# ---------- downloads.py ----------

PART = 64                       # the scripted extension's part size (protocol.js sends 512 KiB; any size behaves alike)
TAB_WAIT_S = 0.05               # downloads.TAB_WAIT_S while a scenario runs (the Rust test scales: its 20 s is real)
DELAY_S = 0.04                  # a slow peer's wait per part, on the same scale


def blob(n: int, seed: int = 0) -> bytes:
    """Invented bytes: (i * 7 + seed) % 256."""
    return bytes((i * 7 + seed) % 256 for i in range(n))


class Peer:
    """The extension's side of download_part, as tests/fake_extension.py's op_download_part answers it, with a fake
    clock: a tick before each part, parts that come back empty, a part that never answers (the clock passes its
    timeout), a stream that breaks at an offset, a part at the wrong offset, a slow part (real time, scaled)."""

    def __init__(self, h: "DlHarness", name: str) -> None:
        self.h, self.name = h, name
        self.held: dict[str, bytes] = {}
        self.tick, self.empty_parts, self.silent_parts, self.fail_at, self.wrong_offset = 0.0, 0, 0, None, False
        self.delay, self.refuse = 0.0, False

    async def call(self, op: str, args: dict[str, Any], timeout: float | None = None, cid: str | None = None) -> Any:
        self.h.calls.append([self.name, op, dict(args), timeout])
        await asyncio.sleep(self.delay) if self.delay else await asyncio.sleep(0)
        did = args["download_id"]
        if self.refuse:
            raise RuntimeError("the connection to the browser closed")
        if did not in self.held:
            raise RuntimeError("download transfer unavailable")
        if args.get("release"):
            self.held.pop(did)
            return {"released": True, "handed_back": True}
        if args.get("abort"):
            self.held.pop(did)
            return {"aborted": True}
        if self.tick:
            self.h.clock.advance(self.tick)
        if self.silent_parts > 0:
            self.silent_parts -= 1
            self.h.clock.advance(timeout or 0)
            raise asyncio.TimeoutError()
        data, offset = self.held[did], int(args["offset"])
        if self.empty_parts > 0:
            self.empty_parts -= 1
            return {"data": "", "offset": offset, "eof": False}
        if self.fail_at is not None and offset >= self.fail_at:
            self.held.pop(did)
            raise RuntimeError("the response stream broke")
        if self.wrong_offset:
            return {"data": "", "offset": offset + 1, "eof": False}
        chunk = data[offset:offset + PART]
        eof = offset + len(chunk) >= len(data)
        if eof:
            self.held.pop(did)
        return {"data": base64.b64encode(chunk).decode(), "offset": offset, "eof": eof}


class DlHarness:
    def __init__(self, home: Path) -> None:
        from ferb.act.browser import downloads as dl
        from ferb.clock.clock import FakeClock
        from ferb.store.db import Database
        self.home = home
        self.db = Database(home / "ferb.db")
        self.db.migrate()
        self.clock = FakeClock()
        self.calls: list[Any] = []
        self.pubs: list[Any] = []
        self.hooks: list[Any] = []
        self.config: dict[str, Any] = {}
        self.free: Any = 10 ** 15
        self.tabs: dict[tuple[str, int], Any] = {}
        self.peers: dict[str, Peer] = {}
        self.broken_finish = False
        self.d = dl.Downloads(home / "ws" / "downloads", self.db, self.clock.now, self.setting,
                              lambda t: t.replace("SECRET", "[redacted]"),
                              lambda topic, data: self.pubs.append([topic, data]))
        self.d.task_tab = lambda br, tab: self.tabs.get((br.name, int(tab)))
        self.d.task_root = lambda task: task.split(".", 1)[0]
        self.d.free_bytes = self.free_bytes
        self.d.monotonic = self.clock.monotonic

        def over() -> None:
            o = self.d.over_cap()
            self.hooks.append(["over_cap", None if o is None else list(o._asdict().values())])
        self.d.on_over_cap = over

    def setting(self, section: str, key: str, default: Any) -> Any:
        if self.config.get("broken"):
            raise ValueError("config.toml is half-edited")
        assert section == "browser"
        return self.config.get(key, default)

    def free_bytes(self) -> int:
        if self.free == "error":
            raise OSError(5, "Input/output error")
        return int(self.free)

    def peer(self, name: str) -> Peer:
        return self.peers.setdefault(name, Peer(self, name))

    def rows(self) -> Any:
        return [dict(r) for r in self.db.conn.execute("SELECT * FROM downloads ORDER BY rowid")]

    def files(self) -> Any:
        import hashlib
        root = self.home / "ws" / "downloads"
        if not root.is_dir():
            return None
        out = []
        for name in sorted(os.listdir(root)):
            p = root / name
            if p.is_symlink():
                out.append([name, "link", os.readlink(p)])
            elif p.is_file():
                data = p.read_bytes()
                out.append([name, len(data), hashlib.sha256(data).hexdigest(), stat.S_IMODE(p.stat().st_mode)])
            else:
                out.append([name, "dir"])
        return out


def _ev(spec: dict[str, Any]) -> Any:
    from ferb.act.browser.ext_protocol import Event
    return Event.model_validate({"event": "download", **spec})


DOWNLOAD_SCENARIOS: list[tuple[str, list[dict[str, Any]]]] = [
    ("taken_with_facts", [
        {"tab": ["chrome/Profile 1", 7, "t1", "t2", "owner"]},
        {"event": {"tab_id": 7, "download_id": "d1", "name": "report.pdf", "mime": "application/pdf", "size": 200,
                   "source_url": "https://x.example/r.pdf?token=SECRET1", "source": "attachment"},
         "data": [200, 1]},
        {"wait": 1}, {"busy": ["chrome/Profile 1", 7]}, {"unreported": ["chrome/Profile 1", 7]},
        {"unreported": ["chrome/Profile 1", 7]}, {"facts_for": [["t1"], None]}, {"facts_for": [["t1"], "ferb"]},
        {"facts_for": [[], None]},
    ]),
    ("hostile_names", [
        {"tab": ["chrome/Profile 1", 7, "t1", "t2", "owner"]},
        *[x for i, n in enumerate(["../../.ssh/authorized_keys", "/etc/passwd", "..", ".", ".bashrc", "",
                                   "a\x00b\nc\r.txt", "evil\u202egpj.exe", "C:\\win\\x.bat", "a:b.txt",
                                   "x" * 400 + ".pdf", " .. / .. /x", "é" * 200 + ".docx", "a.b" + "c" * 200,
                                   "name.with-a-very-long-extension-part-x", "token SECRET.txt"])
          for x in ({"event": {"tab_id": 7, "download_id": f"h{i}", "name": n, "size": 3}, "data": [3, i]},
                    {"wait": 1})],
    ]),
    ("released_and_not_taken", [
        {"event": {"tab_id": 9, "download_id": "own", "name": "theirs.zip", "size": 5}, "data": [5, 0]}, {"wait": 1},
        {"event": {"tab_id": 9, "uncaptured": True, "name": "u.zip"}}, {"wait": 1},
        {"tab": ["chrome/Profile 1", 7, "t1", "t2", "owner"]},
        {"event": {"tab_id": 7, "uncaptured": True, "name": "e.csv", "source_url": "blob:https://shop.example/9",
                   "detail": "canceled"}}, {"wait": 1},
        {"event": {"tab_id": 7, "uncaptured": True, "name": "n.csv", "detail": "new_tab"}}, {"wait": 1},
        {"event": {"tab_id": 7, "uncaptured": True, "name": "s.csv", "detail": "started"}}, {"wait": 1},
        {"event": {"tab_id": 7, "uncaptured": True, "detail": "dropped"}}, {"wait": 1},
        {"event": {"tab_id": 7, "uncaptured": True, "detail": "dropped", "count": 4}}, {"wait": 1},
        {"event": {"tab_id": 7, "name": "noid.bin"}}, {"wait": 1},
        {"unreported": ["chrome/Profile 1", 7]},
    ]),
    ("undeclared_size_and_budget", [
        {"config": {"download_task_max_gb": 0.0000005}},      # 500 bytes
        {"tab": ["chrome/Profile 1", 7, "t1", "t2", "owner"]},
        {"event": {"tab_id": 7, "download_id": "a", "name": "a.bin", "size": 300}, "data": [300, 0]}, {"wait": 1},
        {"event": {"tab_id": 7, "download_id": "b", "name": "b.bin", "size": 300}, "data": [300, 1]}, {"wait": 1},
        {"event": {"tab_id": 7, "download_id": "c", "name": "c.bin"}, "data": [300, 2]}, {"wait": 1},
        {"event": {"tab_id": 7, "download_id": "c2", "name": "c2.bin"}, "data": [200, 3]}, {"wait": 1},
        {"limits": 1},
    ]),
    ("file_count", [
        {"config": {"download_task_max_files": 2}},
        {"tab": ["chrome/Profile 1", 7, "t1", "t2", "owner"]}, {"tab": ["chrome/Profile 1", 8, "t1.step2", "t3", "owner"]},
        *[x for i in range(2) for x in ({"event": {"tab_id": 7, "download_id": f"f{i}", "name": f"f{i}.txt", "size": 1},
                                          "data": [1, i]}, {"wait": 1})],
        {"event": {"tab_id": 8, "download_id": "f2", "name": "f2.txt", "size": 1}, "data": [1, 2]}, {"wait": 1},
    ]),
    ("broken_stream_and_wrong_offset", [
        {"tab": ["chrome/Profile 1", 7, "t1", "t2", "owner"]},
        {"peer": {"name": "chrome/Profile 1", "fail_at": 64}},
        {"event": {"tab_id": 7, "download_id": "cut", "name": "cut.bin", "size": 192}, "data": [192, 0]}, {"wait": 1},
        {"peer": {"name": "chrome/Profile 1", "fail_at": None, "wrong_offset": True}},
        {"event": {"tab_id": 7, "download_id": "odd", "name": "odd.bin", "size": 10}, "data": [10, 0]}, {"wait": 1},
    ]),
    ("stall_and_slow", [
        {"tab": ["chrome/Profile 1", 7, "t1", "t2", "owner"]},
        {"peer": {"name": "chrome/Profile 1", "tick": 50.0}},
        {"event": {"tab_id": 7, "download_id": "slow", "name": "slow.bin", "size": 900}, "data": [900, 0]},
        {"wait": 1},
        {"peer": {"name": "chrome/Profile 1", "tick": 25.0, "empty_parts": 10}},
        {"event": {"tab_id": 7, "download_id": "stuck", "name": "stuck.bin", "size": 128}, "data": [128, 0]},
        {"wait": 1},
        {"peer": {"name": "chrome/Profile 1", "tick": 0.0, "empty_parts": 0, "silent_parts": 1}},
        {"event": {"tab_id": 7, "download_id": "quiet", "name": "quiet.bin", "size": 128}, "data": [128, 0]},
        {"wait": 1},
        {"peer": {"name": "chrome/Profile 1", "tick": 30.0, "empty_parts": 1, "silent_parts": 1}},
        {"event": {"tab_id": 7, "download_id": "late", "name": "late.bin", "size": 128}, "data": [128, 0]},
        {"wait": 1},
    ]),
    ("shell_shares_the_budget", [
        {"config": {"download_task_max_gb": 0.000001}},       # 1,000 bytes
        {"tab": ["chrome/Profile 1", 7, "t1", "t2", "owner"]},
        {"shell_received": ["t1.step3", 0]}, {"shell_received": ["t1.step3", 900]}, {"shell_stop": "t1"},
        {"event": {"tab_id": 7, "download_id": "a", "name": "a.bin", "size": 200}, "data": [200, 0]}, {"wait": 1},
        {"event": {"tab_id": 7, "download_id": "b", "name": "b.bin"}, "data": [200, 1]}, {"wait": 1},
        {"shell_received": ["t1", 101]}, {"shell_stop": "t1.x"}, {"shell_stop": "t9"},
        {"free": 5 * 1000 ** 3}, {"shell_stop": "t9"}, {"free": "error"}, {"shell_stop": "t9"},
        {"config": {"download_task_max_gb": 0.00000002}}, {"free": 10 ** 15}, {"shell_stop": "t9"},
        {"forget_cache": 1}, {"shell_stop": "t1"},
    ]),
    ("disk_floor", [
        {"tab": ["chrome/Profile 1", 7, "t1", "t2", "owner"]},
        {"free": 10 * 1000 ** 3 + 100},
        {"event": {"tab_id": 7, "download_id": "a", "name": "a.bin", "size": 200}, "data": [200, 0]}, {"wait": 1},
        {"event": {"tab_id": 7, "download_id": "b", "name": "b.bin"}, "data": [200, 1]}, {"wait": 1},
        {"free": 10 * 1000 ** 3 - 1},
        {"event": {"tab_id": 7, "download_id": "c", "name": "c.bin"}, "data": [10, 2]}, {"wait": 1},
        {"free": "error"},
        {"event": {"tab_id": 7, "download_id": "d", "name": "d.bin", "size": 10}, "data": [10, 3]}, {"wait": 1},
        {"event": {"tab_id": 7, "download_id": "e", "name": "e.bin"}, "data": [10, 4]}, {"wait": 1},
        {"free": 50_000_000}, {"config": {"download_min_free_gb": 0.01}},
        {"event": {"tab_id": 7, "download_id": "f", "name": "f.bin", "size": 10}, "data": [10, 5]}, {"wait": 1},
    ]),
    ("free_space_fails_mid_stream", [
        {"tab": ["chrome/Profile 1", 7, "t1", "t2", "owner"]},
        {"free_after": [1, "error"]},
        {"event": {"tab_id": 7, "download_id": "a", "name": "a.bin"}, "data": [200, 0]}, {"wait": 1},
    ]),
    ("config_breaks", [
        {"tab": ["chrome/Profile 1", 7, "t1", "t2", "owner"]},
        {"config": {"broken": True}},
        {"event": {"tab_id": 7, "download_id": "a", "name": "a.bin", "size": 5}, "data": [5, 0]}, {"wait": 1},
        {"config": {"download_task_max_gb": "abc", "download_task_max_files": None, "download_total_gb": True,
                    "download_min_free_gb": -3}}, {"limits": 1},
        {"config": {"download_task_max_gb": "1e-6", "download_task_max_files": "7.9", "download_total_gb": " 2 ",
                    "download_min_free_gb": "1_0"}}, {"limits": 1},
        {"config": {"download_task_max_gb": "inf"}},
        {"event": {"tab_id": 7, "download_id": "b", "name": "b.bin", "size": 5}, "data": [5, 1]}, {"wait": 1},
    ]),
    ("record_cant_be_written", [
        {"tab": ["chrome/Profile 1", 7, "t1", "t2", "owner"]}, {"broken_finish": True},
        {"event": {"tab_id": 7, "download_id": "x", "name": "x.bin", "size": 5}, "data": [5, 0]}, {"wait": 1},
    ]),
    ("one_transfer_per_tab", [
        {"tab": ["chrome/Profile 1", 7, "t1", "t2", "owner"]},
        {"event": {"tab_id": 7, "download_id": "one", "name": "one.bin", "size": 192}, "data": [192, 0]},
        {"event": {"tab_id": 7, "download_id": "two", "name": "two.bin", "size": 64}, "data": [64, 1]},
        {"busy": ["chrome/Profile 1", 7]}, {"wait": 1}, {"busy": ["chrome/Profile 1", 7]},
        {"peer": {"name": "chrome/Profile 1", "delay": DELAY_S}},
        {"event": {"tab_id": 7, "download_id": "slow", "name": "slow.bin", "size": 192}, "data": [192, 2]},
        {"event": {"tab_id": 7, "download_id": "next", "name": "next.bin", "size": 4}, "data": [4, 3]},
        {"wait": 1},
    ]),
    ("two_tabs_share_the_budget", [
        {"config": {"download_task_max_gb": 0.0000008}},      # 800 bytes
        {"tab": ["chrome/Profile 1", 7, "t1", "t2", "owner"]}, {"tab": ["chrome/Profile 1", 8, "t1", "t3", "owner"]},
        {"event": {"tab_id": 7, "download_id": "a", "name": "a.bin"}, "data": [450, 0]},
        {"event": {"tab_id": 8, "download_id": "b", "name": "b.bin"}, "data": [450, 1]},
        {"wait": 1}, {"internal": 1},
    ]),
    ("settle_and_forget", [
        {"tab": ["chrome/Profile 1", 7, "t1", "t2", "owner"]},
        {"peer": {"name": "chrome/Profile 1", "delay": DELAY_S}},
        {"event": {"tab_id": 7, "download_id": "s", "name": "s.bin", "size": 128}, "data": [128, 0]},
        {"settle": ["chrome/Profile 1", 7, 60]}, {"busy": ["chrome/Profile 1", 7]},
        {"event": {"tab_id": 7, "download_id": "t", "name": "t.bin", "size": 128}, "data": [128, 1]},
        {"forget_task": "t1"}, {"wait": 1}, {"internal": 1}, {"forget_task": "t1"}, {"internal": 1},
        {"unreported": ["chrome/Profile 1", 7]},
    ]),
    ("owner_and_ferb_profiles", [
        {"tab": ["chrome/Profile 1", 7, "t1", "t2", "owner"]}, {"tab": ["chrome/Profile 2", 7, "t1", "t1", "ferb"]},
        {"event": {"tab_id": 7, "download_id": "o", "name": "o.txt", "size": 1}, "data": [1, 0]}, {"wait": 1},
        {"event": {"tab_id": 7, "download_id": "f", "name": "f.txt", "size": 1}, "data": [1, 1],
         "peer": "chrome/Profile 2"}, {"wait": 1},
        {"facts_for": [["t1"], "owner"]}, {"facts_for": [["t1"], "ferb"]}, {"facts_for": [["t1", "t0"], None]},
        {"peer": {"name": "chrome/Profile 1", "refuse": True}},
        {"event": {"tab_id": 99, "download_id": "gone", "name": "g.txt"}, "data": [1, 2]}, {"wait": 1},
    ]),
    ("cap_delete_sweep_and_restart", [
        {"config": {"download_total_gb": 0.0000005}},          # 500 bytes
        {"tab": ["chrome/Profile 1", 7, "t1", "t2", "owner"]},
        *[x for i in range(3) for x in ({"event": {"tab_id": 7, "download_id": f"c{i}", "name": f"{i}.bin",
                                                   "size": 200}, "data": [200, i]}, {"wait": 1}, {"advance": 1})],
        {"over_cap": 1},
        {"delete": ["$row-0", "$row-0", "nope"]}, {"over_cap": 1}, {"delete": []},
        {"fs": ["unlink", "$path-1"]}, {"fs": ["unlink", "$path-2"]}, {"fs": ["symlink", "$path-2", "$ROOT/keep.txt"]},
        {"fs": ["write", "keep.txt", "the owner's own file"]},
        {"event": {"tab_id": 7, "download_id": "c3", "name": "3.bin", "size": 400}, "data": [400, 3]}, {"wait": 1},
        {"over_cap": 1}, {"delete": ["$row-1", "$row-2"]}, {"over_cap": 1},
        {"sql": "INSERT INTO downloads (id, task_id, profile, identity, tab, state, path, name, bytes, origin, at) "
                "VALUES ('dl-outside00000', 't1', 'chrome/Profile 1', 'owner', 't2', 'taken', '$ROOT/elsewhere/x.bin', "
                "'x.bin', 999999, 'external:download', '2026-01-01T08:00:00+00:00')"},
        {"over_cap": 1}, {"over_cap": 1}, {"delete": ["dl-outside00000"]},
        {"sql": "INSERT INTO downloads (id, task_id, profile, identity, tab, state, name, bytes, origin, at) "
                "VALUES ('dl-running0000', 't1', 'chrome/Profile 1', 'owner', 't2', 'in_progress', 'r.bin', NULL, "
                "'external:download', '2026-01-01T08:00:00+00:00')"},
        {"fs": ["write", "ws/downloads/.dl-0123456789ab.part", "half"]},
        {"fs": ["write", "ws/downloads/dl-0123456789ab-kept.txt", "kept"]},
        {"fs": ["mkdir", "ws/downloads/.dl-folder.part"]},
        {"fs": ["symlink", "ws/downloads/.dl-link.part", "$ROOT/keep.txt"]},
        {"sweep": 1}, {"finalize_stale": 1}, {"finalize_stale": 1}, {"facts_for": [["t1"], None]},
    ]),
    ("sweep_a_link_in_place_of_the_folder", [
        {"fs": ["mkdir", "elsewhere"]}, {"fs": ["write", "elsewhere/.dl-x.part", "theirs"]},
        {"fs": ["mkdir", "ws"]}, {"fs": ["symlink", "ws/downloads", "$ROOT/elsewhere"]},
        {"sweep": 1}, {"delete": ["x"]},
        {"tab": ["chrome/Profile 1", 7, "t1", "t2", "owner"]},
        {"event": {"tab_id": 7, "download_id": "a", "name": "a.bin", "size": 5}, "data": [5, 0]}, {"wait": 1},
    ]),
]


def downloads_fixture() -> dict[str, Any]:
    from types import SimpleNamespace
    from ferb.act.browser import downloads as dl
    real_time, real_wait, real_finish = dl.time, dl.TAB_WAIT_S, dl.Downloads._finish
    out: list[Any] = []
    try:
        for name, steps in DOWNLOAD_SCENARIOS:
            with tempfile.TemporaryDirectory() as tmp:
                root = Path(tmp).resolve()
                h = DlHarness(root)
                dl.time = SimpleNamespace(monotonic=h.clock.monotonic)              # type: ignore[assignment]
                dl.TAB_WAIT_S = TAB_WAIT_S

                def finish(self: Any, br: Any, ev: Any, fid: str, state: str, **kw: Any) -> None:
                    if h.broken_finish and state == "taken":
                        import sqlite3
                        raise sqlite3.OperationalError("disk I/O error")
                    real_finish(self, br, ev, fid, state, **kw)
                dl.Downloads._finish = finish                                       # type: ignore[assignment]
                log = asyncio.run(_run_downloads(h, steps, root))
                h.db.conn.close()
                out.append({"name": name, "steps": log})
    finally:
        dl.time, dl.TAB_WAIT_S, dl.Downloads._finish = real_time, real_wait, real_finish   # type: ignore[assignment]
    pure: dict[str, Any] = {
        "safe_name": {n: dl.safe_name(n) for n in [
            "Quarterly report (final).pdf", "résumé.docx", "y" * 300 + ".xlsx", "", "...", "\x00", "e\u0301t\u00e9.txt",
            "a\u200bb\u2028c\u2029d\ufeffe.txt", "tab\there  and\n\nlines", ". hidden .", "\ud800x" if False else "x",
            "名前" * 60 + ".pdf", "z" * 160 + ".tar.gz", "no-ext-" + "w" * 160, "dots." + "." * 10 + "end",
            "a" * 149 + "é.pdf", "x" * 140 + "." + "e" * 17, "😀" * 50 + ".png"]},
        "gb": {str(n): dl._gb(n) for n in [0, 1, 999, 1_000_000, 99_999_999, 100_000_000, 500_000_000, 1_000_000_000,
                                           1_234_567_890, 32 * 1000 ** 3, 10 ** 15, 5 * 1000 ** 3 - 1, 12_345]},
        "untrusted_roots": [
            dl.untrusted_roots(["~/Downloads"], Path("/ws")),
            dl.untrusted_roots([], Path("/ws")),
            dl.untrusted_roots(["/ws/downloads", "/x"], Path("/ws")),
            dl.untrusted_roots(["/ws/uploads/outside", "/ws/downloads"], Path("/ws"))],
        "not_taken": {str(d): dl._not_taken(d) for d in ["canceled", "new_tab", "started", None]},
        "defaults": dl.DEFAULTS,
    }
    return {"part": PART, "tab_wait_s": TAB_WAIT_S, "scenarios": out, "pure": pure}


async def _run_downloads(h: DlHarness, steps: list[dict[str, Any]], root: Path) -> list[Any]:
    from ferb.act.browser import downloads as dl
    log: list[Any] = []
    last: dict[str, Any] = {}
    free_after: list[Any] = []
    real_free = h.d.free_bytes

    def counted() -> int:
        if free_after:
            if free_after[0] == 0:
                free_after.pop(0)
                h.free = free_after.pop(0)
            else:
                free_after[0] -= 1
        return real_free()
    h.d.free_bytes = counted

    def path_of(text: str) -> str:
        rows = h.rows()
        if text.startswith("$row-"):
            return str(rows[int(text[5:])]["id"])
        if text.startswith("$path-"):
            return str(rows[int(text[6:])]["path"])
        return text.replace("$ROOT", str(root))
    for step in steps:
        kind = next(iter(step))
        a = step[kind]
        got: Any = None
        start_calls, start_pubs, start_hooks = len(h.calls), len(h.pubs), len(h.hooks)
        try:
            if kind == "tab":
                h.tabs[(a[0], a[1])] = dl.TaskTab(a[2], a[3], a[4])
            elif kind == "peer":
                p = h.peer(a["name"])
                for k, v in a.items():
                    if k != "name":
                        setattr(p, k, v)
            elif kind == "config":
                h.config = dict(a)
            elif kind == "free":
                h.free = a
            elif kind == "free_after":
                free_after[:] = list(a)
            elif kind == "broken_finish":
                h.broken_finish = bool(a)
            elif kind == "event":
                peer = h.peer(step.get("peer", "chrome/Profile 1"))
                if "data" in step and a.get("download_id"):
                    peer.held[a["download_id"]] = blob(*step["data"])
                h.d.receive(peer, _ev(a))
            elif kind == "wait":
                for _ in range(200):
                    jobs = [j for js in h.d._active.values() for j in js]
                    if not jobs:
                        break
                    await asyncio.wait(jobs)
                await asyncio.sleep(0)
            elif kind == "settle":
                await h.d.settle(h.peer(a[0]), a[1], a[2])
            elif kind == "busy":
                got = h.d.busy(h.peer(a[0]), a[1])
            elif kind == "unreported":
                got = h.d.unreported(h.peer(a[0]), a[1])
            elif kind == "facts_for":
                got = h.d.facts_for(set(a[0]), a[1])
            elif kind == "shell_received":
                h.d.shell_received(a[0], a[1])
            elif kind == "shell_stop":
                got = h.d.shell_stop(a)
            elif kind == "forget_cache":
                h.d._shell.clear()                       # what a restart forgets; the row stays
            elif kind == "limits":
                got = list(h.d.limits())
            elif kind == "advance":
                h.clock.advance(a)
            elif kind == "over_cap":
                o = h.d.over_cap()
                got = None if o is None else list(o._asdict().values())
            elif kind == "delete":
                got = list(h.d.delete([path_of(x) for x in a]))
            elif kind == "fs":
                op, rel = a[0], path_of(a[1])
                p = Path(rel) if rel.startswith("/") else root / rel
                if op == "unlink":
                    p.unlink()
                elif op == "symlink":
                    p.symlink_to(path_of(a[2]))
                elif op == "write":
                    p.parent.mkdir(parents=True, exist_ok=True)
                    p.write_text(a[2])
                elif op == "mkdir":
                    p.mkdir(parents=True)
            elif kind == "sql":
                h.db.conn.execute(a.replace("$ROOT", str(root)))
            elif kind == "sweep":
                got = h.d.sweep()
            elif kind == "finalize_stale":
                got = h.d.finalize_stale()
            elif kind == "forget_task":
                h.d.forget_task(a)
            elif kind == "internal":
                got = {"inflight": dict(h.d._inflight), "running": dict(h.d._running),
                       "locks": sorted(f"{k[0]}#{k[1]}" for k in h.d._locks),
                       "unreported": sorted(f"{k[0]}#{k[1]}" for k in h.d._unreported),
                       "tab_task": sorted(f"{k[0]}#{k[1]}" for k in h.d._tab_task)}
            else:
                raise AssertionError(kind)
            res: Any = {"ok": got}
        except Exception as e:                                                   # noqa: BLE001
            res = {"error": type(e).__name__, "message": str(e)}
        entry = {"step": step, "out": res, "calls": h.calls[start_calls:], "pub": h.pubs[start_pubs:],
                 "hooks": h.hooks[start_hooks:], "rows": h.rows(),
                 "shell": [dict(r) for r in h.db.conn.execute("SELECT * FROM shell_downloads ORDER BY tree_id")],
                 "files": h.files()}
        entry = _norm(entry, root)
        for key in ("rows", "shell", "files"):                  # only what changed since the step before
            if last.get(key) == entry[key]:
                del entry[key]
            else:
                last[key] = entry[key]
        log.append(entry)
    return log
