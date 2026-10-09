"""A browser upload reads FERB's own copy, never the owner's original in a folder macOS protects (rule 17; the F4 live
run's "Brave Browser would like to access files in your Desktop folder", task 79bc5bfa). The copy keeps the original's
name and origin label (MP2); the original is only read; copies go when the task tree ends."""
import asyncio
import ctypes
import errno
import hashlib
import os
import shutil
import stat
import threading
from pathlib import Path
from types import SimpleNamespace
from typing import Any

import pytest

import ferb.act.browser.ext_bridge as ext_bridge
from ferb.act.browser.downloads import untrusted_roots
from ferb.act.browser.ext_bridge import _TabDriver
from ferb.act.browser.uploads import CleanupLeft, StagingFailed, Uploads
from ferb.act.fs import recovery
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
    name = "chrome/Profile 1"

    def __init__(self) -> None:
        self.given: list[list[str]] = []

    async def call(self, operation: str, args: dict[str, Any], **kwargs: Any) -> dict[str, Any]:
        self.given.append(list(args["paths"]))
        selected = [(Path(p).name, Path(p).read_bytes()) for p in args["paths"]]
        return {"ok": True, "detail": f"{len(selected)} file(s) set", "selected_files": [
            {"name": n, "size": len(data), "sha256": hashlib.sha256(data).hexdigest()} for n, data in selected]}


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
    assert [str(root) in x for x in entry["left"]] == [True]


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


def test_reading_the_identity_and_copying_run_off_the_event_loop_in_one_pass(tmp_path: Path,
                                                                            monkeypatch: pytest.MonkeyPatch) -> None:
    original = tmp_path / "cv.pdf"
    original.write_bytes(b"%PDF cv")
    driver = _driver(tmp_path / "uploads")
    assert driver.uploads is not None
    threads: list[tuple[str, threading.Thread]] = []
    hash_fd, stage_all = ext_bridge.sha256_fd, driver.uploads.stage_all

    def hashing(fd: int) -> str:
        threads.append(("identity", threading.current_thread()))
        return hash_fd(fd)

    def staging(*args: Any) -> list[str]:
        threads.append(("copy", threading.current_thread()))
        return stage_all(*args)
    monkeypatch.setattr(ext_bridge, "sha256_fd", hashing)
    monkeypatch.setattr(driver.uploads, "stage_all", staging)
    tab = SimpleNamespace(bridge=_Page(), chrome_id=7, owner="tree1.worker")
    assert asyncio.run(driver._upload(tab, 0, 1, 0, (str(original),), "i1")).ok
    [(_, a), (_, b)] = threads
    assert [k for k, _ in threads] == ["identity", "copy"] and a is b and a is not threading.main_thread()


def test_a_tab_no_task_owns_gets_no_copy(tmp_path: Path) -> None:
    src = tmp_path / "a.txt"
    src.write_bytes(b"x")
    ups = Uploads(tmp_path / "uploads", lambda: ())
    for tree in ("", ".", "..", "a/b", ".hidden", "outside"):
        with pytest.raises(StagingFailed, match="no task owns this tab"):
            ups.stage(tree, str(src), _sha(src))
        assert ups.forget_tree(tree) == 0
    assert not (tmp_path / "uploads").exists()


# ---------- fix round 1: the copy is made through handles; tabs that outlive the tree keep their copies ----------

def _owner_file(path: Path) -> tuple[bytes, int, int, int]:
    st = path.stat()
    return path.read_bytes(), stat.S_IMODE(st.st_mode), st.st_mtime_ns, st.st_flags


