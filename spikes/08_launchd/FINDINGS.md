# Spike 8 — launchd user agent + socket from the CLI

Status: done (2026-09-28). 6/6 tests pass, 5 runs in a row. macOS 26.6.2, Python 3.12 (Homebrew) in a venv.

Run: `uv run --python 3.12 --no-project --with pytest pytest -q test_spike.py` — installs `com.ferb.spike8`
into `~/Library/LaunchAgents` for the run and removes it at the end (agent unloaded, plist gone).
Leaves `~/.ferb/spike8/` (venv + copies of `daemon.py`, `probes.py`, logs) for reruns.

## Proven

- `launchctl bootstrap gui/<uid> <plist>` starts the daemon (`RunAtLoad`); the CLI reaches it over
  `~/.ferb/run/spike8.sock`; the process answering is the one launchd reports (`launchctl print` pid).
- Socket directory `0700`, socket `0600` (bound under `umask 0177`), owned by the user. The daemon reads the
  peer's uid with `getsockopt(SOL_LOCAL, LOCAL_PEERCRED)` and refuses other users (the CLI role's proof).
- `kill -9` → launchd restarts it (`KeepAlive = {SuccessfulExit: false}`, `ThrottleInterval = 1`), new pid.
- `launchctl bootout` stops and unloads it; the CLI then fails with "daemon not running".
- A stale socket file (crash leftover) doesn't block start: the daemon connects to it first and only replaces it
  if nobody answers; if someone does, it refuses to start (no second daemon).

## Found

- **The CLI must treat EOF / reset as "daemon unavailable"**: right after a kill, `connect` can succeed against the
  dying listener and the read returns EOF. (First run failed on this with a JSON error.)
- **TCC for the launchd-started process** (same machine, same moment as spike 6's terminal: all granted there):
  Accessibility **denied**, Screen Recording **denied**, Full Disk Access **denied**, Automation→Chrome **not
  asked**. Grants attach to the process launchd runs — here the venv's `python` (the Homebrew Python binary) —
  not to FERB and not to the terminal. `ferb setup` must get the owner to grant *that* binary, and `ferb doctor`
  must ask the daemon, not check from the CLI's process.
  Consequence: any Python upgrade or venv rebuild that changes the binary may need grants again → prefer a
  stable, signed executable for the daemon (FERB.app is deferred; an alternative is needed for Block 1).
- **Running the daemon from the repo under `~/Desktop` worked here** (unlike spike 4's browser-launched native
  host). Likely because TCC is decided by the responsible process and this Python binary already has folder
  access on this Mac — not verified (would need reading the TCC database). Don't rely on it: install to `~/.ferb`.

## Not done here

- Actually granting permissions to the daemon's binary and re-checking (needs the owner in System Settings).
- Login/logout and sleep/wake behaviour of the agent.
