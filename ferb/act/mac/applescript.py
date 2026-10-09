"""Encode exact app identifiers and execute agent-supplied AppleScript. Must not choose app targets or rewrite script intent.

AppleScript target encoding and execution. Setup offers Automation; macOS enforces actual grants.
Script content is not classified by permission keywords. osascript runs under FERB's shell sandbox (D27, rule 10):
`do shell script` would otherwise be a shell outside it. Measured (F5a Task 2): Apple Events still reach apps under the
profile, and `do shell script "cat <denied file>"` is refused, also inside a `tell` block. What another app does when
told to (Finder, Terminal) is that app's, outside any sandbox of FERB's: Automation grants bound it."""

import asyncio
import re
from typing import Any

from ferb.core.errors import Refused


def wrap(bundle_id: str, body: str) -> str:
    if not re.fullmatch(r"[A-Za-z0-9.\-]+", bundle_id):
        raise Refused("bad app id")
    return f'tell application id "{bundle_id}"\n{body}\nend tell'


async def run(bundle_id: str, body: str, timeout: float = 30.0, sandbox: Any = None,
              cwd: str | None = None) -> tuple[bool, str]:
    """`sandbox` (ferb/act/shell/sandbox.py's Sandbox) contains osascript with its AppleScript profile (Apple Events
    and app launches kept); the tool never runs a script without one. `cwd` is FERB's workspace."""
    script = wrap(bundle_id, body)
    argv = ["/usr/bin/osascript", "-e", script]
    if sandbox is not None:
        argv = sandbox.wrap(argv, apps=True)
    try:
        proc = await asyncio.create_subprocess_exec(*argv, stdout=asyncio.subprocess.PIPE,
                                                    stderr=asyncio.subprocess.PIPE, cwd=cwd)
        try:
            out, err = await asyncio.wait_for(proc.communicate(), timeout)
        except asyncio.TimeoutError:
            proc.kill()
            await proc.wait()
            return False, "timed out"
    finally:
        if sandbox is not None:
            sandbox.cleanup(sandbox.profile_path(argv))
    return proc.returncode == 0, (out if proc.returncode == 0 else err).decode(errors="replace")
