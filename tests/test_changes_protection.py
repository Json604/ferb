"""L39 fix round 1 (review-1): the change record never reads through a link or into anything FERB's shell sandbox denies
to reads (C1); content only for files a call named exactly, facts for anything else, never a shell history file's text
(I1); files made under new folders are recorded (I2); a new file is paired without reading a whole folder (I5); records
of resuming trees survive the start sweep (M1); locked clones go (M2); one lock per task, the index written on change
(M5)."""

import os
import stat
from pathlib import Path

import pytest

from ferb.act.fs import changes as changes_mod
from ferb.act.fs.changes import Changes, named
from ferb.policy.protection import Protection
from tests.support.documents import docx

TOKEN = "rt-OPAQUE-VALUE-123"


def _protected(tmp_path: Path) -> tuple[Path, Path, Protection, Changes]:
    """A home with FERB's state at ~/.ferb and the never-read store ~/.ssh, the change record closed as the shell's
    sandbox is (Protection.read_denied: the shell's never-read stores)."""
    home = (tmp_path / "home").resolve()
    ferb = home / ".ferb"
    (ferb / "codex-home").mkdir(parents=True)
    (ferb / "codex-home" / "auth.json").write_text('{"refresh_token": "%s"}' % TOKEN)
    (home / ".ssh").mkdir()
    (home / ".ssh" / "id_ed25519").write_text("PRIVATE KEY " + TOKEN)
    protection = Protection(home, ferb, ["~/.ssh"], [], [])
    store = Changes(ferb / "changes", closed=lambda p: protection.read_denied(p) is not None, home=home)
    return home, ferb, protection, store


@pytest.mark.parametrize("target", [".ferb/codex-home/auth.json", ".ssh/id_ed25519"])
def test_a_named_file_swapped_for_a_link_into_a_closed_path_shows_no_content(tmp_path: Path, target: str) -> None:
    """C1, the review's repro: the sandbox refuses `cat` of the closed file but not `ln -s` to it; the unsandboxed
    daemon must never read through the link."""
    home, _, _, store = _protected(tmp_path)
    work = home / "Documents"
    work.mkdir()
    notes = work / "notes.txt"
    notes.write_text("my notes\n")
    store.before("t.step.a", "t", [notes])
    notes.unlink()
    notes.symlink_to(home / target)
    [change] = store.changes("t.step.a")
    text = store.describe(change)
    assert not change.content and change.named and TOKEN not in text and "now a link to" in text


def test_a_new_link_in_a_listed_folder_or_beside_a_new_file_is_never_read(tmp_path: Path) -> None:
    home, _, _, store = _protected(tmp_path)
    work = home / "Documents"
    work.mkdir()
    made = work / "report.txt"
    store.before("t.step.a", "t", [work, made])
    (work / "report-old.txt").symlink_to(home / ".ssh" / "id_ed25519")      # a like-named sibling: a link
    (work / "leak.txt").symlink_to(home / ".ferb" / "codex-home" / "auth.json")
    made.write_text("PRIVATE KEY " + TOKEN[:4] + " report\n")
    out = store.changes("t.step.a")
    assert all(TOKEN not in store.describe(c) for c in out)
    assert [Path(c.path).name for c in out] == ["report.txt"]               # links are no plain files: not listed
    assert out[0].source is None


def test_a_command_naming_a_closed_store_records_nothing_under_it(tmp_path: Path) -> None:
    """C1 ruling: `cat ~/.ssh/id_ed25519 > x` records nothing under ~/.ssh (at most: protected)."""
    home, ferb, _, store = _protected(tmp_path)
    work = home / "work"
    work.mkdir()
    counts = store.before("t.step.a", "t", named("cat ~/.ssh/id_ed25519 > x; ls ~/.ferb", work, home))
    assert counts["protected"] == 2 and counts["copied"] == 0
    rec = store._records["t.step.a"]
    assert not [p for p in [*rec.files, *rec.folders, *rec.named] if ".ssh" in p or "/.ferb/" in p + "/"]
    (home / ".ssh" / "id_ed25519").write_text("changed " + TOKEN)
    assert all(".ssh" not in c.path for c in store.changes("t.step.a"))


def test_the_owners_history_during_a_cd_home_step_is_a_fact_without_content(tmp_path: Path) -> None:
    """I1: a step runs `cd ~ && ls` while the owner types in Terminal; ~/.zsh_history grows."""
    home, _, _, store = _protected(tmp_path)
    history = home / ".zsh_history"
    history.write_text("ls\n")
    store.before("t.step.a", "t", named("cd ~ && ls", home / "work", home))
    history.write_text("ls\nmysql -p hunter2-secret\n")
    (home / "downloaded.pdf").write_bytes(b"%PDF-1.4 owner's own download")
    out = {Path(c.path).name: c for c in store.changes("t.step.a")}
    assert set(out) == {".zsh_history", "downloaded.pdf"} and not any(c.content for c in out.values())
    lines = [store.describe(c) for c in out.values()]
    assert all("during this step" in line and "made by this step" not in line for line in lines)
    assert not any("hunter2" in line or "owner's own download" in line for line in lines)


def test_a_history_file_named_exactly_is_still_a_fact(tmp_path: Path) -> None:
    home, _, _, store = _protected(tmp_path)
    history = home / ".zsh_history"
    history.write_text("ls\n")
    store.before("t.step.a", "t", named("tail ~/.zsh_history", home, home))
    history.write_text("ls\nexport TOKEN=hunter2-secret\n")
    [change] = store.changes("t.step.a")
    assert not change.content and "hunter2" not in store.describe(change)


