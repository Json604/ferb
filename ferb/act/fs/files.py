"""Resolve literal paths and perform paged reads, atomic writes, edits and trash. Must not substitute files or cap deliverables.

File list/read/write/edit/trash using normal OS access. Relative paths resolve in the workspace.
Read provenance is recorded; it does not remove capabilities. Each tool declares the file it names (`file_paths`) for the
gateway's protection check (rule 10), and checks again the file it really opened (`ToolContext.path_closed`).
The file work runs in a worker thread, never on the daemon's loop; an OS or decoding error is the tool's plain error
(nothing changed), never an exception the gateway would record as an unknown outcome.
A write or edit keeps the file as the owner had it apart from its text: its line endings (an edit never rewrites the
rest of the file), mode, extended attributes and ACL (the new version starts as an APFS clone of the old one; where
cloning isn't possible, mode and extended attributes are copied), and its hard links: a file with more than one name
is rewritten in place, so every name still shows the new text (not atomic, but S1's copy was kept first)."""

import asyncio
import ctypes
import errno
import fcntl
import os
import secrets
import stat
from dataclasses import dataclass
from datetime import datetime, timezone
from pathlib import Path
from typing import Any, Callable

from pydantic import BaseModel, Field

from ferb.core.types import ActionFacts, ActionSpec, FileAccess, Identity, RiskLevel, Target, ToolContext, ToolResult

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


def make_folders(folder: Path, closed: Callable[[Path], str | None] | None = None, *, exact: bool = False) -> None:
    """`folder` and any missing folders above it (as mkdir -p), each made inside a folder FERB opened and checked
    (`closed`: the protection lists), so no folder is made inside FERB's own state through a link swapped in after the
    gateway's check. `exact`: the deepest folder that exists must be where `folder` says, as macOS opened it (no link
    anywhere above), before anything is made (restore). Raises PathRefused when the lists close one, or on such a
    link."""
    missing: list[str] = []
    top = folder
    while not top.exists() and top != top.parent:
        missing.append(top.name)
        top = top.parent
    if not missing:
        return
    fd = os.open(top, os.O_RDONLY | os.O_DIRECTORY | os.O_CLOEXEC)
    try:
        if exact:
            try:
                here = opened_path(fd)
            except OSError as e:
                raise PathRefused(f"FERB couldn't tell which folder it opened ({e.strerror or e})") from None
            if str(here).casefold() != str(top).casefold():
                raise PathRefused(f"{top} isn't where it was (it now leads to {here})")
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
        temp = _temp_name(name)
        try:
            flags = os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW | os.O_CLOEXEC
            return os.open(temp, flags, 0o600, dir_fd=folder), temp
        except FileExistsError:
            continue
    raise FileExistsError(f"no free temporary name next to {name}")


def _temp_name(name: str) -> str:
    return f".{name}.{secrets.token_hex(4)}.ferb-tmp"


try:
    _clonefileat: Any = ctypes.CDLL("/usr/lib/libSystem.B.dylib", use_errno=True).clonefileat
    _clonefileat.argtypes = (ctypes.c_int, ctypes.c_char_p, ctypes.c_int, ctypes.c_char_p, ctypes.c_uint32)
    _clonefileat.restype = ctypes.c_int
except (OSError, AttributeError):                 # not macOS: the copy fallback carries mode and xattrs
    _clonefileat = None
_CLONE_NOFOLLOW = 0x0001


def _clone_temp(folder: int, name: str) -> str | None:
    """An APFS clone of `name` next to it (clonefileat, never through a link): the old version's data, mode, extended
    attributes, ACL and flags, ready to be rewritten. None when the volume can't clone (the caller copies instead)."""
    if _clonefileat is None:
        return None
    for _ in range(100):
        temp = _temp_name(name)
        if _clonefileat(folder, os.fsencode(name), folder, os.fsencode(temp), _CLONE_NOFOLLOW) == 0:
            return temp
        code = ctypes.get_errno()
        if code != errno.EEXIST:
            return None
    return None


