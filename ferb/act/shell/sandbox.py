"""Own the macOS sandbox profile FERB's shell and AppleScript run under (S2, SQ3 revised, SQ5, D27, D31). Must not
interpret the command: the profile is built from the protection lists only, and the same profile runs every command.

Seatbelt (`sandbox-exec`) profiles, as measured on this Mac (F5a Task 2 report, E1-E7 and the carried experiments):
- Everything is allowed except what the lists deny, and the last matching rule wins (E1): FERB's own home is denied in
  full, the workspace allowed back after it, and upload staging denied again after that.
- Rules match the path the kernel reaches, so links are followed and a rule must name the resolved path (a rule on
  /tmp/x never matches; /private/tmp/x does). Entries are emitted as resolved at FERB's start, as written, and as
  written resolved now. The workspace allow is the literal folder only: an allow must never widen. On the Mac's
  case-insensitive volume a rule also matches every case and Unicode-normalization spelling, even of a path that
  doesn't exist yet (measured).
- A never-read entry is denied for reads and writes (a path nobody may read is a path nobody may write); a never-write
  entry for writes. Hardlinking a denied file is refused too, so no new name for a protected file can be made; a
  hardlink made before (outside FERB) is a different path and is not covered.
- Renaming a folder above a protected path would carry it to a path no rule names, so each such folder, and the
  workspace itself, is denied as a literal (the folder, not its contents): files can still be made inside them.
- The Keychain's service (com.apple.SecurityServer) is unreachable, so `security` fails at once and no Keychain prompt
  can appear (E4). Trust evaluation goes through trustd and still works (URLSession, Go: measured), but programs that
  list root certificates through the Keychain service (rustls-native-certs, e.g. Codex CLI) find none, so the shell's
  environment names Apple's certificate bundle (SSL_CERT_FILE) for them.
- No network except this Mac's own addresses (E2), and of those only FERB's proxy and servers started inside FERB's
  sandbox (F5a Task 3): every port another program listens on when the command starts is denied (listeners.py), so
  the proxy, which records every destination, is the only way off the Mac. Seatbelt's "localhost" means any of the
  Mac's own addresses (its LAN address too): `(allow network* (local ip "localhost:*"))` lets every connection out
  (measured), so outbound is allowed only by remote address, and listening only by local address. A port-specific
  deny is ignored when an unfiltered `(deny network-outbound)` (or `network*`) comes before it (measured), so
  outbound is denied by filters instead: every remote IP, every unix socket and every kernel control, each as
  Task 2's `(deny network*)` refused them. The daemon's socket is named again last (a file rule on its folder
  doesn't stop a connect: measured, E3).
- Inside it, nothing can apply another sandbox (any deny rule makes `sandbox_apply` fail: E5), and setuid programs
  such as sudo cannot start (E6).
- The shell can't start launchd jobs, open apps or URLs, or send Apple Events to other apps: `job-creation`,
  `lsopen`, the LaunchServices lookups and the Apple Events service are denied (measured). mac.applescript's profile
  keeps Apple Events and launches and every other rule.
- FERB's home and upload staging may be looked at (metadata), never listed or read: tools walk up from the
  workspace (git, node, a POSIX `cd`, venv failed without it: measured).

An approved escape lifts exactly one listed path rule for one run (`lift`); FERB's own state and the network rule are
never lifted, and `admin` (sudo) is no profile change at all, since sudo can't start under any profile (Task 6 runs
that one command outside it)."""

import os
import secrets
import subprocess
from pathlib import Path
from typing import Callable

from ferb.act.shell.listeners import outside_ports
from ferb.policy.protection import ADMIN, FERB_STATE, NETWORK, Entry, Protection

SANDBOX_EXEC = "/usr/bin/sandbox-exec"
SECURITY_SERVER = "com.apple.SecurityServer"
CERT_BUNDLE = "/etc/ssl/cert.pem"            # Apple's root certificates as a file (macOS ships it)
# What a program that starts its own macOS sandbox meets inside FERB's (E5); shown to agents, never enforced on them
NESTED_SANDBOX = ("A program can't start its own macOS sandbox inside FERB's shell (FERB's sandbox applies to it). "
                  "Codex CLI needs its sandbox off: --sandbox danger-full-access (measured working), or "
                  "--dangerously-bypass-approvals-and-sandbox only when the owner asked for no approvals.")
# What the shell profile keeps out beyond files and network (measured, F5a Task 2 fix round 1); shown to agents
NO_APPS = ("FERB's shell can't open apps or URLs on the owner's screen, send Apple Events to other apps, or start "
           "background jobs (launchd); FERB's browser and Mac tools (mac.applescript included) do that.")