@pytest.mark.parametrize("way", ["clone", "byte copy"])
def test_a_folder_swapped_for_a_link_mid_copy_writes_deletes_and_restamps_nothing_outside(
        tmp_path: Path, monkeypatch: pytest.MonkeyPatch, way: str) -> None:
    """The reviewer's race: a shell command learns the random folder, plants a same-named one outside, and swaps
    uploads/<tree> for a link to it while FERB copies. The copy goes into the folder FERB holds, never through the link."""
    original = tmp_path / "Desktop" / "cv.pdf"
    original.parent.mkdir()
    original.write_bytes(b"%PDF the owner's cv")
    victim = tmp_path / "Documents"
    victim.mkdir()
    root = tmp_path / "uploads"
    real, seen = recovery._fclone, []

    def racing(src_fd: int, dir_fd: int, name: str) -> None:
        [leaf] = os.listdir(root / "tree1")
        seen.append(leaf)
        (victim / leaf).mkdir()
        if way == "byte copy":                                  # a same-named owner file the old copy deleted/re-stamped
            planted = victim / leaf / name
            planted.write_bytes(b"the owner's other file")
            os.chmod(planted, 0o640)
            os.utime(planted, ns=(1_600_000_000_000_000_000, 1_600_000_000_000_000_000))
            os.chflags(planted, stat.UF_NODUMP)
            seen.append(_owner_file(planted))
        os.rename(root / "tree1", root / "moved")
        (root / "tree1").symlink_to(victim)
        if way == "clone":
            real(src_fd, dir_fd, name)
        else:
            raise OSError(errno.ENOTSUP, "this volume can't clone")
    monkeypatch.setattr(recovery, "_fclone", racing)
    with pytest.raises(StagingFailed, match="no longer names FERB's copy"):
        Uploads(root, lambda: ()).stage("tree1", str(original), _sha(original))
    leaf = seen[0]
    if way == "clone":
        assert os.listdir(victim / leaf) == []                  # nothing written outside
    else:
        assert os.listdir(victim / leaf) == ["cv.pdf"] and _owner_file(victim / leaf / "cv.pdf") == seen[1]
    assert os.listdir(victim) == [leaf] and os.listdir(root / "moved") == []   # FERB's own copy went, through its folder
    assert original.read_bytes() == b"%PDF the owner's cv"


def test_a_copy_shown_in_a_tab_that_outlives_its_tree_stays_until_that_tab_closes(tmp_path: Path) -> None:
    src = tmp_path / "cv.pdf"
    src.write_bytes(b"%PDF")
    root = tmp_path / "uploads"
    ups = Uploads(root, lambda: ())
    kept = Path(ups.stage("tree1", str(src), _sha(src), tab="chrome/Profile 1#200"))     # the owner's borrowed tab
    gone = Path(ups.stage("tree1", str(src), _sha(src), tab="chrome/Profile 1#7"))       # a tab FERB opened
    assert ups.closed("chrome/Profile 1#7") is False            # FERB closed it during the tree: nothing waits yet
    assert ups.forget_tree("tree1") == 1 and not gone.exists() and kept.read_bytes() == b"%PDF"
    assert ups.collect() == 0 and kept.exists()                 # Chromium reads it again when the owner submits
    assert ups.closed("chrome/Profile 1#200") is True           # the browser said the tab closed
    assert ups.collect() == 1 and not kept.exists() and not (root / "tree1").exists()
    assert ups.closed("chrome/Profile 1#9") is False            # nothing of an ended tree waits any more


def test_several_files_are_staged_and_a_failure_removes_the_copies_already_made(tmp_path: Path) -> None:
    a, b = tmp_path / "a.txt", tmp_path / "b.txt"
    a.write_bytes(b"first")
    b.write_bytes(b"second")
    page = _Page()
    root = tmp_path / "ws" / "uploads"
    tab = SimpleNamespace(bridge=page, chrome_id=7, owner="tree1.worker")
    receipt = asyncio.run(_driver(root)._upload(tab, 0, 1, 0, (str(a), str(b)), "i1"))
    assert receipt.ok and receipt.facts["bytes_match"] is True
    first, second = (Path(p) for p in page.given[0])
    assert (first.name, second.name) == ("a.txt", "b.txt") and first.parent != second.parent
    assert receipt.facts["staged_copies"] == page.given[0]
    ups = Uploads(tmp_path / "u2", lambda: ())
    fa, fb = os.open(a, os.O_RDONLY), os.open(b, os.O_RDONLY)
    try:
        with pytest.raises(StagingFailed, match="changed while FERB copied it"):
            ups.stage_all("tree1", [(fa, str(a), _sha(a)), (fb, str(b), hashlib.sha256(b"before").hexdigest())])
    finally:
        os.close(fa)
        os.close(fb)
    assert os.listdir(tmp_path / "u2" / "tree1") == []          # the first copy went at once, not at the tree's end


