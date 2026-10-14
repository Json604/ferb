"""Resolve literal paths and perform paged reads, atomic writes, edits and trash. Must not substitute files or cap deliverables.

File list/read/write/edit/trash using normal OS access. Relative paths resolve in the workspace.
Read provenance is recorded; it does not remove capabilities. Each tool declares the file it names (`file_paths`) for the
gateway's protection check (rule 10), and checks again the file it really opened (`ToolContext.path_closed`)."""

import fcntl
import os
import secrets
from datetime import datetime, timezone
from pathlib import Path
from typing import Callable

from pydantic import BaseModel, Field

from ferb.core.types import ActionSpec, FileAccess, Identity, RiskLevel, Target, ToolContext, ToolResult

READ_DEFAULT, READ_MAX, LIST_MAX = 256 * 1024, 2 * 1024 * 1024, 500

class PathRefused(Exception):
    pass


def owner_path(path: str, workspace: str) -> Path:
    """Expand explicit home paths; every other relative path is literal inside the workspace.

    Finder display names can differ from disk paths. Agents inspect candidates rather than substituting
    a same-looking existing file in code, especially before uploads, overwrites and trash operations.
    """
    p = Path(path).expanduser()
    return p if p.is_absolute() else Path(workspace) / p


def identity(p: Path) -> dict:
    """Local filesystem identity facts, not a judgment that this is the requested file."""
    st = p.stat()
    return {"path": str(p), "source": "local", "size": st.st_size,
            "modified_at": datetime.fromtimestamp(st.st_mtime, timezone.utc).isoformat(),
            "owner_uid": st.st_uid, "owner_gid": st.st_gid, "device": st.st_dev, "inode": st.st_ino,
            "mtime_ns": st.st_mtime_ns, "symlink": p.is_symlink()}


def opened_path(fd: int) -> Path:
    """What an open file or folder really is (macOS F_GETPATH): every link followed, the volume's own spelling."""
    return Path(os.fsdecode(fcntl.fcntl(fd, fcntl.F_GETPATH, bytes(1024)).split(b"\0", 1)[0]))


def opened_closed(ctx: ToolContext, fd: int, access: FileAccess) -> str | None:
    """The protection lists' answer for the file a tool really opened (rule 10): the error to return, else None. A
    file FERB can't tell is refused before anything is read or changed (a refusal, never an unknown outcome)."""
    if ctx.path_closed is None:
        return None
    try:
        where = opened_path(fd)
    except OSError as e:
        return f"FERB couldn't tell which file it opened ({e.strerror or e}); nothing was read or changed"
    return ctx.path_closed(where, access)


def _check_in(folder: int, name: str, closed: Callable[[Path], str | None] | None) -> None:
    """Raises PathRefused when the lists close `name` inside the opened folder, or when FERB can't tell which folder
    it opened: before anything is written, so the call fails cleanly and a retry isn't blocked."""
    if closed is None:
        return
    try:
        where = opened_path(folder)
    except OSError as e:
        raise PathRefused(f"FERB couldn't tell which folder it opened ({e.strerror or e}); nothing was written") from None
    refusal = closed(where / name)
    if refusal:
        raise PathRefused(refusal)


def make_folders(folder: Path, closed: Callable[[Path], str | None] | None = None) -> None:
    """`folder` and any missing folders above it (as mkdir -p), each made inside a folder FERB opened and checked
    (`closed`: the protection lists), so no folder is made inside FERB's own state through a link swapped in after the
    gateway's check. Raises PathRefused when the lists close one."""
    missing: list[str] = []
    top = folder
    while not top.exists() and top != top.parent:
        missing.append(top.name)
        top = top.parent
    if not missing:
        return
    fd = os.open(top, os.O_RDONLY | os.O_DIRECTORY | os.O_CLOEXEC)
    try:
        for name in reversed(missing):
            _check_in(fd, name, closed)
            try:
                os.mkdir(name, dir_fd=fd)
            except FileExistsError:
                pass
            child = os.open(name, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW | os.O_CLOEXEC, dir_fd=fd)
            os.close(fd)
            fd = child
    finally:
        os.close(fd)


def _sibling_temp(folder: int, name: str) -> tuple[int, str]:
    """A new temp file next to `name` inside the opened folder, never an existing file or link (0600)."""
    for _ in range(100):
        temp = f".{name}.{secrets.token_hex(4)}.ferb-tmp"
        try:
            flags = os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW | os.O_CLOEXEC
            return os.open(temp, flags, 0o600, dir_fd=folder), temp
        except FileExistsError:
            continue
    raise FileExistsError(f"no free temporary name next to {name}")


