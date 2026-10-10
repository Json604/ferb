"""Validate shell working directories. Must not interpret commands as intent or authorization.

Shell working-directory validation; no command deny list or permission gate. What a command reached over the network
is a fact from FERB's egress proxy (proxy.py), never a guess from its words."""

from pathlib import Path

from ferb.act.fs.files import PathRefused, resolve
from ferb.core.errors import Refused
from ferb.core.types import ToolContext

def working_directory(cwd: str | None, ctx: ToolContext) -> Path:
    try:
        folder = resolve(cwd or ctx.workspace, ctx, must_exist=True)
        if not folder.is_dir():
            raise Refused(f"working folder is not a directory: {folder}")
        return folder
    except PathRefused as e:
        raise Refused(f"working folder refused: {e}") from None
