"""L39 fix round 4 (rereview-3): an in-place edit that keeps a backup still shows the edited file's diff, and the backup
is a fact (A1); a history file reached through a link counts as history (I1); a new file's unchanged sibling is listed
and read only through a checked fd (P1); the walk's stop fact names every listed folder it left (nit). Built from the
reviewer's probes backup_edit.py, hist_symlink.py and sibswap.py."""

import os
import subprocess
import sys
from pathlib import Path

import pytest

from ferb.act.fs import changes as changes_mod
from ferb.act.fs.changes import Changes, named
from ferb.policy.protection import Protection

TOKEN = "rt-OPAQUE-VALUE-123"
TYPED = "echo TYPED-BY-OWNER"


def _store(tmp_path: Path) -> tuple[Path, Changes]:
    home = (tmp_path / "home").resolve()
    home.mkdir()
    return home, Changes(tmp_path / "changes", home=home)


@pytest.mark.parametrize("command, backup", [
    ("sed -i.bak 's/ID$/2024/' notes.txt", "notes.txt.bak"),
    ("perl -pi.bak -e 's/ID$/2024/' notes.txt", "notes.txt.bak"),
    ("mv notes.txt notes.txt.orig && sed 's/ID$/2024/' notes.txt.orig > notes.txt", "notes.txt.orig"),
])
def test_an_edit_that_keeps_a_backup_shows_the_edited_files_diff_and_the_backup_as_a_fact(
        tmp_path: Path, command: str, backup: str) -> None:
    """A1 (backup_edit.py): the named file's old inode turns up under the backup name and a new file holds the edit at
    the named path. The named path keeps its record, modified and diffed against FERB's copy (so a pass needs a look);
    the backup name is a fact: it holds the original."""
    _, store = _store(tmp_path)
    w = (tmp_path / "w").resolve()
    w.mkdir()
    notes = w / "notes.txt"
    notes.write_text("Student ID: ID\nline two\n")
    store.before("t.a", "t", named(f"cd '{w}' && {command}", w))
    subprocess.run(command, shell=True, cwd=w, check=True)
    out = store.changes("t.a")
    [edited] = [c for c in out if c.path == str(notes)]
    assert edited.what == "modified" and edited.content and edited.named and edited.before is not None
    assert "-Student ID: ID\n+Student ID: 2024" in store.describe(edited)
    [kept] = [c for c in out if c.path == str(w / backup)]
    assert kept.what == "renamed" and kept.source == str(notes) and kept.named and not kept.content
    assert kept.original
    text = store.describe(kept)
    assert f"holds the original of {notes}" in text and "content unchanged" in text
    assert len(out) == 2


def test_an_in_place_edit_with_no_backup_still_shows_the_diff(tmp_path: Path) -> None:
    """A1: plain `perl -pi -e` (the incident's command, no backup): the one record is the diff."""
    _, store = _store(tmp_path)
    w = (tmp_path / "w").resolve()
    w.mkdir()
    notes = w / "notes.txt"
    notes.write_text("Student ID: ID\nline two\n")
    command = "perl -pi -e 's/ID$/2024/' notes.txt"
    store.before("t.a", "t", named(f"cd '{w}' && {command}", w))
    subprocess.run(command, shell=True, cwd=w, check=True)
    [edited] = store.changes("t.a")
    assert edited.path == str(notes) and edited.what == "modified" and edited.content
    assert "-Student ID: ID\n+Student ID: 2024" in store.describe(edited)


def test_a_backup_that_was_changed_too_is_diffed_as_renamed(tmp_path: Path) -> None:
    """A1: the old inode moved to the backup name and its bytes changed as well: both names carry a diff (neither is
    "the original" any more)."""
    _, store = _store(tmp_path)
    w = (tmp_path / "w").resolve()
    w.mkdir()
    notes = w / "notes.txt"
    notes.write_text("Student ID: ID\nline two\n")
    store.before("t.a", "t", [notes, w / "old.txt"])
    os.rename(notes, w / "old.txt")
    with open(w / "old.txt", "a") as f:
        f.write("added to the backup\n")
    notes.write_text("Student ID: 2024\nline two\n")
    out = {Path(c.path).name: c for c in store.changes("t.a")}
    assert out["notes.txt"].what == "modified" and out["notes.txt"].content
    assert out["old.txt"].what == "renamed" and out["old.txt"].content and not out["old.txt"].original
    assert "+added to the backup" in store.describe(out["old.txt"])


@pytest.mark.parametrize("link, target", [
    (".zsh_history", "dotfiles/zsh_history"),                       # hist_symlink.py: a dotfile manager's link
    (".zsh_sessions/ABC.history", "sync/ABC.history"),              # a session file kept in a synced folder
    (".local/share/fish/fish_history", "dotfiles/fish_history"),
])
def test_a_history_file_reached_through_a_link_is_a_fact(tmp_path: Path, link: str, target: str) -> None:
    """I1 (hist_symlink.py): `~/.zsh_history -> ~/dotfiles/zsh_history`; the step names the real file and the owner's
    shell then appends a line through the link: facts only, the typed text never shown."""
    home, store = _store(tmp_path)
    real = home / target
    real.parent.mkdir(parents=True, exist_ok=True)
    real.write_text("ls\n")
    (home / link).parent.mkdir(parents=True, exist_ok=True)
    (home / link).symlink_to(real)
    store.before("t.a", "t", [real])
    with open(home / link, "a") as f:
        f.write(TYPED + "\n")
    out = store.changes("t.a")
    assert out and not any(c.content for c in out)
    assert all("TYPED-BY-OWNER" not in store.describe(c) for c in out)