def atomic_text(p: Path, text: str, closed: Callable[[Path], str | None] | None = None) -> None:
    """Exclusive sibling temp preserves atomic replacement without touching a pre-existing scratch file. The temp is
    made and renamed inside one opened folder; `closed` checks that folder as macOS opened it (the protection lists),
    so a folder swapped for a link after the gateway's check can't move the write (raises PathRefused)."""
    try:
        folder = os.open(p.parent, os.O_RDONLY | os.O_DIRECTORY | os.O_CLOEXEC)
    except OSError as e:
        raise PathRefused(f"FERB couldn't open the folder for {p.name} ({e.strerror or e}); nothing was written") from None
    try:
        _check_in(folder, p.name, closed)
        fd, temp = _sibling_temp(folder, p.name)
        try:
            with os.fdopen(fd, "w", encoding="utf-8") as stream:
                stream.write(text)
            os.rename(temp, p.name, src_dir_fd=folder, dst_dir_fd=folder)     # replaces atomically (POSIX rename)
        finally:
            try:
                os.unlink(temp, dir_fd=folder)
            except FileNotFoundError:
                pass
    finally:
        os.close(folder)


def _write_check(ctx: ToolContext) -> Callable[[Path], str | None] | None:
    check = ctx.path_closed
    return (lambda path: check(path, "write")) if check is not None else None


write_check = _write_check                         # for other tools that write files (recordings)


def destination_closed(path: Path, closed: Callable[[Path], str | None] | None) -> str | None:
    """`closed`'s answer for a file about to be made at `path`, checked through the deepest folder above it that
    exists, as macOS opens that folder (F_GETPATH): another spelling of a protected folder, or a link, is seen as the
    folder it really is. A folder FERB can't open or tell is refused."""
    if closed is None:
        return None
    tail = [path.name]
    folder = path.parent
    while not folder.exists() and folder != folder.parent:
        tail.append(folder.name)
        folder = folder.parent
    try:
        fd = os.open(folder, os.O_RDONLY | os.O_DIRECTORY | os.O_CLOEXEC)
    except OSError as e:
        return f"FERB couldn't open {folder} ({e.strerror or e}); nothing was written"
    try:
        where = opened_path(fd)
    except OSError as e:
        return f"FERB couldn't tell which folder it opened ({e.strerror or e}); nothing was written"
    finally:
        os.close(fd)
    return closed(where.joinpath(*reversed(tail)))


def real_path(path: str, workspace: str) -> Path:
    """The file a call really acts on: symlinks followed and `..` applied after them, as the OS does. The fs lock and
    S1 recovery key on it; `normalize_path` stays textual (display, provenance) and can name another file when a
    symlinked folder is followed by `..`."""
    raw = owner_path(path, workspace)
    return raw.resolve() if raw.exists() else raw.parent.resolve() / raw.name


def resolve(path: str, ctx: ToolContext, must_exist: bool) -> Path:
    p = real_path(path, ctx.workspace)
    if must_exist and not p.exists():
        raise PathRefused(f"{path} does not exist")
    return p


def _source(p: Path, ctx: ToolContext) -> tuple[str, str]:
    for u in ctx.untrusted_roots:
        ur = Path(u).expanduser().resolve()
        if p == ur or ur in p.parents:
            return ("download", str(p))
    return ("file", str(p))


def _err(msg: str) -> ToolResult:
    return ToolResult({"error": msg}, is_error=True)


class _Base:
    identities = {Identity.OWNER, Identity.FERB}
    target_args = ("path",)                            # Target.path, normalized by the gateway (P19 hash)
    access: FileAccess = "read"

    def file_paths(self, args: BaseModel) -> list[tuple[str, FileAccess]]:
        """The file this call names and what it does to it, for the gateway's protection check (rule 10)."""
        return [(str(getattr(args, "path")), self.access)]

    def describe_path(self, args: BaseModel) -> str:
        return str(Path(str(getattr(args, "path"))).expanduser())

    def real_path(self, args: BaseModel, workspace: str) -> Path:
        """The file this call acts on (`real_path`): what the gateway locks and S1 copies."""
        return real_path(str(getattr(args, "path")), workspace)

    def locks(self, args: BaseModel) -> list[str]:
        """One file at a time per path; the path as given (relative ones are the workspace's)."""
        return [f"file:{self.describe_path(args)}"]


