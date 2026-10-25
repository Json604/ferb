"""Shared support for the shell-sandbox tests: a throwaway keychain, and running a command under a profile.

`security create-keychain` adds the new keychain to the user's search list; `throwaway_keychain` restores the user
search list exactly as it found it, even when the test fails, and deletes the keychain. Nothing names the owner's own
keychains."""
import re
import secrets
import subprocess
from collections.abc import Iterator
from contextlib import contextmanager
from pathlib import Path
from typing import Any

SECRET = "s3cret-value"
SERVICE = "ferb-sandbox-test"


def _user_search_list() -> list[str]:
    out = subprocess.run(["security", "list-keychains", "-d", "user"], capture_output=True, text=True).stdout
    return re.findall(r'"([^"]+)"', out)


@contextmanager
def throwaway_keychain(folder: Path) -> Iterator[Path]:
    """A keychain file in `folder` holding one throwaway item (SERVICE, SECRET)."""
    kc = folder / "scratch.keychain-db"
    before = _user_search_list()
    pw = secrets.token_hex(8)
    try:
        subprocess.run(["security", "create-keychain", "-p", pw, str(kc)], check=True, capture_output=True)
        subprocess.run(["security", "unlock-keychain", "-p", pw, str(kc)], check=True, capture_output=True)
        subprocess.run(["security", "add-generic-password", "-a", "a", "-s", SERVICE, "-w", SECRET, str(kc)],
                       check=True, capture_output=True)
        yield kc
    finally:
        subprocess.run(["security", "delete-keychain", str(kc)], capture_output=True)
        if _user_search_list() != before:
            subprocess.run(["security", "list-keychains", "-d", "user", "-s", *before], capture_output=True)
    assert _user_search_list() == before, "the user's keychain search list was not restored"


def run_under(sandbox: Any, argv: list[str], *, apps: bool = False, timeout: float = 20) -> subprocess.CompletedProcess[str]:
    """argv under the sandbox's profile (the shell's, or mac.applescript's with apps=True); the profile is removed."""
    wrapped = sandbox.wrap(argv, apps=apps)
    try:
        return subprocess.run(wrapped, capture_output=True, text=True, timeout=timeout)
    finally:
        sandbox.cleanup(sandbox.profile_path(wrapped))


# bootstrap_look_up of each named mach service; prints "<name> <kern_return>" (0 = reachable). No app is addressed.
LOOKUP = """
import ctypes, sys
libc = ctypes.CDLL("/usr/lib/libSystem.B.dylib")
libc.bootstrap_look_up.argtypes = [ctypes.c_uint, ctypes.c_char_p, ctypes.POINTER(ctypes.c_uint)]
bp = ctypes.c_uint.in_dll(libc, "bootstrap_port").value
for name in sys.argv[1:]:
    port = ctypes.c_uint(0)
    print(name, libc.bootstrap_look_up(bp, name.encode(), ctypes.byref(port)))
"""


def lookups(sandbox: Any, names: list[str], *, apps: bool = False) -> dict[str, int]:
    import sys
    argv = [sys.executable, "-c", LOOKUP, *names]
    if not apps:
        r = run_under(sandbox, argv)
    else:   # mac.applescript's profile runs only osascript (D31 round 2, N7): this one probe needs its interpreter, so
        # it keeps every other line of that profile and drops only the exec rules
        from ferb.act.shell.sandbox import SANDBOX_EXEC
        text = "\n".join(l for l in sandbox.profile(apps=True).splitlines() if "process-exec" not in l) + "\n"
        path = sandbox._write(text)
        try:
            r = subprocess.run([SANDBOX_EXEC, "-f", str(path), *argv], capture_output=True, text=True, timeout=20)
        finally:
            sandbox.cleanup(path)
    return {n: int(rc) for n, rc in (line.split() for line in r.stdout.splitlines() if line.strip())}
