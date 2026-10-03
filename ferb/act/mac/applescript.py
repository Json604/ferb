"""AppleScript target encoding and execution. Setup offers Automation; macOS enforces actual grants.
Script content is not classified by permission keywords."""

import asyncio
import re

from ferb.core.errors import Refused

MAX_SCRIPT = 4000


def check(body: str) -> None:
    if len(body) > MAX_SCRIPT:
        raise Refused("AppleScript too long")


def wrap(bundle_id: str, body: str) -> str:
    if not re.fullmatch(r"[A-Za-z0-9.\-]+", bundle_id):
        raise Refused("bad app id")
    check(body)
    return f'tell application id "{bundle_id}"\n{body}\nend tell'


async def run(bundle_id: str, body: str, timeout: float = 30.0) -> tuple[bool, str]:
    script = wrap(bundle_id, body)
    proc = await asyncio.create_subprocess_exec("osascript", "-e", script, stdout=asyncio.subprocess.PIPE,
                                                stderr=asyncio.subprocess.PIPE)
    try:
        out, err = await asyncio.wait_for(proc.communicate(), timeout)
    except asyncio.TimeoutError:
        proc.kill()
        return False, "timed out"
    return proc.returncode == 0, (out if proc.returncode == 0 else err).decode(errors="replace").strip()[:4000]
