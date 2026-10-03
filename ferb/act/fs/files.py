"""File list/read/write/edit/trash using normal OS access. Relative paths resolve in the workspace.
Read provenance is recorded; it does not remove capabilities."""

import os
from pathlib import Path

from pydantic import BaseModel, Field

from ferb.core.types import ActionSpec, Identity, RiskLevel, Target, ToolContext, ToolResult

READ_DEFAULT, READ_MAX, WRITE_MAX, LIST_MAX = 256 * 1024, 2 * 1024 * 1024, 5 * 1024 * 1024, 500

class PathRefused(Exception):
    pass


HOME_DIRS = ("Desktop", "Documents", "Downloads")


def owner_path(path: str, workspace: str) -> Path:
    """Where a path the owner or a plan wrote points: ~ expanded; "Desktop/…", "Documents/…", "Downloads/…" are the
    owner's own folders (owner's report 2026-10-01: Desktop/clg/uni landed in FERB's workspace); other relative paths
    are in the workspace. A name Finder shows with "/" is stored with ":" ("clg/uni" is clg:uni on disk): when the path
    doesn't exist as written, the name that does is used."""
    p = Path(path).expanduser()
    if not p.is_absolute():
        first = p.parts[0] if p.parts else ""
        home = next((h for h in HOME_DIRS if first.lower() == h.lower()), None)
        p = (Path.home() / home / Path(*p.parts[1:])) if home else Path(workspace) / p
    return finder_name(p)


def finder_name(p: Path) -> Path:
    """`~/Desktop/clg/uni` -> `~/Desktop/clg:uni` when only the colon form exists (macOS stores Finder's "/" as ":").
    Tries every split, so a plain `clg` folder next to `clg:uni` doesn't hide it; as written if nothing exists."""
    if p.exists():
        return p
    parts = list(p.parts)

    def walk(cur: Path, i: int) -> Path | None:
        if i == len(parts):
            return cur
        for k in range(1, min(5, len(parts) - i + 1)):           # this name alone, or joined with the next ones
            nxt = cur / ":".join(parts[i:i + k])
            if nxt.exists():
                found = walk(nxt, i + k)
                if found is not None:
                    return found
        return None
    return walk(Path(parts[0]), 1) or p


def resolve(path: str, ctx: ToolContext, must_exist: bool) -> Path:
    raw = owner_path(path, ctx.workspace)                        # relative: the workspace, or the owner's Desktop…
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

    def describe_path(self, args: BaseModel) -> str:
        return str(Path(str(getattr(args, "path"))).expanduser())

    def locks(self, args: BaseModel) -> list[str]:
        """One file at a time per path (section 7 step 7); the path as given (relative ones are the workspace's)."""
        return [f"file:{self.describe_path(args)}"]


class FsList(_Base):
    name, description = "fs.list", "List a folder's entries (names, kind, size)."
    scopes, risk, reversible = {"fs:read"}, RiskLevel.LOW, True

    class Input(BaseModel):
        path: str = Field(description="Folder path (absolute, ~, or relative to FERB's workspace)")

    def describe(self, args: BaseModel) -> ActionSpec:
        return ActionSpec("read", Target(path=self.describe_path(args)))

    async def run(self, args: BaseModel, ctx: ToolContext) -> ToolResult:
        try:
            p = resolve(str(getattr(args, "path")), ctx, must_exist=True)
        except PathRefused as e:
            return _err(str(e))
        if not p.is_dir():
            return _err(f"{p} is not a folder")
        entries = []
        for child in sorted(p.iterdir())[:LIST_MAX]:
            try:
                st = child.lstat()
                entries.append({"name": child.name, "kind": "dir" if child.is_dir() else "file",
                                "size": st.st_size})
            except OSError:
                continue
        return ToolResult({"path": str(p), "entries": entries}, read_sources=(_source(p, ctx),))


class FsRead(_Base):
    name, description = "fs.read", "Read a text file (UTF-8), optionally a byte range."
    scopes, risk, reversible = {"fs:read"}, RiskLevel.LOW, True

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
        with open(p, "rb") as f:
            f.seek(int(getattr(args, "offset")))
            data = f.read(int(getattr(args, "max_bytes")) + 1)
        truncated = len(data) > int(getattr(args, "max_bytes"))
        text = data[:int(getattr(args, "max_bytes"))].decode("utf-8", errors="replace")
        return ToolResult({"path": str(p), "text": text, "truncated": truncated, "size": p.stat().st_size},
                          read_sources=(_source(p, ctx),))


class FsWrite(_Base):
    name, description = "fs.write", "Create or overwrite a text file."
    scopes, risk, reversible = {"fs:write"}, RiskLevel.MEDIUM, False

    class Input(BaseModel):
        path: str
        content: str = Field(max_length=WRITE_MAX)
        overwrite: bool = False

    def describe(self, args: BaseModel) -> ActionSpec:
        return ActionSpec("write_file", Target(path=self.describe_path(args)), content=str(getattr(args, "content")))

    async def run(self, args: BaseModel, ctx: ToolContext) -> ToolResult:
        try:
            p = resolve(str(getattr(args, "path")), ctx, must_exist=False)
        except PathRefused as e:
            return _err(str(e))
        if p.exists() and not getattr(args, "overwrite"):
            return _err(f"{p} exists (set overwrite=true to replace it)")
        p.parent.mkdir(parents=True, exist_ok=True)
        tmp = p.with_name(f".{p.name}.ferb-tmp")
        tmp.write_text(str(getattr(args, "content")), encoding="utf-8")
        os.replace(tmp, p)                                        # atomic
        return ToolResult({"path": str(p), "bytes": p.stat().st_size})


class FsEdit(_Base):
    name, description = "fs.edit", "Replace one exact occurrence of old_text with new_text in a file."
    scopes, risk, reversible = {"fs:write"}, RiskLevel.MEDIUM, False

    class Input(BaseModel):
        path: str
        old_text: str = Field(min_length=1)
        new_text: str

    def describe(self, args: BaseModel) -> ActionSpec:
        return ActionSpec("write_file", Target(path=self.describe_path(args)), content=str(getattr(args, "new_text")))

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
        tmp = p.with_name(f".{p.name}.ferb-tmp")
        tmp.write_text(text.replace(old, str(getattr(args, "new_text")), 1), encoding="utf-8")
        os.replace(tmp, p)
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
        from Foundation import NSURL, NSFileManager
        ok, result_url, err = NSFileManager.defaultManager().trashItemAtURL_resultingItemURL_error_(
            NSURL.fileURLWithPath_(str(p)), None, None)
        if not ok:
            return _err(f"couldn't move to Trash: {err}")
        return ToolResult({"path": str(p), "trashed_to": str(result_url.path()) if result_url else None})


CAPABILITIES = [FsList(), FsRead(), FsWrite(), FsEdit(), FsTrash()]


def normalize_path(path: str, workspace: str) -> str:
    """One form for policy comparisons: the same place `resolve` uses (owner_path), normalized."""
    return os.path.normpath(str(owner_path(path, workspace)))
