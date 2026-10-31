"""Encode exact app identifiers and execute agent-supplied AppleScript. Must not choose app targets or rewrite script intent.

AppleScript target encoding and execution. Setup offers Automation; macOS enforces actual grants.
Script content is not classified by permission keywords. osascript runs under FERB's shell sandbox (D27, rule 10):
`do shell script` would otherwise be a shell outside it. Measured (F5a Task 2): Apple Events still reach apps under the
profile, and `do shell script "cat <denied file>"` is refused, also inside a `tell` block. What the named app does when
told to is that app's, outside any sandbox of FERB's: Automation grants bound it. Apple Events reach only the app the
call names (D31 review round 1): a script addressed to Notes can't tell Terminal or System Events anything; a call that
names a terminal app or System Events asks the owner first, showing the script (tools.MacAppleScript)."""

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
    deadline = asyncio.get_running_loop().time() + timeout
    if timeout <= 0:
        return False, "timed out before execution"
    script = wrap(bundle_id, body)
    argv = ["/usr/bin/osascript", "-e", script]
    env = None
    if sandbox is not None:
        from ferb.act.shell.manager import clean_env
        # Building the profile reads sockets and runs sandbox_check: off the event loop. The shell's clean environment
        argv, env = await asyncio.to_thread(sandbox.wrap, argv, apps=True, events_to=bundle_id), clean_env(sandbox)
    try:
        if asyncio.get_running_loop().time() >= deadline:
            return False, "timed out before execution"
        proc = None
        spawning = asyncio.create_task(asyncio.create_subprocess_exec(
            *argv, stdout=asyncio.subprocess.PIPE, stderr=asyncio.subprocess.PIPE, cwd=cwd, env=env))
        try:
            proc = await asyncio.shield(spawning)
            out, err = await asyncio.wait_for(proc.communicate(), max(0, deadline - asyncio.get_running_loop().time()))
        except asyncio.CancelledError:
            # Retain ownership if Stop arrives while the OS is still handing back the process handle.
            if proc is None:
                try:
                    proc = await spawning
                except OSError:
                    pass
            if proc is not None:
                await _kill_and_drain(proc)
            raise
        except asyncio.TimeoutError:
            assert proc is not None
            await _kill_and_drain(proc)
            return False, "timed out"
    finally:
        if sandbox is not None:
            sandbox.cleanup(sandbox.profile_path(argv))
    return proc.returncode == 0, (out if proc.returncode == 0 else err).decode(errors="replace")


async def _kill_and_drain(proc: asyncio.subprocess.Process) -> None:
    if proc.returncode is None:
        try:
            proc.kill()
        except ProcessLookupError:
            pass
    await proc.communicate()