def _xattr_fns() -> Any:
    """macOS's fd extended-attribute calls (Python's os has none on macOS), or None where they're missing."""
    try:
        lib = ctypes.CDLL("/usr/lib/libSystem.B.dylib", use_errno=True)
        lib.flistxattr.argtypes = (ctypes.c_int, ctypes.c_char_p, ctypes.c_size_t, ctypes.c_int)
        lib.flistxattr.restype = ctypes.c_ssize_t
        lib.fgetxattr.argtypes = (ctypes.c_int, ctypes.c_char_p, ctypes.c_void_p, ctypes.c_size_t, ctypes.c_uint32,
                                  ctypes.c_int)
        lib.fgetxattr.restype = ctypes.c_ssize_t
        lib.fsetxattr.argtypes = (ctypes.c_int, ctypes.c_char_p, ctypes.c_void_p, ctypes.c_size_t, ctypes.c_uint32,
                                  ctypes.c_int)
        lib.fsetxattr.restype = ctypes.c_int
        return lib
    except (OSError, AttributeError):
        return None


_XATTR = _xattr_fns()


def _xattrs(fd: int) -> dict[bytes, bytes]:
    """Every extended attribute of an open file (best effort: one that can't be read is left out)."""
    if _XATTR is None:
        return {}
    size = _XATTR.flistxattr(fd, None, 0, 0)
    if size <= 0:
        return {}
    names = ctypes.create_string_buffer(size)
    size = _XATTR.flistxattr(fd, names, size, 0)
    out = {}
    for name in names.raw[:max(size, 0)].split(b"\0"):
        if not name:
            continue
        length = _XATTR.fgetxattr(fd, name, None, 0, 0, 0)
        if length < 0:
            continue
        value = ctypes.create_string_buffer(max(length, 1))
        got = _XATTR.fgetxattr(fd, name, value, length, 0, 0)
        if got >= 0:
            out[name] = value.raw[:got]
    return out


def _copy_attributes(src: int, dst: int) -> None:
    """The fallback when cloning isn't possible: the old version's mode and extended attributes onto the new one
    (its ACL can't be copied this way). Best effort for attributes macOS won't let FERB set."""
    os.fchmod(dst, stat.S_IMODE(os.fstat(src).st_mode))
    if _XATTR is None:
        return
    for name, value in _xattrs(src).items():
        _XATTR.fsetxattr(dst, name, value, len(value), 0, 0)


def _write_all(fd: int, data: bytes) -> None:
    view = memoryview(data)
    while view:
        n = os.write(fd, view)
        view = view[n:]


@dataclass(frozen=True)
class Written:
    """How a text file was written: replaced atomically (the usual), or rewritten in place because it has other hard
    links (`links` names in all), which a replacement would have cut off from the new text."""
    in_place: bool = False
    links: int = 1


def atomic_text(p: Path, text: str, closed: Callable[[Path], str | None] | None = None) -> Written:
    """Write `text` (UTF-8, line endings exactly as given) to `p`. The new version is made next to it and renamed over
    it inside one opened folder; `closed` checks that folder as macOS opened it (the protection lists), so a folder
    swapped for a link after the gateway's check can't move the write (raises PathRefused). An existing file keeps its
    mode, extended attributes and ACL (an APFS clone of it is rewritten; elsewhere mode and extended attributes are
    copied), and one with other hard links is rewritten in place (see the module). A folder at `p` raises PathRefused.
    An OSError means nothing was changed, except in place, where the file may be part written (S1 kept its copy)."""
    try:
        folder = os.open(p.parent, os.O_RDONLY | os.O_DIRECTORY | os.O_CLOEXEC)
    except OSError as e:
        raise PathRefused(f"FERB couldn't open the folder for {p.name} ({e.strerror or e}); nothing was written") from None
    data = text.encode("utf-8")
    try:
        _check_in(folder, p.name, closed)
        try:
            old = os.stat(p.name, dir_fd=folder, follow_symlinks=False)
        except FileNotFoundError:
            old = None
        if old is not None and stat.S_ISDIR(old.st_mode):
            raise PathRefused(f"{p} is a folder, not a file; nothing was written")
        if old is not None and stat.S_ISREG(old.st_mode) and old.st_nlink > 1:
            fd = os.open(p.name, os.O_WRONLY | os.O_NOFOLLOW | os.O_CLOEXEC, dir_fd=folder)
            try:
                st = os.fstat(fd)
                if (st.st_dev, st.st_ino) != (old.st_dev, old.st_ino):
                    raise PathRefused(f"{p} changed while FERB wrote it; nothing was written")
                os.ftruncate(fd, 0)
                _write_all(fd, data)
            finally:
                os.close(fd)
            return Written(in_place=True, links=old.st_nlink)
        regular = old is not None and stat.S_ISREG(old.st_mode)
        temp = _clone_temp(folder, p.name) if regular else None
        cloned = temp is not None
        try:
            if cloned:
                assert old is not None and temp is not None
                # the clone has the old mode; the owner's write bit for the rewrite, the old mode back after
                os.chmod(temp, stat.S_IMODE(old.st_mode) | stat.S_IWUSR, dir_fd=folder, follow_symlinks=False)
                fd = os.open(temp, os.O_WRONLY | os.O_TRUNC | os.O_NOFOLLOW | os.O_CLOEXEC, dir_fd=folder)
            else:
                fd, temp = _sibling_temp(folder, p.name)
            try:
                _write_all(fd, data)
                if cloned:
                    assert old is not None
                    os.fchmod(fd, stat.S_IMODE(old.st_mode))
                elif regular:
                    src = os.open(p.name, os.O_RDONLY | os.O_NOFOLLOW | os.O_CLOEXEC, dir_fd=folder)
                    try:
                        _copy_attributes(src, fd)
                    finally:
                        os.close(src)
            finally:
                os.close(fd)
            os.rename(temp, p.name, src_dir_fd=folder, dst_dir_fd=folder)     # replaces atomically (POSIX rename)
        finally:
            if temp is not None:
                try:
                    os.unlink(temp, dir_fd=folder)
                except FileNotFoundError:
                    pass
    finally:
        os.close(folder)
    return Written()