# What the network rules and FERB's proxy mean for a command (F5a Task 3, measured); shown to agents
NETWORK_FACT = ("Network goes only through FERB's proxy (HTTPS_PROXY/HTTP_PROXY are set; programs that ignore them, "
                "and ssh, get no network); each host reached is recorded and labels the output. On this Mac, only "
                "servers started in FERB's shell are reachable, and Apple-framework clients must address them as "
                "127.0.0.1, not localhost; other programs' servers are out of reach.")
# System services that would reach the network for a contained process, outside its network rules and FERB's proxy:
# background URL sessions (nsurlsessiond: a download from inside reached a server the sandbox denies, measured F5a
# Task 3) and mDNSResponder's resolver service (the proxy resolves names; the sandbox has no DNS). Denied to both
# profiles (AppleScriptObjC can make a URL session too)
NETWORK_SERVICES = ("com.apple.nsurlsessiond", "com.apple.nsurlsessiond-launchondemand", "com.apple.dnssd.service")
# Denied to the shell profile only (mac.applescript's keeps them): app lookup and the Apple Events service
APP_SERVICES = ("com.apple.coreservices.launchservicesd", "com.apple.lsd.mapdb", "com.apple.coreservices.appleevents")


def _q(path: Path) -> str:
    """A path as an SBPL string literal."""
    return '"' + str(path).replace("\\", "\\\\").replace('"', '\\"') + '"'


def _forms(path: Path) -> list[Path]:
    """A path as written and as the kernel reaches it (rules match the resolved path)."""
    return list(dict.fromkeys((path, Path(os.path.realpath(path)))))


