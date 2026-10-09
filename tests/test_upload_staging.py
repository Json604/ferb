"""A browser upload reads FERB's own copy, never the owner's original in a folder macOS protects (rule 17; the F4 live
run's "Brave Browser would like to access files in your Desktop folder", task 79bc5bfa). The copy keeps the original's
name and origin label (MP2); the original is only read; copies go when the task tree ends."""
import asyncio
import hashlib
import os
import shutil
import threading
from pathlib import Path
from types import SimpleNamespace
from typing import Any

import pytest

from ferb.act.browser.downloads import untrusted_roots
from ferb.act.browser.ext_bridge import _TabDriver
from ferb.act.browser.uploads import StagingFailed, Uploads
from ferb.act.fs.files import _source
from ferb.core import origin
from ferb.core.errors import Refused
from tests.conftest import RunningDaemon


def _sha(p: Path) -> str:
    return hashlib.sha256(p.read_bytes()).hexdigest()


def _driver(root: Path, untrusted: tuple[str, ...] = ()) -> _TabDriver:
    driver = _TabDriver(SimpleNamespace(), lambda: [])          # type: ignore[arg-type]
    driver.uploads = Uploads(root, lambda: untrusted)
    driver.task_root = lambda task: task.split(".", 1)[0]
    return driver


class _Page:
    """The extension's side: records the paths it was given and reports the bytes the page selected."""

    def __init__(self) -> None:
        self.given: list[list[str]] = []

    async def call(self, operation: str, args: dict[str, Any], **kwargs: Any) -> dict[str, Any]:
        self.given.append(list(args["paths"]))
        data = Path(args["paths"][0]).read_bytes()
        return {"ok": True, "detail": "1 file(s) set", "selected_files": [
            {"name": Path(args["paths"][0]).name, "size": len(data), "sha256": hashlib.sha256(data).hexdigest()}]}


def test_the_browser_is_given_the_staged_copy_and_the_original_is_untouched(tmp_path: Path) -> None:
    original = tmp_path / "Desktop" / "clg:uni" / "submission.csv"
    original.parent.mkdir(parents=True)
    original.write_bytes(b"id,label\n1,a\n")
    before = original.stat()
    page = _Page()
    root = tmp_path / "ws" / "uploads"
    tab = SimpleNamespace(bridge=page, chrome_id=7, owner="tree1.worker")
    receipt = asyncio.run(_driver(root)._upload(tab, 0, 1, 0, (str(original),), "i1"))
    [given] = page.given[0]
    staged = Path(given)
    assert receipt.ok and receipt.facts["bytes_match"] is True
    assert staged.name == "submission.csv" and staged.parent.parent == root / "tree1"
    assert staged.read_bytes() == original.read_bytes()
    assert receipt.facts["files"][0]["path"] == os.path.realpath(original)        # the identity is the original's
    assert receipt.facts["staged_copies"] == [given]
    after = original.stat()
    assert (after.st_ino, after.st_size, after.st_mtime_ns) == (before.st_ino, before.st_size, before.st_mtime_ns)


def test_a_staged_copy_keeps_the_originals_origin_label(tmp_path: Path) -> None:
    ws = (tmp_path / "ws").resolve()
    (ws / "downloads").mkdir(parents=True)
    outside = ws / "downloads" / "dl-0123456789ab-report.pdf"
    outside.write_bytes(b"%PDF from a website")
    mine = tmp_path / "Desktop" / "notes.txt"
    mine.parent.mkdir()
    mine.write_bytes(b"my notes")
    roots = untrusted_roots([], ws)
    ups = Uploads(ws / "uploads", lambda: roots)
    ctx = SimpleNamespace(untrusted_roots=roots)
    a = Path(ups.stage("tree1", str(outside), _sha(outside))).resolve()
    b = Path(ups.stage("tree1", str(mine.resolve()), _sha(mine))).resolve()
    assert origin.from_read_source(*_source(a, ctx)) == "external:download"     # outside content stays outside
    assert origin.from_read_source(*_source(b, ctx)) == "private"               # the owner's file stays the owner's
    assert a.parent.parent == ws / "uploads" / "outside" / "tree1" and b.parent.parent == ws / "uploads" / "tree1"


