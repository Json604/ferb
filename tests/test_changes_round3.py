"""L39 fix round 3 (rereview-2): history files by identity across shells and REPLs (I1); new folders walked only
through fd-relative opens (N1); per-check limits on facts (N7); a new named DOCX/PDF readable to its end (N6); a renamed
named file stays visible (A1); a corrected copy under another name is diffed (A2); an unreadable named file is a named
fact (B2); nothing blocks on a FIFO. Built from the reviewer's probes in probes2/."""

import os
import sys
import threading
import time
from pathlib import Path

import pytest

from ferb.act.fs import changes as changes_mod
from ferb.act.fs.changes import Changes, named
from ferb.policy.protection import Protection
from tests.support.documents import docx

SECRET = "mysql -phunter2-SECRET"


def _store(tmp_path: Path) -> tuple[Path, Changes]:
    home = (tmp_path / "home").resolve()
    home.mkdir()
    return home, Changes(tmp_path / "changes", home=home)


@pytest.mark.parametrize("rel, name", [
    (".zsh_sessions/ABC.history", "w/h.txt"),                 # a hard link to a session file (sessions.py)
    (".python_history", ".PYTHON_HISTORY"),                   # a REPL history under another spelling
    (".local/share/fish/fish_history", "w/fish.txt"),
    (".psql_history", "w/p.txt"),
    (".lesshst", "w/l.txt"),
    (".histfile", "w/hf.txt"),
    (".bash_sessions/X.history", ".bash_sessions/X.history"),   # older macOS bash's per-session histories
    (".sh_history", ".Sh_History"),
    (".history", "w/tcsh.txt"),
])
def test_every_history_file_is_a_fact_by_identity(tmp_path: Path, rel: str, name: str) -> None:
    home, store = _store(tmp_path)
    history = home / rel
    history.parent.mkdir(parents=True, exist_ok=True)
    history.write_text("ls\n")
    alias = home / name
    if name.startswith("w/"):
        alias.parent.mkdir(exist_ok=True)
        os.link(history, alias)
    store.before("t.a", "t", [alias])
    with open(history, "a") as f:
        f.write(SECRET + "\n")
    out = store.changes("t.a")
    assert out and not any(c.content for c in out)
    assert all("hunter2" not in store.describe(c) for c in out)


def test_a_new_folder_is_walked_only_through_the_handle_that_was_checked(tmp_path: Path,
                                                                         monkeypatch: pytest.MonkeyPatch) -> None:
    """N1 (the reviewer's walkswap.py): the parent is swapped for a link to the home folder right after FERB opens a
    folder the step made; the walk lists that folder's own files through its handle, never the closed folder's names,
    and no folder at or under it is opened by path (it is opened relative to the listed folder's checked fd)."""
    home = (tmp_path / "home").resolve()
    ferb = home / ".ferb"
    ferb.mkdir(parents=True)
    (home / ".ssh").mkdir()
    (home / ".ssh" / "id_ed25519_work_github").write_text("k")
    protection = Protection(home, ferb, ["~/.ssh"], [], [])
    store = Changes(ferb / "changes", closed=lambda p: protection.read_denied(p) is not None, home=home)
    proj = home / "Documents" / "proj"
    proj.mkdir(parents=True)
    store.before("t.a", "t", [proj])
    (proj / ".ssh").mkdir()                                   # a folder the step made, named like the closed one
    (proj / ".ssh" / "mine.txt").write_text("x")
    (proj / ".ssh" / "sub").mkdir()
    (proj / ".ssh" / "sub" / "deeper.txt").write_text("y")
    real_open = os.open
    by_path: list[str] = []
    swapped: list[str] = []

    def opened(path: object, flags: int, *a: object, dir_fd: int | None = None, **k: object) -> int:
        fd = real_open(path, flags, *a, dir_fd=dir_fd, **k)    # type: ignore[arg-type]
        mine = sys._getframe(1).f_code.co_filename == changes_mod.__file__
        if mine and dir_fd is None and flags & os.O_DIRECTORY and str(path).startswith(str(proj / ".ssh")):
            by_path.append(str(path))
        if mine and flags & os.O_DIRECTORY and str(path) in (".ssh", str(proj / ".ssh")):
            swapped.append(str(path))
            if not proj.is_symlink():
                os.rename(proj, proj.with_name("proj.real"))
                proj.symlink_to(home)                         # the parent swapped for a link after FERB's open
        return fd
    monkeypatch.setattr(os, "open", opened)
    try:
        out = store.changes("t.a")
    finally:
        monkeypatch.setattr(os, "open", real_open)
        if proj.is_symlink():
            proj.unlink()
            os.rename(proj.with_name("proj.real"), proj)
    assert not any("id_ed25519" in c.path or "id_ed25519" in c.note for c in out)
    assert sorted(Path(c.path).name for c in out) == ["deeper.txt", "mine.txt"]
    assert by_path == []                                       # never by path: relative to a checked fd
    assert swapped                                             # the swap happened at FERB's open