def test_an_unreadable_original_is_refused_with_the_reason(tmp_path: Path) -> None:
    original = tmp_path / "locked.pdf"
    original.write_bytes(b"%PDF")
    os.chmod(original, 0)
    page = _Page()
    tab = SimpleNamespace(bridge=page, chrome_id=7, owner="tree1.worker")
    try:
        with pytest.raises(Refused, match="can't read the file to upload .*Permission denied"):
            asyncio.run(_driver(tmp_path / "uploads")._upload(tab, 0, 1, 0, (str(original),), "i1"))
    finally:
        os.chmod(original, 0o600)
    assert page.given == []


def test_a_cleanup_removes_what_it_can_and_names_what_it_couldnt(tmp_path: Path) -> None:
    src = tmp_path / "a.txt"
    src.write_bytes(b"x")
    ups = Uploads(tmp_path / "uploads", lambda: ())
    stuck = Path(ups.stage("tree1", str(src), _sha(src)))
    loose = Path(ups.stage("tree1", str(src), _sha(src)))
    os.chflags(stuck, stat.UF_IMMUTABLE)                        # a lock a shell command set on FERB's copy
    try:
        with pytest.raises(CleanupLeft) as left:
            ups.forget_tree("tree1")
        assert not loose.exists() and stuck.exists() and any(str(stuck) in x for x in left.value.left)
        other = Path(ups.stage("tree2", str(src), _sha(src)))
        with pytest.raises(CleanupLeft) as left:
            ups.sweep()
        assert not other.exists() and stuck.exists() and any(str(stuck) in x for x in left.value.left)
    finally:
        os.chflags(stuck, 0)


# ---------- fix round 2: no link at the copy's own name; concurrent cleanups report only real failures ----------

