"""Command pre-checks for the shell (section 3 act/shell guard.py), used before policy sees the call:
- the working folder must be inside the allowed roots (FERB's workspace; the owner's listed folders for OWNER tasks),
- commands that touch FERB's own files, the Keychain or protected locations are refused outright,
- destructive commands (rm -rf, git push --force, sudo, disk erase, ...) are classified as permanent deletion, so
  policy asks the owner every time (hard floor)."""

import re
from pathlib import Path

from ferb.act.fs.files import PROTECTED, PathRefused, resolve
from ferb.core.errors import Refused
from ferb.core.types import ToolContext
from ferb.policy.rules import Kind, destructive_shell

REFUSED = [
    (re.compile(r"\bsecurity\s+(find|dump|export|delete|add|unlock|set)-", re.I), "Keychain access"),
    (re.compile(r"(^|[\s'\"=/])~?/?\.ferb(/|\b)|FERB_HOME|ferb\.db", re.I), "FERB's own files"),
    (re.compile(r"\bosascript\b", re.I), "AppleScript (use the mac tools)"),
    (re.compile(r"\b(launchctl)\b", re.I), "changing login items / agents"),
]


def check(command: str, cwd: str | None, ctx: ToolContext) -> Path:
    for pattern, why in REFUSED:
        if pattern.search(command):
            raise Refused(f"refused: {why} isn't allowed from the shell")
    for p in PROTECTED:
        for form in (str(p), "~/" + str(p.relative_to(Path.home())) if p.is_relative_to(Path.home()) else str(p)):
            if form in command:
                raise Refused(f"refused: {form} is a protected location")
    try:
        return resolve(cwd or ctx.workspace, ctx, must_exist=True)
    except PathRefused as e:
        raise Refused(f"working folder refused: {e}") from None


# Commands that bring content in from the network: their output is untrusted (section 10.2), so the worker that
# runs them is exposed (loses shell, AppleScript and file-write for the rest of its run). Checked on the command
# word of every pipeline segment (so `grep http README` isn't a fetch), plus a few argument patterns.
NET_COMMANDS = {"curl", "wget", "http", "https", "xh", "aria2c", "lynx", "w3m", "links", "elinks", "nc", "ncat",
                "netcat", "telnet", "ssh", "scp", "sftp", "ftp", "yt-dlp", "youtube-dl"}
PREFIXES = {"env", "sudo", "nohup", "time", "xargs", "command", "exec", "nice"}
SEGMENT = re.compile(r"\|\||&&|[;|&\n]|\$\(|`|\(")
NET_ARGS = re.compile(r"\bgit\s+(clone|fetch|pull|ls-remote)\b|\brsync\b[^;&|]*\s\S+:"
                      r"|\b(python3?|node|ruby|perl)\b[^;&|]*(urllib|requests|http\.client|fetch\(|net/http|LWP|socket)",
                      re.I)


def fetches_network(command: str) -> bool:
    if NET_ARGS.search(command):
        return True
    for seg in SEGMENT.split(command):
        words = [w for w in seg.split() if "=" not in w or w.startswith("-")]
        while words and (words[0] in PREFIXES or words[0].startswith("-")):
            words = words[1:]
        if words and words[0].rsplit("/", 1)[-1].lower() in NET_COMMANDS:
            return True
    return False


def kind(command: str) -> str:
    return Kind.DELETE_PERMANENT if destructive_shell(command) else Kind.RUN_SHELL