def test_a_declared_file_under_a_new_folder_is_recorded(tmp_path: Path) -> None:
    """I2: fs.write ~/Desktop/report/summary.txt with report/ new."""
    home, _, _, store = _protected(tmp_path)
    (home / "Desktop").mkdir()
    summary = home / "Desktop" / "report" / "summary.txt"
    store.before("t.step.a", "t", [summary])
    summary.parent.mkdir()
    summary.write_text("the summary\n")
    [change] = store.changes("t.step.a")
    assert change.path == str(summary) and change.content and "the summary" in store.describe(change)


def test_files_copied_into_a_folder_the_command_makes_are_reported(tmp_path: Path) -> None:
    """I2: `mkdir -p out && cp a out/` reports out/a (a fact: the command named the folder, not the file)."""
    home, _, _, store = _protected(tmp_path)
    work = home / "work"
    work.mkdir()
    (work / "a").write_text("alpha\n")
    store.before("t.step.a", "t", named("mkdir -p out && cp a out/", work, home))
    (work / "out").mkdir()
    (work / "out" / "a").write_text("alpha\n")
    out = store.changes("t.step.a")
    assert [(Path(c.path).relative_to(work).as_posix(), c.content) for c in out] == [("out/a", False)]


def test_pairing_a_new_file_reads_only_a_few_like_named_files(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    """I5: a folder with 200 same-type siblings: at most PAIR_READS of them are read, chosen by name and size."""
    home, _, _, store = _protected(tmp_path)
    folder = home / "Downloads"
    folder.mkdir()
    for i in range(200):
        docx(folder / f"invoice-{i:03d}.docx", [f"Invoice {i}", "Amount due"])
    original = docx(folder / "HCI_Assignment1.docx", ["Human Computer Interaction", "Student ID", "ID"])
    made = folder / "Sam_HCI_Assignment1.docx"
    store.before("t.step.a", "t", [made])
    docx(made, ["Human Computer Interaction", "Student ID", "2031xy04417"])
    reads: list[Path] = []
    real_read = changes_mod._read

    def counted(path: Path, closed, limit=None):
        reads.append(Path(path))
        return real_read(path, closed, limit)
    monkeypatch.setattr(changes_mod, "_read", counted)
    [change] = store.changes("t.step.a")
    siblings = [p for p in reads if p.parent == folder and p != made]
    assert len(set(siblings)) <= changes_mod.PAIR_READS
    assert change.source == str(original)


def test_a_resuming_trees_record_survives_the_start_sweep(tmp_path: Path) -> None:
    """M1: the sweep keeps the records of trees that will resume, so their checks still see earlier changes."""
    root = tmp_path / "changes"
    f = tmp_path / "a.txt"
    f.write_text("one\n")
    Changes(root).before("keep.step.a", "keep", [f])
    Changes(root).before("gone.step.a", "gone", [f])
    f.write_text("two\n")
    after = Changes(root)
    assert after.sweep(keep=lambda tree: tree == "keep") == []
    [change] = after.changes("keep.step.a", "keep")
    assert "-one\n+two" in after.describe(change)
    assert after.changes("gone.step.a", "gone") == []


def test_a_locked_files_clone_is_removable(tmp_path: Path) -> None:
    """M2: clonefile carries a locked flag; the record clears it, so forget leaves nothing behind."""
    f = tmp_path / "locked.txt"
    f.write_text("locked\n")
    os.chflags(f, stat.UF_IMMUTABLE)
    try:
        store = Changes(tmp_path / "changes")
        assert store.before("t.step.a", "t", [f])["copied"] == 1
        assert store.forget("t") == []
        assert not any((tmp_path / "changes").iterdir())
    finally:
        os.chflags(f, 0)


def test_one_lock_per_task_and_the_index_is_written_only_on_change(tmp_path: Path) -> None:
    """M5: one task's large folder never holds another task's calls; an unchanged record isn't rewritten."""
    f = tmp_path / "a.txt"
    f.write_text("one\n")
    store = Changes(tmp_path / "changes")
    assert store._lock("a.step.x") is not store._lock("b.step.y")
    store.before("a.step.x", "a", [f])
    index = next((tmp_path / "changes").rglob("index.json"))
    first = index.stat().st_mtime_ns
    os.utime(index, ns=(first - 10_000_000_000, first - 10_000_000_000))
    store.before("a.step.x", "a", [f])                                    # nothing new to record
    assert index.stat().st_mtime_ns == first - 10_000_000_000


def test_the_daemons_change_record_is_closed_as_the_shell_sandbox_is(daemon) -> None:
    """C1 wiring: the daemon's record closes what the shell's sandbox denies to reads (Protection.read_denied), even
    where FERB's own file tools may read with the owner's Yes."""
    protection = daemon.daemon.protection
    store = daemon.daemon.changes
    entry = protection.reads[0].path / "inside"
    assert store.closed(entry) is True and store.closed(daemon.daemon.home / "config.toml") is True
    assert store.closed(daemon.daemon.home / "workspace" / "notes.txt") is False


def test_the_daemons_launches_are_stamped_with_ferbs_clock(daemon) -> None:
    """M7 wiring: the launch time comes from FERB's clock (the desktop records' clock), not the wall clock."""
    launched = daemon.daemon.mac.launched                                 # type: ignore[union-attr]
    assert launched.now() == daemon.daemon.clock.now().isoformat()
