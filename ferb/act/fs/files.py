"""File capabilities (section 3 act/fs): list / read / write / edit / trash inside allowed roots.

Roots: FERB's workspace always; the owner's folders only when listed in config ([fs] owner_roots), and only for
OWNER tasks. Symlinks are resolved and must stay inside a root; some paths are refused even inside one.
Reads from untrusted roots (default: ~/Downloads) are reported so the gateway marks the worker exposed.
"""

import os
from pathlib import Path

from pydantic import BaseModel, Field

from ferb.core.types import ActionSpec, Identity, RiskLevel, Target, ToolContext, ToolResult

READ_DEFAULT, READ_MAX, WRITE_MAX, LIST_MAX = 256 * 1024, 2 * 1024 * 1024, 5 * 1024 * 1024, 500

PROTECTED = [Path.home() / ".ssh", Path.home() / ".gnupg", Path.home() / ".aws", Path.home() / "Library" / "Keychains",
             Path.home() / ".codex", Path.home() / ".claude", Path.home() / ".config" / "gh"]


class PathRefused(Exception):
    pass


def _roots(ctx: ToolContext) -> list[Path]:
    roots = [Path(ctx.workspace)]
    if ctx.identity is Identity.OWNER:
        roots += [Path(r).expanduser() for r in ctx.owner_roots]
    return [r.resolve() for r in roots]


def resolve(path: str, ctx: ToolContext, must_exist: bool) -> Path:
    raw = Path(path).expanduser()
    if not raw.is_absolute():
        raw = Path(ctx.workspace) / raw                          # relative paths are relative to the workspace
    if any(part == ".." for part in raw.parts):
        raise PathRefused("'..' is not allowed in paths")
    p = raw.resolve() if (must_exist or raw.exists()) else raw.parent.resolve() / raw.name
    if must_exist and not p.exists():
        raise PathRefused(f"{path} does not exist")
    inside = [r for r in _roots(ctx) if p == r or r in p.parents]
    if not inside:
        raise PathRefused(f"{path} is outside the allowed folders")
    ferb_home = Path(ctx.workspace).resolve().parent
    if (ferb_home in p.parents or p == ferb_home) and not (Path(ctx.workspace).resolve() in p.parents
                                                           or p == Path(ctx.workspace).resolve()):
        raise PathRefused("FERB's own files are off limits (except its workspace)")
    for bad in PROTECTED:
        b = bad.resolve() if bad.exists() else bad
        if p == b or b in p.parents:
            raise PathRefused(f"{path} is a protected location")
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
        if p in _roots(ctx):
            return _err("refusing to trash an allowed root itself")
        from Foundation import NSURL, NSFileManager
        ok, result_url, err = NSFileManager.defaultManager().trashItemAtURL_resultingItemURL_error_(
            NSURL.fileURLWithPath_(str(p)), None, None)
        if not ok:
            return _err(f"couldn't move to Trash: {err}")
        return ToolResult({"path": str(p), "trashed_to": str(result_url.path()) if result_url else None})


CAPABILITIES = [FsList(), FsRead(), FsWrite(), FsEdit(), FsTrash()]


def normalize_path(path: str, workspace: str) -> str:
    """One form for policy comparisons: ~ expanded, relative paths under the workspace, normalized."""
    p = Path(path).expanduser()
    if not p.is_absolute():
        p = Path(workspace) / p
    return os.path.normpath(str(p))
