"""Hands fix list (step 5a), files: a write or edit keeps the file as the owner had it apart from its text (line
endings, mode, extended attributes, hard links); OS and decoding errors are plain tool errors; fs.trash on a link
moves the link; fs.write onto a folder is a plain error; the file work runs off the daemon's loop; `ferb restore`
of a copy honours the protection lists. Offline, on scratch files in pytest's tmp_path."""
import asyncio
import ctypes
import os
import stat
import threading
from pathlib import Path
from typing import Any

import pytest

from ferb.act.fs import files, recovery
from ferb.act.fs.files import FsEdit, FsList, FsRead, FsTrash, FsWrite
from ferb.core.types import ActionFacts, Identity, ToolContext
from ferb.policy.protection import Protection
from tests.support.trash import use_trash

_lib = ctypes.CDLL("/usr/lib/libSystem.B.dylib", use_errno=True)
_lib.setxattr.argtypes = (ctypes.c_char_p, ctypes.c_char_p, ctypes.c_void_p, ctypes.c_size_t, ctypes.c_uint32,
                          ctypes.c_int)
_lib.getxattr.argtypes = (ctypes.c_char_p, ctypes.c_char_p, ctypes.c_void_p, ctypes.c_size_t, ctypes.c_uint32,
                          ctypes.c_int)
_lib.getxattr.restype = ctypes.c_ssize_t


def _set_xattr(path: Path, name: str, value: bytes) -> None:
    assert _lib.setxattr(os.fsencode(path), name.encode(), value, len(value), 0, 0) == 0


def _get_xattr(path: Path, name: str) -> bytes | None:
    buf = ctypes.create_string_buffer(256)
    n = _lib.getxattr(os.fsencode(path), name.encode(), buf, 256, 0, 0)
    return None if n < 0 else buf.raw[:n]


@pytest.fixture
def ws(tmp_path: Path) -> Path:
    folder = tmp_path / ".ferb" / "workspace"
    folder.mkdir(parents=True)
    return folder


def _ctx(ws: Path, closed: Any = None) -> ToolContext:
    return ToolContext("t", "w", "r", Identity.OWNER, frozenset({"fs:read", "fs:write"}), "dev", None, str(ws), (),
                       (), path_closed=closed)


def _run(cap: Any, ws: Path, closed: Any = None, **args: Any) -> Any:
    return asyncio.run(cap.run(cap.Input.model_validate(args), _ctx(ws, closed)))


def test_an_edit_keeps_crlf_line_endings_and_the_rest_of_the_file(ws: Path) -> None:
    f = ws / "win.txt"
    f.write_bytes(b"first line\r\nsecond line\r\nthird\r\n")
    r = _run(FsEdit(), ws, path=str(f), old_text="second line", new_text="2nd line")
    assert not r.is_error and f.read_bytes() == b"first line\r\n2nd line\r\nthird\r\n"
    # a worker's \n stands for the file's CRLF, in what it looks for and what it writes
    r = _run(FsEdit(), ws, path=str(f), old_text="2nd line\nthird", new_text="two\nthree")
    assert not r.is_error and f.read_bytes() == b"first line\r\ntwo\r\nthree\r\n"
    g = ws / "unix.txt"
    g.write_bytes(b"a\nb\r\nc\n")                         # mixed endings: only the replaced text changes
    assert not _run(FsEdit(), ws, path=str(g), old_text="c", new_text="C").is_error
    assert g.read_bytes() == b"a\nb\r\nC\n"


def test_a_write_and_an_edit_keep_mode_and_extended_attributes(ws: Path) -> None:
    f = ws / "script.sh"
    f.write_text("echo one\n")
    os.chmod(f, 0o754)
    _set_xattr(f, "com.ferb.test", b"kept")
    assert not _run(FsWrite(), ws, path=str(f), content="echo two\n", overwrite=True).is_error
    assert stat.S_IMODE(f.stat().st_mode) == 0o754 and _get_xattr(f, "com.ferb.test") == b"kept"
    assert not _run(FsEdit(), ws, path=str(f), old_text="two", new_text="three").is_error
    assert f.read_text() == "echo three\n"
    assert stat.S_IMODE(f.stat().st_mode) == 0o754 and _get_xattr(f, "com.ferb.test") == b"kept"
    ro = ws / "readonly.txt"                               # a read-only file is rewritten and stays read-only
    ro.write_text("old")
    os.chmod(ro, 0o444)
    assert not _run(FsWrite(), ws, path=str(ro), content="new", overwrite=True).is_error
    assert ro.read_text() == "new" and stat.S_IMODE(ro.stat().st_mode) == 0o444