class FsList(_Base):
    name, description = "fs.list", "List every folder candidate through offset pages, including full path, source, size, owner and modification time."
    scopes, risk, reversible = {"fs:read"}, RiskLevel.LOW, True
    observation = True                                 # only reads: the final checker may use it (V3, W4)

    class Input(BaseModel):
        path: str = Field(description="Folder path (absolute, ~, or relative to FERB's workspace)")
        offset: int = Field(0, ge=0)
        limit: int = Field(LIST_MAX, ge=1, le=LIST_MAX)

    def describe(self, args: BaseModel) -> ActionSpec:
        return ActionSpec("read", Target(path=self.describe_path(args)))

    async def run(self, args: BaseModel, ctx: ToolContext) -> ToolResult:
        try:
            p = resolve(str(getattr(args, "path")), ctx, must_exist=True)
        except PathRefused as e:
            return _err(str(e))
        if not p.is_dir():
            return _err(f"{p} is not a folder")
        folder = os.open(p, os.O_RDONLY | os.O_DIRECTORY | os.O_CLOEXEC)
        try:
            refusal = opened_closed(ctx, folder, self.access)
            if refusal:
                return _err(refusal)
            children = [p / name for name in sorted(os.listdir(folder))]
        finally:
            os.close(folder)
        offset, limit = int(getattr(args, "offset")), int(getattr(args, "limit"))
        entries, errors = [], []
        for child in children[offset:offset + limit]:
            try:
                entries.append({"name": child.name, "kind": "dir" if child.is_dir() else "file", **identity(child)})
            except OSError as exc:
                errors.append({"path": str(child), "error": str(exc)})
        next_offset = offset + limit if offset + limit < len(children) else None
        return ToolResult({"path": str(p), "entries": entries, "errors": errors, "total": len(children),
                           "offset": offset, "next_offset": next_offset, "truncated": next_offset is not None,
                           "retrieval": "fs.list with next_offset until null" if next_offset is not None else None},
                          read_sources=(_source(p, ctx),))


class FsRead(_Base):
    name, description = "fs.read", "Read a text file (UTF-8), optionally a byte range."
    scopes, risk, reversible = {"fs:read"}, RiskLevel.LOW, True
    observation = True                                 # only reads: the final checker may use it (V3, W4)

    class Input(BaseModel):
        path: str
        offset: int = Field(0, ge=0)
        max_bytes: int = Field(READ_DEFAULT, gt=0, le=READ_MAX)

    def describe(self, args: BaseModel) -> ActionSpec:
        return ActionSpec("read", Target(path=self.describe_path(args)))

    async def run(self, args: BaseModel, ctx: ToolContext) -> ToolResult:
        try:
            p = resolve(str(getattr(args, "path")), ctx, must_exist=True)
        except PathRefused as e:
            return _err(str(e))
        if not p.is_file():
            return _err(f"{p} is not a file")
        offset, count = int(getattr(args, "offset")), int(getattr(args, "max_bytes"))
        with open(p, "rb") as stream:
            refusal = opened_closed(ctx, stream.fileno(), self.access)
            if refusal:
                return _err(refusal)
            stream.seek(offset)
            data = stream.read(count)
            # Finish the current UTF-8 scalar instead of replacing halves across pages. At most three
            # extra bytes are transport framing, never an artifact cap.
            for _ in range(3):
                try:
                    text = data.decode("utf-8")
                    break
                except UnicodeDecodeError as exc:
                    if exc.reason != "unexpected end of data":
                        return _err(f"{p} is not UTF-8 at byte {offset + exc.start}; use a binary-aware tool")
                    more = stream.read(1)
                    if not more:
                        return _err(f"{p} ends with invalid UTF-8; use a binary-aware tool")
                    data += more
            else:
                try:
                    text = data.decode("utf-8")
                except UnicodeDecodeError:
                    return _err(f"offset {offset} is not a UTF-8 boundary; resume at the returned next_offset")
        facts = identity(p)
        end = offset + len(data)
        next_offset = end if end < facts["size"] else None
        return ToolResult({**facts, "text": text, "offset": offset, "next_offset": next_offset,
                           "truncated": next_offset is not None,
                           "retrieval": "fs.read with next_offset until null" if next_offset is not None else None},
                          read_sources=(_source(p, ctx),))


class FsWrite(_Base):
    name, description = "fs.write", "Create or overwrite a text file."
    scopes, risk, reversible = {"fs:write"}, RiskLevel.MEDIUM, False
    access = "write"

    class Input(BaseModel):
        path: str
        content: str
        overwrite: bool = False

    def describe(self, args: BaseModel) -> ActionSpec:
        return ActionSpec("write_file", Target(path=self.describe_path(args)), content=str(getattr(args, "content")))

    def recovery_reason(self, args: BaseModel) -> str | None:
        """S1: why the gateway copies an existing file first; None when this call can't change one (no overwrite)."""
        return "overwrite" if getattr(args, "overwrite") else None

    async def run(self, args: BaseModel, ctx: ToolContext) -> ToolResult:
        try:
            p = resolve(str(getattr(args, "path")), ctx, must_exist=False)
        except PathRefused as e:
            return _err(str(e))
        if p.exists() and not getattr(args, "overwrite"):
            return _err(f"{p} exists (set overwrite=true to replace it)")
        try:
            make_folders(p.parent, _write_check(ctx))
            atomic_text(p, str(getattr(args, "content")), _write_check(ctx))
        except PathRefused as e:
            return _err(str(e))
        return ToolResult({**identity(p), "bytes": p.stat().st_size})


