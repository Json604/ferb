"""P24 metadata-only disk usage: never opens a regular file, follows a symlink, or returns its contents."""
import asyncio
import os
import stat
from pathlib import Path
from pydantic import BaseModel
from ferb.core.types import ActionSpec, FileAccess, Identity, RiskLevel, Target, ToolContext, ToolResult
from ferb.act.fs.files import real_path, opened_path
from ferb.core.errors import Refused


def usage(path: Path, closed=None, details: bool = False) -> dict:
    files = directories = bytes_ = allocated = 0
    errors: list[str] = []
    paths: list[str] = []
    def count(folder: int) -> None:
        nonlocal files, directories, bytes_, allocated
        where = opened_path(folder)
        if closed and closed(where, 'metadata'):
            errors.append(str(where) + ': protected state omitted')
            return
        directories += 1
        if details:
            paths.append(str(where) + '/')
        for name in os.listdir(folder):
            try:
                item = os.stat(name, dir_fd=folder, follow_symlinks=False)
                if stat.S_ISDIR(item.st_mode):
                    child = os.open(name, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW, dir_fd=folder)
                    try:
                        count(child)
                    finally:
                        os.close(child)
                else:
                    if details:
                        paths.append(str(where / name))
                    files += 1
                    bytes_ += item.st_size
                    allocated += item.st_blocks * 512
            except OSError as e:
                errors.append(f'{where / name}: {type(e).__name__}')
    info = path.lstat()
    if stat.S_ISDIR(info.st_mode):
        fd = os.open(path, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
        try:
            count(fd)
        finally:
            os.close(fd)
    else:
        files, bytes_, allocated = 1, info.st_size, info.st_blocks * 512
        if details:
            paths.append(str(path))
    return {'path': str(path), 'files': files, 'directories': directories, 'bytes': bytes_, 'allocated_bytes': allocated,
            'complete': not errors, 'errors': errors, 'symlinks': 'count link metadata; never follow', **({'paths': paths} if details else {})}


class FsDiskUsage:
    name = 'fs.disk_usage'
    description = 'Count files and logical/allocated bytes under an exact path, using metadata only; no contents or symlink traversal.'
    scopes = {'fs:read'}
    identities = {Identity.OWNER, Identity.FERB}
    observation, reversible, risk = True, True, RiskLevel.LOW
    class Input(BaseModel):
        path: str
    def file_paths(self, args) -> list[tuple[str, FileAccess]]:
        return [(args.path, 'metadata')]
    def describe(self, args) -> ActionSpec:
        return ActionSpec('read', Target(path=args.path))
    async def run(self, args, ctx: ToolContext) -> ToolResult:
        path = real_path(args.path, ctx.workspace)
        if ctx.path_closed and (why := ctx.path_closed(path, 'metadata')):
            raise Refused(why)
        result = await asyncio.to_thread(usage, path, ctx.path_closed)
        return ToolResult(result, read_sources=(('file', str(path)),))