def test_a_link_planted_at_the_copys_name_before_the_clone_creates_nothing_outside(
        tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    """A dangling link at <leaf>/<name>: fclonefileat without CLONE_NOFOLLOW created the clone at its target."""
    original = tmp_path / "cv.pdf"
    original.write_bytes(b"%PDF the owner's cv")
    victim = tmp_path / "Documents"
    victim.mkdir()
    root = tmp_path / "uploads"
    real = recovery._fclone

    def planting(src_fd: int, dir_fd: int, name: str) -> None:
        [leaf] = os.listdir(root / "tree1")
        (root / "tree1" / leaf / name).symlink_to(victim / "planted.pdf")      # dangling: names a file not yet there
        real(src_fd, dir_fd, name)
    monkeypatch.setattr(recovery, "_fclone", planting)
    with pytest.raises(StagingFailed, match="already taken"):
        Uploads(root, lambda: ()).stage("tree1", str(original), _sha(original))
    assert os.listdir(victim) == [] and os.listdir(root / "tree1") == []
    assert original.read_bytes() == b"%PDF the owner's cv"


def test_a_link_swapped_in_for_the_copy_after_it_was_made_is_not_given_to_the_browser(
        tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    original = tmp_path / "cv.pdf"
    original.write_bytes(b"%PDF the owner's cv")
    elsewhere = tmp_path / "elsewhere.pdf"
    elsewhere.write_bytes(b"%PDF the owner's cv")
    root = tmp_path / "uploads"
    real = recovery.copy_into

    def swapping(src_fd: int, dir_fd: int, name: str) -> str:
        digest = real(src_fd, dir_fd, name)
        os.unlink(name, dir_fd=dir_fd)
        os.symlink(elsewhere, name, dir_fd=dir_fd)                  # same name, now a link to a file outside
        return digest
    monkeypatch.setattr(recovery, "copy_into", swapping)
    with pytest.raises(StagingFailed, match="no longer names FERB's copy"):
        Uploads(root, lambda: ()).stage("tree1", str(original), _sha(original))
    assert os.listdir(root / "tree1") == [] and elsewhere.read_bytes() == b"%PDF the owner's cv"


def test_closing_a_tab_asks_for_a_collect_only_when_it_showed_a_copy_of_an_ended_tree(tmp_path: Path) -> None:
    src = tmp_path / "a.txt"
    src.write_bytes(b"x")
    ups = Uploads(tmp_path / "uploads", lambda: ())
    ups.stage("tree1", str(src), _sha(src), tab="p#1")
    ups.stage("tree2", str(src), _sha(src), tab="p#2")
    ups.forget_tree("tree1")                                    # tab 1 still shows a copy of an ended tree
    assert ups.closed("p#9") is False                           # a tab that showed nothing
    assert ups.closed("p#2") is False                           # its tree is still running: its end removes the copy
    assert ups.closed("p#1") is True and ups.collect() == 1


def test_concurrent_cleanups_report_no_failure_that_didnt_happen(tmp_path: Path) -> None:
    """Closing a window of several tabs runs several collects at once; one removing what another is removing is no
    failure."""
    src = tmp_path / "a.txt"
    src.write_bytes(b"x")
    root = tmp_path / "uploads"
    for round_ in range(5):
        ups = Uploads(root, lambda: ())
        tabs = [f"p#{i}" for i in range(6)]
        for tab in tabs:
            for _ in range(4):
                ups.stage("tree1", str(src), _sha(src), tab=tab)
        ups.forget_tree("tree1")
        for tab in tabs:
            ups.closed(tab)
        failures: list[BaseException] = []
        start = threading.Barrier(6)

        def collect() -> None:
            start.wait()
            try:
                ups.collect()
            except BaseException as e:                          # noqa: BLE001 - the test reports any failure
                failures.append(e)
        workers = [threading.Thread(target=collect) for _ in range(6)]
        for w in workers:
            w.start()
        for w in workers:
            w.join()
        assert failures == [] and not (root / "tree1").exists(), (round_, failures)


def test_a_missing_staging_function_only_disables_its_own_path(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    assert recovery._optional("ferb_no_such_libc_function") is None
    assert recovery._libc is not None and recovery._libc.clonefile and recovery._libc.renamex_np   # S1 unaffected
    monkeypatch.setattr(recovery, "_fclonefileat", None)        # no fclonefileat: a byte copy, never a failure
    src = tmp_path / "cv.pdf"
    src.write_bytes(b"%PDF")
    copy = Path(Uploads(tmp_path / "uploads", lambda: ()).stage("tree1", str(src), _sha(src)))
    assert copy.read_bytes() == b"%PDF" and copy.stat().st_mtime_ns == src.stat().st_mtime_ns


def test_a_lock_flag_the_clone_inherited_that_cant_be_cleared_refuses_the_copy(
        tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    src = tmp_path / "cv.pdf"
    src.write_bytes(b"%PDF")
    os.chflags(src, stat.UF_IMMUTABLE)                          # a locked original: its clone inherits the lock
    root = tmp_path / "uploads"
    def failing(fd: int, flags: int) -> int:
        ctypes.set_errno(errno.EEXIST)                          # an errno that must not read as "the name was taken"
        return -1
    monkeypatch.setattr(recovery, "_fchflags", failing)
    try:
        with pytest.raises(OSError, match="lock flag can't be cleared"):
            Uploads(root, lambda: ()).stage("tree1", str(src), _sha(src))
    finally:
        os.chflags(src, 0)
        for dirpath, _, names in os.walk(root):
            for n in names:
                os.chflags(os.path.join(dirpath, n), 0)
