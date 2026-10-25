"""L39 fix round 2 (rereview-1): nothing in the change record clones, reads or lists through a path that can change
between FERB's check and its use (R1, N1); history files by identity, not spelling (I1); new folders listed fast, as
facts (N7); pairing in bulk reads a few candidates per new file (N8). Built from the reviewer's probes p3, p5, p6, p7."""

import os
import subprocess
import sys
import time
from pathlib import Path

import pytest

from ferb.act.fs import changes as changes_mod
from ferb.act.fs.changes import Changes, named
from ferb.policy.protection import Protection
from tests.support.documents import docx

TOKEN = "rt-OPAQUE-VALUE-123"
RACER = Path(__file__).parent / "support" / "racer.py"


def _home(tmp_path: Path) -> tuple[Path, Path, Path, Changes]:
    home = (tmp_path / "home").resolve()
    ferb = home / ".ferb"
    (ferb / "codex-home").mkdir(parents=True)
    secret = ferb / "codex-home" / "auth.json"
    secret.write_text('{"refresh_token": "%s"}\n' % TOKEN)
    (home / ".ssh").mkdir()
    (home / ".ssh" / "id_ed25519_work_github").write_text("PRIVATE KEY " + TOKEN)
    protection = Protection(home, ferb, ["~/.ssh"], [], [], True)
    store = Changes(ferb / "changes", closed=lambda p: protection.read_denied(p) is not None, home=home)
    return home, ferb, secret, store


def _copies_hold_token(store: Changes, tree: str, task: str) -> bool:
    rec = store._records[task]
    return any(f.copy and TOKEN in (store._folder(tree, task) / f.copy).read_text(errors="replace")
               for f in rec.files.values())