def test_naming_the_history_link_itself_shows_nothing_typed(tmp_path: Path) -> None:
    """I1: the step names the link `~/.zsh_history` (not its target): nothing it holds is shown."""
    home, store = _store(tmp_path)
    (home / "dotfiles").mkdir()
    real = home / "dotfiles" / "zsh_history"
    real.write_text("ls\n")
    (home / ".zsh_history").symlink_to(real)
    store.before("t.a", "t", [home / ".zsh_history", home / "notes.txt"])
    with open(home / ".zsh_history", "a") as f:
        f.write(TYPED + "\n")
    out = store.changes("t.a")
    assert not any(c.content for c in out)
    assert all("TYPED-BY-OWNER" not in store.describe(c) for c in out)


def _closed_home(tmp_path: Path) -> tuple[Path, Path, Changes]:
    home = (tmp_path / "home").resolve()
    ferb = home / ".ferb"
    (ferb / "codex-home").mkdir(parents=True)
    (ferb / "codex-home" / "auth.json").write_text(f'{{"refresh_token": "{TOKEN}"}}\n{{"account": "me"}}\n')
    protection = Protection(home, ferb, ["~/.ssh"], [], [], True)
    store = Changes(ferb / "changes", closed=lambda p: protection.read_denied(p) is not None, home=home)
    return home, ferb, store


def test_a_sibling_is_read_at_describe_only_through_a_checked_fd(tmp_path: Path,
                                                                 monkeypatch: pytest.MonkeyPatch) -> None:
    """P1 (sibswap.py): a new file paired with an unchanged sibling; while describe() reads the sibling, its folder is
    swapped for a link to a closed folder holding a file of the sibling's name. No closed text reaches the record; the
    new file's text is shown whole instead."""
    home, ferb, store = _closed_home(tmp_path)
    w = home / "Documents" / "w"
    w.mkdir(parents=True)
    (w / "auth.json").write_text('{"refresh_token": "decoy"}\n{"account": "me"}\n')       # existed before the step
    new = w / "auth2.json"
    store.before("t.a", "t", [new])
    new.write_text('{"refresh_token": "decoy"}\n{"account": "me"}\n{"x": 1}\n')
    [change] = store.changes("t.a")
    assert change.source == str(w / "auth.json")                     # paired with the sibling
    real_read = changes_mod._read
    swapped: list[bool] = []

    def read(path: Path, closed: object, limit: int | None = None) -> bytes | None:
        if Path(path) == w / "auth.json" and not w.is_symlink():
            swapped.append(True)
            os.rename(w, w.with_name("w.real"))
            w.symlink_to(ferb / "codex-home")                         # the sibling's folder, swapped during the read
            try:
                return real_read(path, closed, limit)                 # type: ignore[arg-type]
            finally:
                w.unlink()
                os.rename(w.with_name("w.real"), w)
        return real_read(path, closed, limit)                         # type: ignore[arg-type]
    monkeypatch.setattr(changes_mod, "_read", read)
    text = store.describe(change)
    monkeypatch.setattr(changes_mod, "_read", real_read)
    assert swapped
    assert TOKEN not in text
    assert '{"x": 1}' in text and "Its text" in text                  # the new file's whole text, no base


def test_a_new_files_siblings_are_listed_only_through_a_checked_fd(tmp_path: Path,
                                                                    monkeypatch: pytest.MonkeyPatch) -> None:
    """P1: the folder beside a new file is listed for pairing through an fd opened without following a link and
    checked, never by path."""
    home, _, store = _closed_home(tmp_path)
    w = home / "Documents" / "w"
    w.mkdir(parents=True)
    body = "".join(f"line {i} of the thesis\n" for i in range(20))
    (w / "thesis.txt").write_text(body)
    store.before("t.a", "t", [w / "thesis-final.txt"])
    (w / "thesis-final.txt").write_text(body + "one more line\n")
    real_scandir = os.scandir
    by_path: list[str] = []

    def scandir(path: object = ".") -> object:
        if sys._getframe(1).f_code.co_filename == changes_mod.__file__ and not isinstance(path, int) \
                and Path(str(path)) == w:
            by_path.append(str(path))
        return real_scandir(path)                                     # type: ignore[call-overload]
    monkeypatch.setattr(os, "scandir", scandir)
    [change] = store.changes("t.a")
    monkeypatch.setattr(os, "scandir", real_scandir)
    assert by_path == []
    assert change.source == str(w / "thesis.txt")                     # still paired
    assert "+one more line" in store.describe(change)


def test_the_stop_fact_names_every_listed_folder_the_walk_left(tmp_path: Path) -> None:
    """Nit (rereview-3, N7): the walk stops in the first listed folder; a later listed folder whose new folder wasn't
    walked gets the "stopped" fact too."""
    a, b = (tmp_path / "a").resolve(), (tmp_path / "b").resolve()
    a.mkdir()
    b.mkdir()
    store = Changes(tmp_path / "changes")
    store.before("t.s", "t", [a, b])
    (a / "out").mkdir()
    for i in range(changes_mod.WALK_MAX + 10):
        (a / "out" / f"f{i}.txt").write_text("x")
    (b / "build").mkdir()
    (b / "build" / "x.txt").write_text("x")
    stops = [c.path for c in store.changes("t.s") if f"more than {changes_mod.WALK_MAX:,} entries" in c.note]
    assert sorted(stops) == [str(a), str(b)]