def new_file(p: Path, data: bytes, closed: Callable[[Path], str | None] | None = None) -> None:
    """Create `p` with `data`, never replacing anything (T-1065: a PDF FERB made): inside one opened folder that
    `closed` checks as macOS opened it (the protection lists), with O_EXCL, so an existing file, a link or a folder
    there refuses (PathRefused) and nothing is written. A write that fails part way removes the part written."""
    try:
        folder = os.open(p.parent, os.O_RDONLY | os.O_DIRECTORY | os.O_CLOEXEC)
    except OSError as e:
        raise PathRefused(f"FERB couldn't open the folder for {p.name} ({e.strerror or e}); nothing was written") from None
    try:
        _check_in(folder, p.name, closed)
        try:
            fd = os.open(p.name, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW | os.O_CLOEXEC, 0o644,
                         dir_fd=folder)
        except FileExistsError:
            raise PathRefused(f"{p} already exists; FERB never overwrites it, so nothing was written") from None
        try:
            _write_all(fd, data)
        except OSError:
            os.close(fd)
            os.unlink(p.name, dir_fd=folder)
            raise
        os.close(fd)
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


def _full_disk_access() -> object:
    from ferb.device.permissions import full_disk_access
    return full_disk_access()


def _err(msg: str) -> ToolResult:
    """A file tool's error. macOS refusing a protected folder while FERB lacks Full Disk Access says so, so the worker
    asks the owner to turn it on (owner 2026-10-26: ask when first needed, like Claude and ChatGPT)."""
    if "Operation not permitted" in msg:
        from ferb.device.permissions import Status
        if _full_disk_access() is not Status.GRANTED:
            return ToolResult({"code": "needs_permission", "permission": "Full Disk Access", "error": msg,
                               "detail": "macOS blocks this folder until FERB has Full Disk Access: ask the owner to turn "
                                         "it on in System Settings → Privacy & Security → Full Disk Access (add FERB), "
                                         "then try again"}, is_error=True)
    return ToolResult({"error": msg}, is_error=True)


def _os_error(e: OSError, what: str) -> ToolResult:
    """An OS refusal as the tool's plain error, naming what didn't happen (Full Disk Access advice when it applies)."""
    where = f" ({e.filename})" if getattr(e, "filename", None) else ""
    return _err(f"{e.strerror or e}{where}: {what}")


class _Base:
    identities = {Identity.OWNER, Identity.FERB}
    target_args = ("path",)                            # Target.path, normalized by the gateway (P19 hash)
    access: FileAccess = "read"
    undone = "nothing was read"                        # what an OS error means for this tool

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

    async def run(self, args: BaseModel, ctx: ToolContext) -> ToolResult:
        """The file work in a worker thread; an OS or decoding error is a plain error (nothing changed)."""
        return await asyncio.to_thread(self._guarded, args, ctx)

    def _guarded(self, args: BaseModel, ctx: ToolContext) -> ToolResult:
        try:
            return self._run(args, ctx)
        except PathRefused as e:
            return _err(str(e))
        except UnicodeDecodeError as e:
            return _err(f"the file isn't UTF-8 text (byte {e.start}): {self.undone}; use a binary-aware tool")
        except OSError as e:
            return _os_error(e, self.undone)

    def _run(self, args: BaseModel, ctx: ToolContext) -> ToolResult:
        raise NotImplementedError