def test_no_copy_means_nothing_is_given_to_the_browser(tmp_path: Path) -> None:
    original = tmp_path / "Desktop" / "cv.pdf"
    original.parent.mkdir()
    original.write_bytes(b"%PDF cv")
    root = tmp_path / "ws" / "uploads"
    root.mkdir(parents=True)
    elsewhere = tmp_path / "elsewhere"
    elsewhere.mkdir()
    (root / "tree1").symlink_to(elsewhere)                      # planted by a shell command in the workspace
    page = _Page()
    tab = SimpleNamespace(bridge=page, chrome_id=7, owner="tree1.worker")
    with pytest.raises(Refused, match="couldn't make its own copy"):
        asyncio.run(_driver(root)._upload(tab, 0, 1, 0, (str(original),), "i1"))
    assert page.given == [] and list(elsewhere.iterdir()) == []


def test_a_copy_that_doesnt_match_the_identity_is_removed_and_refused(tmp_path: Path) -> None:
    original = tmp_path / "a.txt"
    original.write_bytes(b"now")
    ups = Uploads(tmp_path / "uploads", lambda: ())
    with pytest.raises(StagingFailed, match="changed while FERB copied it"):
        ups.stage("tree1", str(original), hashlib.sha256(b"before").hexdigest())
    assert not any((tmp_path / "uploads" / "tree1").iterdir())


def test_copies_go_when_their_tree_ends_and_at_start_up(tmp_path: Path) -> None:
    src = tmp_path / "a.txt"
    src.write_bytes(b"x")
    ups = Uploads(tmp_path / "uploads", lambda: ())
    mine = Path(ups.stage("tree1", str(src), _sha(src)))
    other = Path(ups.stage("tree2", str(src), _sha(src)))
    assert ups.forget_tree("tree1") == 1 and not mine.exists() and other.exists() and src.read_bytes() == b"x"
    assert ups.sweep() == 1 and not other.exists() and src.exists()
    assert shutil.rmtree.avoids_symlink_attacks                 # forget_tree / sweep never follow a planted link


def test_the_daemon_wires_the_copies_and_clears_them_when_a_tree_ends(daemon: RunningDaemon) -> None:
    d = daemon.daemon
    assert d.router.uploads is d.uploads and d.ferb_identity.uploads is d.uploads
    src = d.home / "notes.txt"
    src.write_text("mine")

    async def stage() -> str:
        return d.uploads.stage("tree9", str(src.resolve()), _sha(src))
    staged = Path(daemon.run(stage()))
    assert staged.exists() and staged.is_relative_to(d.home / "workspace" / "uploads" / "tree9")
    daemon.run(d.supervisor.on_task_finish("tree9"))
    assert not staged.exists() and src.read_text() == "mine"


def test_the_daemon_journals_a_cleanup_it_could_not_do_and_follows_no_link(daemon: RunningDaemon,
                                                                           tmp_path: Path) -> None:
    d = daemon.daemon
    victim = _victim(tmp_path)
    root = d.home / "workspace" / "uploads"
    if root.exists():
        shutil.rmtree(root)
    root.symlink_to(victim)                                     # planted by a shell command in the workspace
    daemon.run(d.supervisor.on_task_finish("tree1"))
    assert _untouched(victim)

    async def journal() -> list[dict[str, Any]]:
        return [e.data for e in d._journal().entries() if e.kind == "browser.uploads_cleanup_failed"]
    [entry] = daemon.run(journal())
    assert entry["task_id"] == "tree1" and "not FERB's folder" in entry["error"]


# ---------- every delete stays inside the uploads folder and follows no link ----------

def _victim(tmp_path: Path) -> Path:
    """A folder of the owner's that a planted link points at: names a cleanup might reach, each holding a file."""
    victim = tmp_path / "Documents"
    for name in ("tree1", "tree2", "outside"):
        (victim / name).mkdir(parents=True)
        (victim / name / "keep.txt").write_text("the owner's")
    return victim


def _untouched(victim: Path) -> bool:
    return all((victim / n / "keep.txt").read_text() == "the owner's" for n in ("tree1", "tree2", "outside"))