class Sandbox:
    def __init__(self, protection: Protection, profiles_dir: Path,
                 proxy_port: Callable[[], int | None] = lambda: None, *, socket: Path | None = None,
                 listeners: Callable[[], set[int] | None] | None = None) -> None:
        self.protection, self.profiles_dir, self.proxy_port, self.socket = protection, profiles_dir, proxy_port, socket
        # Ports other programs listen on, read when each command starts (None: unknown, so every local server is out)
        self.listeners = listeners or (lambda: outside_ports(protection.ferb_home, protection.workspace))

    def profile(self, lift: str | None = None, *, apps: bool = False) -> str:
        """The shell's profile; `apps=True` is mac.applescript's: the same file, network and Keychain rules, but Apple
        Events and app launches stay possible (`tell application` may launch the app)."""
        p = self.protection
        if lift is not None:
            if lift in (FERB_STATE, NETWORK):
                raise ValueError(f"{lift} is never lifted")
            if lift == ADMIN:
                raise ValueError("admin is no profile change: sudo can't start under any sandbox profile")
            if p.entry(lift) is None:
                raise ValueError(f"no such rule: {lift}")
        lines = ["(version 1)", "(allow default)"]
        lines += [f"(deny file-read* file-write* (subpath {_q(f)}))" for f in _forms(p.ferb_home)]
        # The workspace exactly as FERB made it, never where a link there points: an allow must not widen
        lines.append(f"(allow file-read* file-write* (subpath {_q(p.workspace)}))")
        # Tools walk up from the workspace (git, node's realpath, a POSIX `cd`, venv): FERB's home may be looked at,
        # never listed or read; so may upload staging, from the workspace root (`ls -l`), its contents stay closed
        lines += [f"(allow file-read-metadata (literal {_q(f)}))" for f in _forms(p.ferb_home)]
        lines += [f"(deny file-read* file-write* (subpath {_q(f)}))" for f in _forms(p.uploads)]
        lines += [f"(allow file-read-metadata (literal {_q(f)}))" for f in _forms(p.uploads)]
        lines += [f"(deny file-read* file-write* (subpath {_q(f)}))" for f in self._paths(p.reads, lift)]
        lines += [f"(deny file-write* (subpath {_q(f)}))" for f in self._paths(p.writes, lift)]
        # An existing folder is denied as a literal. A missing one may still be made (a first install creates
        # ~/.cargo), but once made it can't be removed or renamed, and it can't be made as a link
        guarded = list(dict.fromkeys((*p.guarded_parents(), p.workspace)))
        present = [f for g in guarded if os.path.isdir(g) for f in _forms(g)]
        missing = [f for g in guarded if not os.path.isdir(g) for f in _forms(g)]
        lines += [f"(deny file-write* (literal {_q(f)}))" for f in dict.fromkeys(present)]
        for f in dict.fromkeys(missing):
            lines += [f"(deny file-write-unlink (literal {_q(f)}))",
                      f"(deny file-write-create (require-all (literal {_q(f)}) (vnode-type SYMLINK)))"]
        lines += [f'(deny mach-lookup (global-name "{SECURITY_SERVER}"))', "(deny job-creation)"]
        lines += [f'(deny mach-lookup (global-name "{name}"))' for name in NETWORK_SERVICES]
        # The kernel's SMB and NFS clients run outside the sandbox: mounting a share makes kernel_task reach the
        # server, past the network rules and FERB's proxy (measured, F5a Task 3 fix 1). Deny mounting and the SMB
        # device node, in both profiles
        lines += ["(deny file-mount)", '(deny file-read* file-write* file-ioctl (regex #"^/dev/nsmb"))']
        lines += self._network()
        if not apps:
            lines.append("(deny lsopen)")
            lines += [f'(deny mach-lookup (global-name "{name}"))' for name in APP_SERVICES]
        if self.socket is not None:
            lines += [f"(deny network-outbound (remote unix-socket (path-literal {_q(s)})))" for s in _forms(self.socket)]
        return "\n".join(lines) + "\n"

    def _network(self) -> list[str]:
        """Listening on this Mac's addresses; connecting only to them, minus every port another program listens on
        now; FERB's proxy last, so nothing above can close it."""
        proxy, outside = self.proxy_port(), self.listeners()
        lines = ["(deny network-inbound)", "(deny network-bind)",
                 '(allow network-bind network-inbound (local ip "localhost:*"))',
                 '(deny network-outbound (remote ip "*:*"))',
                 "(deny network-outbound (remote unix-socket))",
                 '(deny network-outbound (control-name-prefix ""))',
                 '(allow network-outbound (remote ip "localhost:*"))']
        if outside is None:
            lines.append('(deny network-outbound (remote ip "localhost:*"))')
        else:
            lines += [f'(deny network-outbound (remote ip "localhost:{port}"))' for port in sorted(outside - {proxy})]
        if proxy is not None:
            lines.append(f'(allow network-outbound (remote tcp "localhost:{proxy}"))')
        return lines

    @staticmethod
    def _paths(entries: list[Entry], lift: str | None) -> list[Path]:
        """Each entry as resolved at FERB's start, as written, and as written resolved now (it may be a link since)."""
        return list(dict.fromkeys(f for e in entries if e.id != lift for f in (e.path, *_forms(e.typed))))

    def wrap(self, argv: list[str], lift: str | None = None, *, apps: bool = False) -> list[str]:
        """argv under the profile. The profile file stays until `cleanup` (after the process ends): sandbox-exec reads
        it after it starts, so removing it as soon as Popen returns failed every run (measured)."""
        text = self.profile(lift, apps=apps)
        self.profiles_dir.mkdir(parents=True, exist_ok=True, mode=0o700)
        path = self.profiles_dir / f"{secrets.token_hex(8)}.sb"
        fd = os.open(path, os.O_CREAT | os.O_EXCL | os.O_WRONLY, 0o600)
        with os.fdopen(fd, "w") as f:
            f.write(text)
        return [SANDBOX_EXEC, "-f", str(path), *argv]

    def argv(self, command: str, lift: str | None = None) -> list[str]:
        return self.wrap(["/bin/sh", "-c", command], lift)

    @staticmethod
    def profile_path(argv: list[str]) -> Path | None:
        """The profile file a wrapped argv uses."""
        return Path(argv[2]) if argv[:2] == [SANDBOX_EXEC, "-f"] else None

    @staticmethod
    def cleanup(path: Path | None) -> None:
        if path is not None:
            path.unlink(missing_ok=True)

    def sweep(self) -> None:
        """Profiles left by a daemon that stopped while its commands ran (at start, before any command)."""
        for old in self.profiles_dir.glob("*.sb"):
            old.unlink(missing_ok=True)

    @staticmethod
    def env() -> dict[str, str]:
        """For the contained process's environment: root certificates as a file (see the module docstring)."""
        return {"SSL_CERT_FILE": CERT_BUNDLE} if os.path.isfile(CERT_BUNDLE) else {}

    def available(self) -> tuple[bool, str]:
        """Whether a command can run contained: the binary exists and this profile compiles and runs `exit 0`."""
        if not os.path.exists(SANDBOX_EXEC):
            return False, f"{SANDBOX_EXEC} is missing"
        try:
            argv = self.argv("exit 0")
        except (OSError, ValueError) as e:
            return False, f"the profile couldn't be written: {e}"
        try:
            r = subprocess.run(argv, capture_output=True, text=True, timeout=10)
        except (OSError, subprocess.SubprocessError) as e:
            return False, f"sandbox-exec couldn't run: {e}"
        finally:
            self.cleanup(self.profile_path(argv))
        if r.returncode != 0:
            return False, f"sandbox-exec failed (exit {r.returncode}): {r.stderr.strip()[:300]}"
        return True, "ok"