class FsList(_Base):
    name, description = "fs.list", "List every folder candidate through offset pages, including full path, source, size, owner and modification time."
    scopes, risk, reversible = {"fs:read"}, RiskLevel.LOW, True
    observation = True                                 # only reads: the final checker may use it (V3, W4)
    probe_safe = True  # reads state, moves nothing, returns at once: a loop's probe may use it (L31)
    undone = "nothing was listed"

    class Input(BaseModel):
        path: str = Field(description="Folder path (absolute, ~, or relative to FERB's workspace)")
        offset: int = Field(0, ge=0)
        limit: int = Field(LIST_MAX, ge=1, le=LIST_MAX)

    def describe(self, args: BaseModel) -> ActionSpec:
        return ActionSpec("read", Target(path=self.describe_path(args)))

    def _run(self, args: BaseModel, ctx: ToolContext) -> ToolResult:
        p = resolve(str(getattr(args, "path")), ctx, must_exist=True)
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
    reads_content = True                               # reads what the file says: a checker's look at it (L39 M8)
    probe_safe = True  # reads state, moves nothing, returns at once: a loop's probe may use it (L31)

    class Input(BaseModel):
        path: str
        offset: int = Field(0, ge=0)
        max_bytes: int = Field(READ_DEFAULT, gt=0, le=READ_MAX)

    def describe(self, args: BaseModel) -> ActionSpec:
        return ActionSpec("read", Target(path=self.describe_path(args)))

    def _run(self, args: BaseModel, ctx: ToolContext) -> ToolResult:
        p = resolve(str(getattr(args, "path")), ctx, must_exist=True)
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


def _written_facts(p: Path, how: Written) -> dict[str, Any]:
    """What a write left, read back; a failure to read it back after the write is said, never an error (it ran)."""
    out: dict[str, Any] = {"in_place": True, "hard_links": how.links} if how.in_place else {}
    try:
        return {**identity(p), "bytes": p.stat().st_size, **out}
    except OSError as e:
        return {"path": str(p), "written": True, "readback_error": e.strerror or str(e), **out}


class FsWrite(_Base):
    name, description = "fs.write", "Create or overwrite a text file."
    scopes, risk, reversible = {"fs:write"}, RiskLevel.MEDIUM, False
    access = "write"
    undone = "nothing was written"

    class Input(BaseModel):
        path: str
        content: str
        overwrite: bool = False

    def describe(self, args: BaseModel) -> ActionSpec:
        path = self.describe_path(args)
        return ActionSpec("write_file", Target(path=path), content=str(getattr(args, "content")),
                          facts=ActionFacts(writes=(path,)))

    def recovery_reason(self, args: BaseModel) -> str | None:
        """S1: why the gateway copies an existing file first; None when this call can't change one (no overwrite)."""
        return "overwrite" if getattr(args, "overwrite") else None

    def _run(self, args: BaseModel, ctx: ToolContext) -> ToolResult:
        p = resolve(str(getattr(args, "path")), ctx, must_exist=False)
        if p.is_dir():
            return _err(f"{p} is a folder, not a file; nothing was written")
        if p.exists() and not getattr(args, "overwrite"):
            return _err(f"{p} exists (set overwrite=true to replace it)")
        make_folders(p.parent, _write_check(ctx))
        how = atomic_text(p, str(getattr(args, "content")), _write_check(ctx))
        return ToolResult(_written_facts(p, how))