def test_a_link_swapped_in_after_the_check_is_never_cloned(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    """R1, the reviewer's deterministic repro (p3): the swap lands between the check and the clone."""
    home, _, secret, store = _home(tmp_path)
    work = home / "Documents"
    work.mkdir()
    n = work / "n.txt"
    n.write_text("plain\n")
    real_access = os.access

    def access(p: object, mode: int, *a: object, **k: object) -> bool:
        if Path(str(p)) == n and not n.is_symlink():
            n.unlink()
            n.symlink_to(secret)                      # the racer's rename lands here
        return real_access(p, mode, *a, **k)          # type: ignore[arg-type]
    monkeypatch.setattr(os, "access", access)
    store.before("t.a", "t", [n])
    monkeypatch.setattr(os, "access", real_access)
    assert not _copies_hold_token(store, "t", "t.a")
    n.unlink()
    n.write_text("the worker's text\n")
    assert all(TOKEN not in store.describe(c) for c in store.changes("t.a"))


def test_sixteen_racers_never_get_a_closed_file_into_the_record(tmp_path: Path) -> None:
    """R1, the real race (p3/p6b) for a few seconds: the reviewer won in 6 tries (0.2 s) before the fix."""
    home, ferb, secret, store = _home(tmp_path)
    work = home / "Documents"
    work.mkdir()
    paths = []
    for i in range(16):
        p = work / f"r{i}.txt"
        p.write_text("plain\n")
        paths.append(p)
    racers = [subprocess.Popen([sys.executable, str(RACER), str(p), str(secret)]) for p in paths]
    tries, leaked, start = 0, False, time.monotonic()
    try:
        time.sleep(0.3)
        while time.monotonic() - start < 3 and not leaked:
            tries += 1
            s = Changes(ferb / "changes", closed=store.closed, home=home)
            s.before(f"x{tries}.a", f"x{tries}", paths)
            leaked = _copies_hold_token(s, f"x{tries}", f"x{tries}.a")
            texts = [s.describe(c) for c in s.changes(f"x{tries}.a")]
            leaked = leaked or any(TOKEN in t for t in texts)
            s.forget(f"x{tries}")
    finally:
        for r in racers:
            r.kill()
            r.wait()
    assert tries > 5 and not leaked


def test_a_folder_swapped_for_a_link_between_check_and_listing_lists_nothing(tmp_path: Path,
                                                                             monkeypatch: pytest.MonkeyPatch) -> None:
    """N1, the reviewer's p5: the closed folder's names never reach the record."""
    home, _, _, store = _home(tmp_path)
    work = home / "Documents" / "proj"
    work.mkdir(parents=True)
    real_skip = store._skip
    calls: list[Path] = []

    def skip(p: Path) -> bool:
        calls.append(p)
        r = real_skip(p)
        if p == work and calls.count(work) == 2 and not work.is_symlink():   # _list's own check
            os.rename(work, work.with_name("proj.real"))
            work.symlink_to(home / ".ssh")            # lands between the check and the listing
        return r
    monkeypatch.setattr(store, "_skip", skip)
    store.before("t.a", "t", [work])
    monkeypatch.setattr(store, "_skip", real_skip)
    names = [n for listing in store._records["t.a"].folders.values() for n in listing]
    assert "id_ed25519_work_github" not in names
    work.unlink()
    os.rename(work.with_name("proj.real"), work)
    assert all("id_ed25519" not in store.describe(c) for c in store.changes("t.a"))


@pytest.mark.parametrize("spelling", ["~/.ZSH_HISTORY", "~/.Zsh_History", "data volume", "hard link"])
def test_a_history_file_is_a_fact_however_it_is_spelled(tmp_path: Path, spelling: str) -> None:
    """I1: a shell history file by identity (device and inode), never by spelling."""
    home, _, _, store = _home(tmp_path)
    history = home / ".zsh_history"
    history.write_text("ls\n")
    if spelling == "data volume":
        path = Path("/System/Volumes/Data" + str(history))
        if not path.exists():
            pytest.skip("this volume has no Data firmlink spelling")
        names = [path]
    elif spelling == "hard link":
        os.link(history, home / "h2")
        names = [home / "h2"]
    else:
        names = named(f"tail {spelling}", home, home)
    store.before("t.a", "t", names)
    history.write_text("ls\nmysql -phunter2-SECRET\n")
    out = store.changes("t.a")
    assert out and not any(c.content for c in out)
    assert all("hunter2" not in store.describe(c) for c in out)
    assert len({os.stat(c.path).st_ino for c in out if os.path.exists(c.path)}) == len(out)   # listed once


def test_a_named_file_replaced_by_a_link_is_a_named_fact(tmp_path: Path) -> None:
    """N2: the step named it; it is now a link: a fact that says it was named, never a "may be the owner's" fact."""
    home, _, secret, store = _home(tmp_path)
    doc = home / "report.txt"
    doc.write_text("the owner's report\n")
    store.before("t.a", "t", [doc])
    doc.unlink()
    doc.symlink_to(secret)
    [change] = store.changes("t.a")
    assert change.named and not change.content
    line = store.describe(change)
    assert "named by the call" in line and "now a link to" in line and TOKEN not in line


def test_a_new_folder_of_thirty_thousand_files_is_listed_fast_as_facts(tmp_path: Path) -> None:
    """N7 (p7, `cd proj && npm install`): a page of names, then "and K more"; under a second."""
    proj = tmp_path / "proj"
    proj.mkdir()
    (proj / "package.json").write_text("{}")
    store = Changes(tmp_path / "changes")
    store.before("t.s", "t", named(f"cd {proj} && npm install", tmp_path))
    for i in range(300):
        d = proj / "node_modules" / f"pkg{i}" / "lib"
        d.mkdir(parents=True)
        for j in range(100):
            (d / f"f{j}.js").write_text("x")
    start = time.monotonic()
    out = store.changes("t.s")
    elapsed = time.monotonic() - start
    texts = [store.describe(c) for c in out]
    assert elapsed < 1.0, elapsed
    assert not any(c.content for c in out) and len(out) <= changes_mod.FOLDER_FACTS + 1
    assert any("and 29,9" in t and "more new files" in t for t in texts)


def test_pairing_in_a_bulk_step_reads_a_few_candidates_per_new_file(tmp_path: Path,
                                                                     monkeypatch: pytest.MonkeyPatch) -> None:
    """N8: ten originals named and ten corrected copies made: no N×N text extraction."""
    folder = tmp_path / "docs"
    folder.mkdir()
    originals = [docx(folder / f"chapter{i}.docx", [f"Chapter {i}", "Student ID", "ID"]) for i in range(10)]
    made = [folder / f"chapter{i}-fixed.docx" for i in range(10)]
    store = Changes(tmp_path / "changes")
    store.before("t.a", "t", [*originals, *made])
    for i, m in enumerate(made):
        docx(m, [f"Chapter {i}", "Student ID", "2024eb02238"])
    calls: list[Path] = []
    real_text = changes_mod._text

    def counted(path: Path, closed: object) -> object:
        calls.append(Path(path))
        return real_text(path, closed)                    # type: ignore[arg-type]
    monkeypatch.setattr(changes_mod, "_text", counted)
    out = {Path(c.path).name: c for c in store.changes("t.a")}
    assert len(calls) <= len(made) * (1 + changes_mod.PAIR_READS)
    assert all(Path(out[m.name].source or "").name == f"chapter{i}.docx" for i, m in enumerate(made))
