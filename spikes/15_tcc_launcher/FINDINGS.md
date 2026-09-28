# Spike 15 — TCC grant on a stable launcher vs the interpreter (build step 3 measurement)

Status: done (2026-09-28). macOS 26.6.2. Manual steps by the owner (grant in System Settings).

Setup: `ferb-launcher` (tiny C program, ad-hoc signed, `~/.ferb/spike15/`) is run by a temporary LaunchAgent, so
the launcher — not the terminal — is the responsible process; it `posix_spawn`s a Python probe that reports
`AXIsProcessTrusted()` / `CGPreflightScreenCaptureAccess()` via `ctypes`. Harness: `measure.py build|run <python>`.

| # | Situation | Accessibility seen by the Python child |
|---|-----------|----------------------------------------|
| 0 | No grant (baseline), interpreter A and B | denied |
| 1 | Launcher granted, interpreter A (Homebrew 3.12.14) | **granted** — the grant covers the spawned child |
| 2 | Same launcher, interpreter B (python.org 3.12.8, different path) | **granted** — survives an interpreter swap |
| 3 | Launcher rebuilt from identical source | granted — deterministic build, same CDHash |
| 3b | Launcher rebuilt with a one-line change (new CDHash) | **denied** — any change to the launcher needs a re-grant |

## Conclusion

- Launcher: grants survive interpreter upgrades, but TCC only checks the launcher's signature, so **whatever
  program sits at the path it spawns inherits the grant** — a tampered/replaced interpreter keeps Accessibility.
- Interpreter granted directly (ARCHITECTURE section 8 step 6): TCC checks the interpreter's own signature, so
  tampering voids the grant, and the grant covers nothing else. Costs a re-grant when FERB upgrades its
  interpreter; `ferb doctor` detects that.
- Recommendation adopted: keep section 8 step 6 (dedicated interpreter, granted directly).

Cleanup: owner removes `ferb-launcher` from the Accessibility list; then `~/.ferb/spike15` is deleted.