class FsEdit(_Base):
    name, description = ("fs.edit", "Replace one exact occurrence of old_text with new_text in a UTF-8 text file. The "
                         "rest of the file stays byte for byte, its line endings too (in a file with CRLF line "
                         "endings, \\n in old_text and new_text means its CRLF). Include enough surrounding text "
                         "that old_text matches only the occurrence you mean.")
    scopes, risk, reversible = {"fs:write"}, RiskLevel.MEDIUM, False
    access = "write"
    undone = "nothing was changed"

    class Input(BaseModel):
        path: str
        old_text: str = Field(min_length=1)
        new_text: str

    def describe(self, args: BaseModel) -> ActionSpec:
        path = self.describe_path(args)
        return ActionSpec("write_file", Target(path=path), content=str(getattr(args, "new_text")),
                          facts=ActionFacts(writes=(path,)))

    def recovery_reason(self, args: BaseModel) -> str | None:
        """S1: the gateway copies the file before an edit (an edit that refuses drops the copy)."""
        return "edit"

    def _run(self, args: BaseModel, ctx: ToolContext) -> ToolResult:
        p = resolve(str(getattr(args, "path")), ctx, must_exist=True)
        if not p.is_file():
            return _err(f"{p} is not a file; nothing was changed")
        with open(p, "rb") as stream:
            refusal = opened_closed(ctx, stream.fileno(), self.access)
            if refusal:
                return _err(refusal)
            text = stream.read().decode("utf-8")          # as stored: no line-ending translation
        old, new = str(getattr(args, "old_text")), str(getattr(args, "new_text"))
        n = text.count(old)
        if n == 0 and "\r\n" in text and "\n" in old and "\r" not in old:
            # the file's own line endings: the worker's \n stands for its CRLF (a mechanical spelling, not a guess)
            crlf_old, crlf_new = old.replace("\n", "\r\n"), new.replace("\r\n", "\n").replace("\n", "\r\n")
            if text.count(crlf_old):
                old, new, n = crlf_old, crlf_new, text.count(crlf_old)
        if n != 1:
            return _err(f"old_text occurs {n} times; it must occur exactly once")
        how = atomic_text(p, text.replace(old, new, 1), _write_check(ctx))
        out: dict[str, Any] = {"path": str(p), "replaced": 1}
        if how.in_place:
            out |= {"in_place": True, "hard_links": how.links}
        return ToolResult(out, read_sources=(_source(p, ctx),))


def link_path(path: str, workspace: str) -> Path:
    """The item a call names, its last part never followed: a link names the link itself (fs.trash moves the link,
    not what it points to). Its folder is resolved as the OS does."""
    raw = owner_path(path, workspace)
    return raw.parent.resolve() / raw.name


class FsTrash(_Base):
    name, description = ("fs.trash", "Move a file or folder to the Trash (recoverable). A link goes itself; what it "
                         "points to stays.")
    scopes, risk, reversible = {"fs:write"}, RiskLevel.LOW, True
    access = "remove"
    undone = "nothing was moved"

    class Input(BaseModel):
        path: str

    def describe(self, args: BaseModel) -> ActionSpec:
        path = self.describe_path(args)
        return ActionSpec("trash", Target(path=path), facts=ActionFacts(deletes=(path,)))

    def real_path(self, args: BaseModel, workspace: str) -> Path:
        """The item itself (`link_path`): a link is locked, and kept by S1's record, as the link."""
        return link_path(str(getattr(args, "path")), workspace)

    def _run(self, args: BaseModel, ctx: ToolContext) -> ToolResult:
        given = str(getattr(args, "path"))
        p = link_path(given, ctx.workspace)
        if not os.path.lexists(p):
            return _err(f"{given} does not exist")
        try:
            folder = os.open(p.parent, O_SEARCH | os.O_DIRECTORY | os.O_CLOEXEC)
        except OSError as e:
            return _err(f"FERB couldn't open the folder of {p.name} ({e.strerror or e}); nothing was moved")
        try:
            t = trash_item(folder, p.name, ctx.path_closed, self.access)
        finally:
            os.close(folder)
        if t.trashed_to is None:
            return _err(t.error or "nothing was moved")
        if t.error is not None:                      # S1 records it as needing the owner (gateway, recovery)
            return ToolResult({"error": t.error, "path": str(t.where), "trashed_to": t.trashed_to, "verified": False},
                              is_error=True)
        return ToolResult({"path": str(t.where), "trashed_to": t.trashed_to, "device": t.device, "inode": t.inode})


class TrashRefused(Exception):
    pass


O_SEARCH = getattr(os, "O_SEARCH", os.O_RDONLY)   # a folder opened only to work inside it, never listed: macOS lets
                                                  # FERB into ~/.Trash this way without Full Disk Access (measured)
RENAME_EXCL = 0x00000004                          # renameatx_np: fail rather than replace what stands at the target
TRASH_DIRS = (".Trash", ".Trashes")               # macOS Trash folders: the home volume's, and other volumes'