def test_the_fact_limits_hold_per_check_across_all_new_folders(tmp_path: Path) -> None:
    """N7 (p7_variants.py): 1,000 new folders of 30 files: under a second, at most 50 names, the rest counted."""
    proj = tmp_path / "proj"
    proj.mkdir()
    (proj / "package.json").write_text("{}")
    store = Changes(tmp_path / "changes")
    store.before("t.s", "t", named(f"cd {proj} && make", tmp_path))
    for i in range(1000):
        d = proj / f"dir{i}"
        d.mkdir()
        for j in range(30):
            (d / f"f{j}.txt").write_text("x")
    start = time.monotonic()
    out = store.changes("t.s")
    elapsed = time.monotonic() - start
    texts = [store.describe(c) for c in out]
    names = [c for c in out if c.size is not None]
    assert elapsed < 1.0, elapsed
    assert len(names) <= changes_mod.FOLDER_FACTS
    assert any("more than 5,000 entries" in t for t in texts)


def test_new_plain_files_in_a_listed_folder_share_the_same_page(tmp_path: Path) -> None:
    """N7 (the flat case): 30,000 new files directly in a listed folder: 50 names and a count."""
    proj = tmp_path / "proj"
    proj.mkdir()
    store = Changes(tmp_path / "changes")
    store.before("t.s", "t", [proj])
    for i in range(30_000):
        (proj / f"f{i}.txt").write_text("x")
    start = time.monotonic()
    out = store.changes("t.s")
    assert time.monotonic() - start < 2.0
    assert len([c for c in out if c.size is not None]) <= changes_mod.FOLDER_FACTS
    assert any("29,950 more" in store.describe(c) for c in out)


def test_a_new_named_docx_is_readable_to_its_last_line(tmp_path: Path) -> None:
    """N6 (newdocx.py): the record holds the whole extracted text of a new named document."""
    _, store = _store(tmp_path)
    w = tmp_path / "w"
    w.mkdir()
    store.before("t.a", "t", [w / "submission.docx"])
    docx(w / "submission.docx", [f"paragraph {i} of the thesis" for i in range(1500)] + ["UNREQUESTED LINE AT THE END"])
    [change] = store.changes("t.a")
    assert "UNREQUESTED LINE AT THE END" in store.describe(change)


def test_a_renamed_named_file_stays_visible_and_is_diffed(tmp_path: Path) -> None:
    """A1 (rename.py): `mv notes.txt notes-final.txt` and an edit: "renamed", diffed against the original's copy."""
    _, store = _store(tmp_path)
    w = tmp_path / "w"
    w.mkdir()
    (w / "notes.txt").write_text("Student ID: ID\nline two\n")
    store.before("t.a", "t", named(f"cd {w} && mv notes.txt notes-final.txt && sed -i '' s/ID$/2024/ notes-final.txt",
                                   w))
    os.rename(w / "notes.txt", w / "notes-final.txt")
    (w / "notes-final.txt").write_text("Student ID: 2024\nline two\n")
    [change] = store.changes("t.a")
    text = store.describe(change)
    assert change.what == "renamed" and change.named and change.content
    assert Path(change.path).name == "notes-final.txt" and Path(change.source or "").name == "notes.txt"
    assert "renamed from" in text and "-Student ID: ID\n+Student ID: 2024" in text


def test_a_pure_rename_is_a_named_fact(tmp_path: Path) -> None:
    _, store = _store(tmp_path)
    w = tmp_path / "w"
    w.mkdir()
    (w / "a.txt").write_text("same\n")
    store.before("t.a", "t", [w / "a.txt", w / "b.txt"])
    os.rename(w / "a.txt", w / "b.txt")
    [change] = store.changes("t.a")
    assert change.what == "renamed" and change.named and not change.content
    assert "content unchanged" in store.describe(change)


def test_a_corrected_copy_under_another_name_is_diffed_against_its_original(tmp_path: Path) -> None:
    """A2 (pairname.py): the step named thesis.txt and submission.txt; the copy's unrequested change shows in a diff."""
    _, store = _store(tmp_path)
    w = tmp_path / "w"
    w.mkdir()
    body = "".join(f"paragraph {i} of the thesis text\n" for i in range(40))
    (w / "thesis.txt").write_text(body + "Student ID: ID\n")
    store.before("t.a", "t", [w / "thesis.txt", w / "submission.txt"])
    (w / "submission.txt").write_text(body.replace("paragraph 7 ", "paragraph SEVEN ") + "Student ID: 2024\n")
    [change] = store.changes("t.a")
    text = store.describe(change)
    assert Path(change.source or "").name == "thesis.txt"
    assert "-paragraph 7 of the thesis text\n+paragraph SEVEN of the thesis text" in text


