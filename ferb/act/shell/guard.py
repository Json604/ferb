"""Shell working-directory validation and network-source annotations; no command deny list or permission gate."""

import re
from pathlib import Path

from ferb.act.fs.files import PathRefused, resolve
from ferb.core.errors import Refused
from ferb.core.types import ToolContext
from ferb.policy.rules import Kind

def check(command: str, cwd: str | None, ctx: ToolContext) -> Path:
    try:
        folder = resolve(cwd or ctx.workspace, ctx, must_exist=True)
        if not folder.is_dir():
            raise Refused(f"working folder is not a directory: {folder}")
        return folder
    except PathRefused as e:
        raise Refused(f"working folder refused: {e}") from None


# Commands that bring content in from the network: their output is untrusted (section 10.2), recorded as provenance. A worker that
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
    return Kind.RUN_SHELL