try:
    _renameatx_np: Any = ctypes.CDLL("/usr/lib/libSystem.B.dylib", use_errno=True).renameatx_np
    _renameatx_np.argtypes = (ctypes.c_int, ctypes.c_char_p, ctypes.c_int, ctypes.c_char_p, ctypes.c_uint)
    _renameatx_np.restype = ctypes.c_int
except (OSError, AttributeError):                 # not macOS: nothing moves to the Trash
    _renameatx_np = None


def move_excl(src_dir: int, src: str, dst_dir: int, dst: str) -> None:
    """Move `src` in one open folder to `dst` in another (renameatx_np, RENAME_EXCL). Both names are looked up only
    inside those folders, so a link swapped in anywhere above them changes nothing, and whatever stands at `dst` is
    never replaced: FileExistsError."""
    if _renameatx_np is None:
        raise OSError(errno.ENOTSUP, "renameatx_np is unavailable")
    if _renameatx_np(src_dir, os.fsencode(src), dst_dir, os.fsencode(dst), RENAME_EXCL) != 0:
        code = ctypes.get_errno()
        raise OSError(code, os.strerror(code))       # EEXIST is raised as FileExistsError


def in_trash(folder: Path) -> bool:
    """`folder` is a Trash folder or inside one (named .Trash or .Trashes): the rule restore uses (recovery)."""
    return any(f.name in TRASH_DIRS for f in (folder, *folder.parents))


def trash_folder(near: Path) -> Path:
    """The Trash macOS keeps for the volume holding `near` (NSFileManager's NSTrashDirectory: ~/.Trash on the startup
    disk, <volume>/.Trashes/<uid> on another disk). A missing one is made by macOS only after FERB checked that no
    link stands at it or above it, so it is never made through a link. Raises TrashRefused when there is none."""
    found = _trash_url(near, False)
    if found is not None and os.path.isdir(found) and not os.path.islink(found):
        return found
    expected = found if found is not None else _expected_trash(near)
    if expected is None:
        raise TrashRefused("macOS named no Trash for it")
    if os.path.lexists(expected):
        raise TrashRefused(f"a link or a file stands where the Trash should be ({expected})")
    if os.path.realpath(expected.parent) != str(expected.parent):
        raise TrashRefused(f"a link stands above where the Trash should be ({expected})")
    made = _trash_url(near, True)
    if made != expected:
        raise TrashRefused(f"macOS made the Trash at {made}, not at {expected}")
    return made


def _owner_home() -> Path:
    import pwd
    return Path(pwd.getpwuid(os.getuid()).pw_dir)


def _expected_trash(near: Path) -> Path | None:
    """Where macOS keeps the Trash for `near`'s volume when it has none yet: ~/.Trash on the home's volume, else
    <volume>/.Trashes/<uid>."""
    try:
        dev = os.stat(near).st_dev
        home = _owner_home()
        if os.stat(home).st_dev == dev:
            return home / ".Trash"
        top = Path(os.path.realpath(near))
        while top != top.parent and os.stat(top.parent).st_dev == dev:
            top = top.parent
        return top / ".Trashes" / str(os.getuid())
    except OSError:
        return None


def _trash_url(near: Path, create: bool) -> Path | None:
    from Foundation import NSURL, NSFileManager, NSTrashDirectory, NSUserDomainMask
    url, err = NSFileManager.defaultManager().URLForDirectory_inDomain_appropriateForURL_create_error_(
        NSTrashDirectory, NSUserDomainMask, NSURL.fileURLWithPath_(str(near)), create, None)
    return Path(str(url.path())) if url is not None and url.path() else None


@dataclass(frozen=True)
class Trashed:
    """What a move to the Trash did. `trashed_to` None: nothing moved (`error` says why). `error` None: what FERB
    checked is in the Trash at `trashed_to`, with that `device` and `inode`, read through the open Trash. Both set: an
    item moved but isn't (or can't be shown to be) what FERB checked; it stays in the Trash for the owner."""
    where: Path | None
    trashed_to: str | None = None
    error: str | None = None
    device: int | None = None
    inode: int | None = None