def test_without_cloning_mode_and_extended_attributes_are_copied(ws: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    """A volume that can't clone (another disk): the fallback copies mode and extended attributes."""
    monkeypatch.setattr(files, "_clonefileat", None)
    f = ws / "plain.txt"
    f.write_text("one")
    os.chmod(f, 0o640)
    _set_xattr(f, "com.ferb.test", b"copied")
    assert not _run(FsWrite(), ws, path=str(f), content="two", overwrite=True).is_error
    assert f.read_text() == "two" and stat.S_IMODE(f.stat().st_mode) == 0o640
    assert _get_xattr(f, "com.ferb.test") == b"copied"


def test_a_file_with_other_hard_links_is_rewritten_in_place_and_says_so(ws: Path) -> None:
    f = ws / "a.txt"
    f.write_text("shared")
    other = ws / "b.txt"
    os.link(f, other)
    r = _run(FsWrite(), ws, path=str(f), content="new text", overwrite=True)
    assert not r.is_error and r.content["in_place"] is True and r.content["hard_links"] == 2
    assert other.read_text() == "new text" and os.stat(f).st_ino == os.stat(other).st_ino
    r = _run(FsEdit(), ws, path=str(other), old_text="new", new_text="newer")
    assert not r.is_error and r.content["in_place"] is True and f.read_text() == "newer text"


def test_os_and_decoding_errors_are_plain_tool_errors(ws: Path) -> None:
    binary = ws / "blob.bin"
    binary.write_bytes(b"\xff\xfe\x00bad")
    r = _run(FsEdit(), ws, path=str(binary), old_text="bad", new_text="good")
    assert r.is_error and "isn't UTF-8" in r.content["error"] and binary.read_bytes() == b"\xff\xfe\x00bad"
    locked = ws / "locked"
    locked.mkdir()
    (locked / "x.txt").write_text("x")
    os.chmod(locked, 0o000)
    try:
        for cap, args in ((FsList(), {"path": str(locked)}), (FsRead(), {"path": str(locked / "x.txt")}),
                          (FsEdit(), {"path": str(locked / "x.txt"), "old_text": "x", "new_text": "y"}),
                          (FsWrite(), {"path": str(locked / "new.txt"), "content": "y"})):
            r = _run(cap, ws, **args)                       # raised before: the gateway called it "outcome unknown"
            assert r.is_error and "error" in r.content, (cap.name, r.content)
    finally:
        os.chmod(locked, 0o755)


def test_writing_over_a_folder_is_a_plain_error(ws: Path) -> None:
    (ws / "dir").mkdir()
    r = _run(FsWrite(), ws, path=str(ws / "dir"), content="x", overwrite=True)
    assert r.is_error and "is a folder" in r.content["error"] and (ws / "dir").is_dir()


def test_trashing_a_link_moves_the_link_and_leaves_its_target(ws: Path, tmp_path: Path,
                                                             monkeypatch: pytest.MonkeyPatch) -> None:
    trash = tmp_path / ".Trash"
    trash.mkdir()
    use_trash(monkeypatch, trash)
    target = ws / "target.txt"
    target.write_text("keep me")
    link = ws / "shortcut"
    link.symlink_to(target)
    r = _run(FsTrash(), ws, path=str(link))
    assert not r.is_error, r.content
    assert target.read_text() == "keep me" and not os.path.lexists(link)
    assert (trash / "shortcut").is_symlink()
    assert FsTrash().real_path(FsTrash.Input(path=str(trash / "shortcut")), str(ws)) == trash.resolve() / "shortcut"


def test_the_file_work_runs_off_the_event_loop(ws: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    (ws / "a.txt").write_text("hi")
    seen: list[int] = []
    real = FsRead._run

    def spy(self: Any, args: Any, ctx: Any) -> Any:
        seen.append(threading.get_ident())
        return real(self, args, ctx)
    monkeypatch.setattr(FsRead, "_run", spy)

    async def go() -> int:
        await FsRead().run(FsRead.Input(path="a.txt"), _ctx(ws))
        return threading.get_ident()
    loop_thread = asyncio.run(go())
    assert seen and seen[0] != loop_thread


def test_file_tools_report_what_they_write_and_remove_as_facts(ws: Path) -> None:
    assert FsWrite().describe(FsWrite.Input(path="/x/a.txt", content="")).facts == ActionFacts(writes=("/x/a.txt",))
    assert FsEdit().describe(FsEdit.Input(path="/x/a.txt", old_text="a", new_text="b")).facts.writes == ("/x/a.txt",)
    assert FsTrash().describe(FsTrash.Input(path="/x/a.txt")).facts == ActionFacts(deletes=("/x/a.txt",))
    assert FsRead().describe(FsRead.Input(path="/x/a.txt")).facts is None


def test_restoring_a_copy_honours_the_protection_lists(tmp_path: Path) -> None:
    """A copy's original that now resolves into FERB's own state (a folder swapped for a link since) is refused;
    a protected place the owner's own `ferb restore` names is checked with only the never-lifted rules."""
    home, ferb = tmp_path / "home", tmp_path / "home" / ".ferb"
    (ferb / "workspace").mkdir(parents=True)
    root = ferb / "recovery"
    root.mkdir()
    (root / "copy.txt").write_text("old version")
    prot = Protection(home, ferb, [], ["~/.zshrc"], [])

    def closed(path: Path, access: str) -> str | None:
        rule = prot.fs_denied(path, access)
        return f"closed by {rule}" if rule else None
    record = {"saved_to": "copy.txt", "written_sha256": None}
    with pytest.raises(recovery.NotRestorable, match="closed by ferb_state"):
        recovery._restore_copy(record, ferb / "config.toml", ferb / "config.toml", root, closed)
    assert not (ferb / "config.toml").exists()
    with pytest.raises(recovery.NotRestorable, match="closed by write:~/.zshrc"):
        recovery._restore_copy(record, home / ".zshrc", home / ".zshrc", root, closed)
    back = recovery._restore_copy(record, home / "notes.txt", home / "notes.txt", root, closed)
    assert back.read_text() == "old version"
