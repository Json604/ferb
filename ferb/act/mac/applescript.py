"""AppleScript for one app at a time (section 3 act/mac). FERB wraps the model's statements in its own
`tell application id "<bundle>"` block and refuses scripts that reach past that app: nested `tell`s, System Events
(keystrokes into any app), shell or other scripts. Automation permission is asked by macOS on first use per app.
"""

import asyncio
import re

from ferb.core.errors import Refused

FORBIDDEN = [
    (re.compile(r"\bdo\s+shell\s+script\b", re.I), "shell commands (use the shell tool)"),
    (re.compile(r"\b(run|load|store)\s+script\b", re.I), "running other scripts"),
    (re.compile(r"\btell\b", re.I), "addressing other apps"),
    (re.compile(r"\bSystem Events\b", re.I), "System Events (it reaches every app)"),
    (re.compile(r"\busing\s+terms\s+from\b", re.I), "other apps' terms"),
    (re.compile(r"\bosascript\b|\bcurl\b", re.I), "shell tools"),
    (re.compile(r"«", re.I), "raw Apple event codes"),
    (re.compile(r"\b(activate|reopen)\b", re.I), "bringing the app to the front (FERB works with apps in the "
                                                  "background; the owner stays in their own app)"),
]
MAX_SCRIPT = 4000


def check(body: str) -> None:
    if len(body) > MAX_SCRIPT:
        raise Refused("AppleScript too long")
    for pattern, why in FORBIDDEN:
        if pattern.search(body):
            raise Refused(f"AppleScript refused: {why} isn't allowed")


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