class FsEdit(_Base):
    name, description = "fs.edit", "Replace one exact occurrence of old_text with new_text in a file."
    scopes, risk, reversible = {"fs:write"}, RiskLevel.MEDIUM, False
    access = "write"

    class Input(BaseModel):
        path: str
        old_text: str = Field(min_length=1)
        new_text: str

    def describe(self, args: BaseModel) -> ActionSpec:
        return ActionSpec("write_file", Target(path=self.describe_path(args)), content=str(getattr(args, "new_text")))

    def recovery_reason(self, args: BaseModel) -> str | None:
        """S1: the gateway copies the file before an edit (an edit that refuses drops the copy)."""
        return "edit"

    async def run(self, args: BaseModel, ctx: ToolContext) -> ToolResult:
        try:
            p = resolve(str(getattr(args, "path")), ctx, must_exist=True)
        except PathRefused as e:
            return _err(str(e))
        with open(p, encoding="utf-8") as stream:
            refusal = opened_closed(ctx, stream.fileno(), self.access)
            if refusal:
                return _err(refusal)
            text = stream.read()
        old = str(getattr(args, "old_text"))
        n = text.count(old)
        if n != 1:
            return _err(f"old_text occurs {n} times; it must occur exactly once")
        try:
            atomic_text(p, text.replace(old, str(getattr(args, "new_text")), 1), _write_check(ctx))
        except PathRefused as e:
            return _err(str(e))
        return ToolResult({"path": str(p), "replaced": 1}, read_sources=(_source(p, ctx),))


class FsTrash(_Base):
    name, description = "fs.trash", "Move a file or folder to the Trash (recoverable)."
    scopes, risk, reversible = {"fs:write"}, RiskLevel.LOW, True
    access = "remove"

    class Input(BaseModel):
        path: str

    def describe(self, args: BaseModel) -> ActionSpec:
        return ActionSpec("trash", Target(path=self.describe_path(args)))

    async def run(self, args: BaseModel, ctx: ToolContext) -> ToolResult:
        try:
            p = resolve(str(getattr(args, "path")), ctx, must_exist=True)
        except PathRefused as e:
            return _err(str(e))
        if ctx.path_closed is not None:
            # The Trash takes a path, not an open file: check the item as macOS opens it (F_GETPATH; opened for its
            # name only), so the lists see the kernel's own spelling of what moves, never an alias of it
            try:
                item = os.open(p, getattr(os, "O_EVTONLY", 0x8000) | os.O_NONBLOCK | os.O_CLOEXEC)
            except OSError as e:
                return _err(f"FERB couldn't open {p.name} to check it ({e.strerror or e}); nothing was moved")
            try:
                where = opened_path(item)
            except OSError as e:
                return _err(f"FERB couldn't tell which file it opened ({e.strerror or e}); nothing was moved")
            finally:
                os.close(item)
            refusal = ctx.path_closed(where, self.access)
            if refusal:
                return _err(refusal)
        try:
            trashed_to = move_to_trash(p)
        except TrashRefused as e:
            return _err(f"couldn't move to Trash: {e}")
        return ToolResult({"path": str(p), "trashed_to": trashed_to})


class TrashRefused(Exception):
    pass


def move_to_trash(p: Path) -> str | None:
    """The Mac's Trash (NSFileManager): where the item went, None if macOS didn't say. The gateway records both paths
    durably (S1: ferb/act/fs/recovery.py), so getting it back never depends on Finder's Put Back."""
    from Foundation import NSURL, NSFileManager
    ok, result_url, err = NSFileManager.defaultManager().trashItemAtURL_resultingItemURL_error_(
        NSURL.fileURLWithPath_(str(p)), None, None)
    if not ok:
        raise TrashRefused(str(err))
    return str(result_url.path()) if result_url else None


CAPABILITIES = [FsList(), FsRead(), FsWrite(), FsEdit(), FsTrash()]


def normalize_path(path: str, workspace: str) -> str:
    """One form for policy comparisons: the same place `resolve` uses (owner_path), normalized."""
    return os.path.normpath(str(owner_path(path, workspace)))