def test_a_link_planted_for_a_trees_folder_is_removed_never_followed(tmp_path: Path) -> None:
    victim = _victim(tmp_path)
    root = tmp_path / "uploads"
    (root / "outside").mkdir(parents=True)
    (root / "tree1").symlink_to(victim / "tree1")
    (root / "outside" / "tree1").symlink_to(victim / "tree1")
    ups = Uploads(root, lambda: ())
    assert ups.forget_tree("tree1") == 2                        # both links go: links, not what they point at
    assert not (root / "tree1").is_symlink() and not (root / "outside" / "tree1").is_symlink() and _untouched(victim)
    (root / "tree2").symlink_to(victim / "tree2")
    (root / "outside" / "tree2").symlink_to(victim / "tree2")
    (root / "stray").symlink_to(victim / "tree1" / "keep.txt")
    assert ups.sweep() == 3 and sorted(os.listdir(root)) == ["outside"] and os.listdir(root / "outside") == []
    assert _untouched(victim)


def test_a_link_planted_for_the_outside_folder_is_never_followed(tmp_path: Path) -> None:
    victim = _victim(tmp_path)
    root = tmp_path / "uploads"
    root.mkdir()
    (root / "outside").symlink_to(victim)                       # root/outside/tree1 would be ~/Documents/tree1
    ups = Uploads(root, lambda: ())
    with pytest.raises(OSError, match="not FERB's folder"):
        ups.forget_tree("tree1")
    assert _untouched(victim)
    assert ups.sweep() == 1                                     # at start-up the link itself goes, nothing under it
    assert not (root / "outside").exists() and _untouched(victim)


def test_a_link_planted_for_the_uploads_folder_is_never_followed(tmp_path: Path) -> None:
    victim = _victim(tmp_path)
    root = tmp_path / "ws" / "uploads"
    root.parent.mkdir()
    root.symlink_to(victim)                                     # every copy path would land in ~/Documents
    ups = Uploads(root, lambda: ())
    for clean in (lambda: ups.forget_tree("tree1"), ups.sweep):
        with pytest.raises(OSError, match="not FERB's folder"):
            clean()
    assert root.is_symlink() and _untouched(victim)


@pytest.mark.parametrize("planted", ["uploads", "outside", "tree1"])
def test_staging_never_writes_through_a_planted_link(tmp_path: Path, planted: str) -> None:
    downloads = tmp_path / "downloads"
    downloads.mkdir()
    src = downloads / "report.pdf"                              # outside content: its copy goes under outside/
    src.write_bytes(b"%PDF")
    root = tmp_path / "uploads"
    elsewhere = tmp_path / "elsewhere"
    elsewhere.mkdir()
    link = {"uploads": root, "outside": root / "outside", "tree1": root / "outside" / "tree1"}[planted]
    link.parent.mkdir(parents=True, exist_ok=True)
    link.symlink_to(elsewhere)
    ups = Uploads(root, lambda: (str(downloads),))
    with pytest.raises(StagingFailed, match="not FERB's folder"):
        ups.stage("tree1", str(src), _sha(src))
    assert list(elsewhere.iterdir()) == [] and src.read_bytes() == b"%PDF"


def test_copying_runs_off_the_event_loop(tmp_path: Path) -> None:
    original = tmp_path / "cv.pdf"
    original.write_bytes(b"%PDF cv")
    driver = _driver(tmp_path / "uploads")
    assert driver.uploads is not None
    staging, threads = driver.uploads.stage, []

    def stage(tree: str, source: str, sha256: str) -> str:
        threads.append(threading.current_thread())
        return staging(tree, source, sha256)
    driver.uploads.stage = stage                                # type: ignore[method-assign]
    tab = SimpleNamespace(bridge=_Page(), chrome_id=7, owner="tree1.worker")
    assert asyncio.run(driver._upload(tab, 0, 1, 0, (str(original),), "i1")).ok
    assert threads and threads[0] is not threading.main_thread()


def test_a_tab_no_task_owns_gets_no_copy(tmp_path: Path) -> None:
    src = tmp_path / "a.txt"
    src.write_bytes(b"x")
    ups = Uploads(tmp_path / "uploads", lambda: ())
    for tree in ("", ".", "..", "a/b", ".hidden"):
        with pytest.raises(StagingFailed, match="no task owns this tab"):
            ups.stage(tree, str(src), _sha(src))
        assert ups.forget_tree(tree) == 0
    assert not (tmp_path / "uploads").exists()