def test_a_named_file_ferb_cant_read_is_a_named_fact(tmp_path: Path) -> None:
    """B2 (unreadable.py): a write-only named file that grew: named by the call, FERB can't read it."""
    _, store = _store(tmp_path)
    w = tmp_path / "w"
    w.mkdir()
    f = w / "log.txt"
    f.write_text("a\n")
    os.chmod(f, 0o200)
    try:
        store.before("t.a", "t", [f])
        with open(f, "a") as h:
            h.write("b\n")
        [change] = store.changes("t.a")
        assert change.named and not change.content
        assert "named by the call" in store.describe(change) and "can't read it" in store.describe(change)
    finally:
        os.chmod(f, 0o600)


def test_nothing_blocks_on_a_fifo(tmp_path: Path) -> None:
    """fifo.py: every read opens with O_NONBLOCK and checks for a plain file; a named file turned FIFO is a fact."""
    fifo = tmp_path / "n.txt"
    os.mkfifo(fifo)
    for fn in (lambda: changes_mod._read(fifo, None), lambda: changes_mod._same(fifo, tmp_path / "x")):
        t = threading.Thread(target=fn, daemon=True)
        t.start()
        t.join(2)
        assert not t.is_alive()
    _, store = _store(tmp_path)
    f = tmp_path / "doc.txt"
    f.write_text("x\n")
    store.before("t.a", "t", [f])
    f.unlink()
    os.mkfifo(f)
    done: list[object] = []
    t = threading.Thread(target=lambda: done.append(store.changes("t.a")), daemon=True)
    t.start()
    t.join(5)
    assert done and done[0][0].named and not done[0][0].content            # type: ignore[index]


def test_a_named_file_moved_into_a_new_folder_is_renamed(tmp_path: Path) -> None:
    """A1: `mkdir done && mv notes.txt done/notes.txt`: found by its identity in the folder the step made."""
    _, store = _store(tmp_path)
    w = (tmp_path / "w").resolve()
    w.mkdir()
    (w / "notes.txt").write_text("a\nb\n")
    store.before("t.a", "t", named(f"cd {w} && mkdir done && mv notes.txt done/notes.txt", w))
    (w / "done").mkdir()
    os.rename(w / "notes.txt", w / "done" / "notes.txt")
    [change] = store.changes("t.a")
    assert change.what == "renamed" and change.path == str(w / "done" / "notes.txt")
    assert change.source == str(w / "notes.txt") and not change.content         # same bytes: a fact


def test_a_rename_in_letter_case_only_is_renamed(tmp_path: Path) -> None:
    _, store = _store(tmp_path)
    w = (tmp_path / "w").resolve()
    w.mkdir()
    (w / "notes.txt").write_text("a\n")
    if not (w / "NOTES.TXT").exists():
        pytest.skip("a case-sensitive volume")
    store.before("t.a", "t", [w / "notes.txt"])
    os.rename(w / "notes.txt", w / "Notes.txt")
    [change] = store.changes("t.a")
    assert change.what == "renamed" and Path(change.path).name == "Notes.txt"


def test_a_named_file_past_the_walk_limit_is_still_found(tmp_path: Path) -> None:
    """N7 with a named file: the walk stops after WALK_MAX entries; a file a call named under it is still found and
    carries content."""
    _, store = _store(tmp_path)
    w = (tmp_path / "w").resolve()
    w.mkdir()
    result = w / "out" / "zz" / "result.txt"
    store.before("t.a", "t", [w, result])
    (w / "out" / "zz").mkdir(parents=True)
    for i in range(changes_mod.WALK_MAX + 10):
        (w / "out" / f"f{i}.txt").write_text("x")
    result.write_text("the answer\n")
    out = store.changes("t.a")
    [made] = [c for c in out if c.path == str(result)]
    assert made.content and made.named and "the answer" in store.describe(made)
    assert any(f"more than {changes_mod.WALK_MAX:,} entries" in store.describe(c) for c in out)


def test_empty_new_folders_count_toward_the_walk_limit(tmp_path: Path) -> None:
    """N7: each folder made during the step is an entry, so thousands of empty ones stop the walk too."""
    proj = tmp_path / "proj"
    proj.mkdir()
    store = Changes(tmp_path / "changes")
    store.before("t.s", "t", [proj])
    for i in range(changes_mod.WALK_MAX + 100):
        (proj / f"e{i}").mkdir()
    out = store.changes("t.s")
    assert any(f"more than {changes_mod.WALK_MAX:,} entries" in store.describe(c) for c in out)
