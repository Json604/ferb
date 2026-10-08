"""Resolve literal paths and perform paged reads, atomic writes, edits and trash. Must not substitute files or cap deliverables.

File list/read/write/edit/trash using normal OS access. Relative paths resolve in the workspace.
Read provenance is recorded; it does not remove capabilities."""

import os
import tempfile
from datetime import datetime, timezone
from pathlib import Path

from pydantic import BaseModel, Field

from ferb.core.types import ActionSpec, Identity, RiskLevel, Target, ToolContext, ToolResult

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


def atomic_text(p: Path, text: str) -> None:
    """Exclusive sibling temp preserves atomic replacement without touching a pre-existing scratch file."""
    fd, name = tempfile.mkstemp(prefix=f".{p.name}.", suffix=".ferb-tmp", dir=p.parent)
    temp = Path(name)
    try:
        with os.fdopen(fd, "w", encoding="utf-8") as stream:
            stream.write(text)
        os.replace(temp, p)
    finally:
        temp.unlink(missing_ok=True)


def resolve(path: str, ctx: ToolContext, must_exist: bool) -> Path:
    raw = owner_path(path, ctx.workspace)
    p = raw.resolve() if (must_exist or raw.exists()) else raw.parent.resolve() / raw.name
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

    def describe_path(self, args: BaseModel) -> str:
        return str(Path(str(getattr(args, "path"))).expanduser())

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
        children = sorted(p.iterdir())
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
        p.parent.mkdir(parents=True, exist_ok=True)
        atomic_text(p, str(getattr(args, "content")))
        return ToolResult({**identity(p), "bytes": p.stat().st_size})


class FsEdit(_Base):
    name, description = "fs.edit", "Replace one exact occurrence of old_text with new_text in a file."
    scopes, risk, reversible = {"fs:write"}, RiskLevel.MEDIUM, False

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
        text = p.read_text(encoding="utf-8")
        old = str(getattr(args, "old_text"))
        n = text.count(old)
        if n != 1:
            return _err(f"old_text occurs {n} times; it must occur exactly once")
        atomic_text(p, text.replace(old, str(getattr(args, "new_text")), 1))
        return ToolResult({"path": str(p), "replaced": 1}, read_sources=(_source(p, ctx),))


class FsTrash(_Base):
    name, description = "fs.trash", "Move a file or folder to the Trash (recoverable)."
    scopes, risk, reversible = {"fs:write"}, RiskLevel.LOW, True

    class Input(BaseModel):
        path: str

    def describe(self, args: BaseModel) -> ActionSpec:
        return ActionSpec("trash", Target(path=self.describe_path(args)))

    async def run(self, args: BaseModel, ctx: ToolContext) -> ToolResult:
        try:
            p = resolve(str(getattr(args, "path")), ctx, must_exist=True)
        except PathRefused as e:
            return _err(str(e))
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