def trash_item(folder: int, name: str, closed: Callable[[Path, FileAccess], str | None] | None = None,
               access: FileAccess = "remove") -> Trashed:
    """`name` in the open `folder`, to the Trash of its volume (fs.trash, downloads retention). FERB moves the item
    itself, between two folders it holds open, both checked as opened (F_GETPATH): the item in its folder, with its
    device and inode noted without opening it (a mode-000 file or a socket moves too); and the Trash, which must be on
    the item's disk, be a Trash folder, and be open to the lists for each name the item would get. One move, never
    replacing anything in the Trash. Nothing is looked up by path after the check, so a link swapped in can't redirect
    it, and nothing is ever moved out of the Trash again: getting an item back is `ferb restore`'s, through S1's
    record (rule 10)."""
    try:
        where = opened_path(folder) / name
        checked = os.stat(name, dir_fd=folder, follow_symlinks=False)
    except OSError as e:
        return Trashed(None, error=f"FERB couldn't tell what it would move ({e.strerror or e}); nothing was moved")
    if closed is not None:
        refusal = closed(where, access)
        if refusal:
            return Trashed(where, error=refusal)
    try:
        bin_path = trash_folder(where.parent)
    except TrashRefused as e:
        return Trashed(where, error=f"couldn't move to Trash: no Trash for {where.parent} ({e}); nothing was moved")
    try:
        trash = os.open(bin_path, O_SEARCH | os.O_DIRECTORY | os.O_NOFOLLOW | os.O_CLOEXEC)
    except OSError as e:
        return Trashed(where, error=f"FERB couldn't open the Trash at {bin_path} ({e.strerror or e}); nothing was moved")
    try:
        try:
            bin_where, bin_dev = opened_path(trash), os.fstat(trash).st_dev
        except OSError as e:
            return Trashed(where, error=f"FERB couldn't tell which Trash it opened ({e.strerror or e}); nothing was "
                                        "moved")
        if bin_dev != checked.st_dev:                 # a move to another disk would be a copy, not a move
            return Trashed(where, error=f"{where} is on a different disk from its Trash ({bin_where}); FERB moves an "
                                        "item to the Trash only on its own disk, so nothing was moved")
        if not in_trash(bin_where):
            return Trashed(where, error=f"{bin_where} isn't a Trash folder, so nothing was moved")
        try:
            landed = _move_into(folder, name, trash, bin_where, closed)
        except PathRefused as e:
            return Trashed(where, error=str(e))
        except OSError as e:
            return Trashed(where, error=f"couldn't move {where} to the Trash ({e.strerror or e}); nothing was moved")
        trashed_to = str(bin_where / landed)          # known before anything that can fail after the move
        try:
            moved = os.stat(landed, dir_fd=trash, follow_symlinks=False)
        except Exception as e:                        # it moved: whatever fails now, where it went is reported
            return Trashed(where, trashed_to, f"FERB moved {where} to the Trash at {trashed_to} but couldn't confirm "
                                              f"it was what it checked ({e}); it stays there for you to look at")
        if (moved.st_dev, moved.st_ino) != (checked.st_dev, checked.st_ino):
            return Trashed(where, trashed_to, f"{where} changed while FERB moved it: what is in the Trash at "
                                              f"{trashed_to} isn't what FERB checked, so FERB left it there for you "
                                              "to look at and moved nothing out")
        return Trashed(where, trashed_to, device=moved.st_dev, inode=moved.st_ino)
    finally:
        os.close(trash)


def _move_into(folder: int, name: str, trash: int, bin_where: Path,
               closed: Callable[[Path, FileAccess], str | None] | None) -> str:
    """Moves `name` into the open Trash under a name nothing there has (the item's own name first, as Finder does) and
    returns it. Each name is checked in the Trash as opened (rule 10: a Trash inside a protected folder gets nothing).
    Raises PathRefused, or OSError when it can't move."""
    stem, suffix = os.path.splitext(name)
    for attempt in range(100):
        candidate = name if attempt == 0 else f"{stem} {secrets.token_hex(4)}{suffix}"
        if closed is not None:
            refusal = closed(bin_where / candidate, "write")
            if refusal:
                raise PathRefused(refusal)
        try:
            move_excl(folder, name, trash, candidate)
            return candidate
        except FileExistsError:
            continue
    raise FileExistsError(errno.EEXIST, f"no free name for {name} in the Trash")


CAPABILITIES = [FsList(), FsRead(), FsWrite(), FsEdit(), FsTrash()]


def normalize_path(path: str, workspace: str) -> str:
    """One form for policy comparisons: the same place `resolve` uses (owner_path), normalized."""
    return os.path.normpath(str(owner_path(path, workspace)))
