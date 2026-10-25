# Packaging Phase A: Measure, Shrink, Two Architectures — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** FERB.app holds only what FERB runs, and `build-dmg.sh` makes `FERB-AppleSilicon.dmg` and `FERB-Intel.dmg`, each from a one-architecture app.

**Architecture:** One sourced helper, `scripts/app-parts.sh`, names each architecture's parts, trims what FERB never runs, thins universal Mach-O files to the app's one architecture, checks that, and prints a size table. `build-app.sh` takes `--arch arm64|x86_64` and fetches that architecture's CPython, Codex package and Swift/clang output (an Intel app is built on Apple silicon under Rosetta). The daemon picks Codex's binary by `platform.machine()`. The lock gets an Intel-only cryptography pin, because cryptography 49+ ships no Intel Mac wheels.

**Tech Stack:** zsh build scripts, uv (standalone CPython 3.12.13, uv.lock), npm (`package-lock.json`, Codex 0.157.1), SwiftPM / swiftc / clang, `lipo`, `hdiutil`, pytest.

**Spec:** `docs/superpowers/specs/2026-10-24-ferb-for-everyone-design.md` (L34), section "Phase A — Measure and shrink, two architectures".

## Global Constraints

- Owner decisions (spec, 2026-10-24): "Free items only; no Developer ID or notarization for now." "Intel support as a separate download: two DMGs." The release page offers `FERB-AppleSilicon.dmg` and `FERB-Intel.dmg`.
- "The size target is set from the measurement, with the owner." Task 1 ends with the owner's answer; nothing before it removes anything.
- "Intel is tested on an Intel Mac or Rosetta-free VM." Apple silicon can't run an Intel macOS VM, so this needs real Intel hardware (owner question A4).
- macOS floor stays `LSMinimumSystemVersion` 14.2 (`ui/mac/FERBNotch/Info.plist`, `ui/mac/FERBAgent/Info.plist`, `macapp/FERB/Info.plist`); Swift targets `-mmacosx-version-min=14.2` / `-target <arch>-apple-macos14.2`.
- Pins stay exactly as they are: CPython `3.12.13` (`scripts/build-app.sh` `CPYTHON`), Codex `0.157.1` (`package.json`, `ferb/runtimes/codex_exec.py` `CODEX_VERSION`), every other version in `uv.lock`. The only lock change is Task 4's Intel-only cryptography line.
- Bare minimum, correct behaviour first (owner, 2026-10-25: "focus on correct working").
- Work on `main`; commit by path (other agents have uncommitted work in this checkout); check `git diff --cached` before each commit; commit messages cite L34 and end with `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.
- Never install, quit or replace the running notch (`ui/mac/FERBNotch/build/FERB Notch.app`) or the owner's daemon, and never touch `~/.ferb`. Verification builds go to `/private/tmp/ferb-pkg/<arch>` and run with `nice -n 10`. Installs happen only on the second macOS user account in the owner's live check.
- After each task: run the focused tests, then `uv run pytest -q`, `uv run mypy ferb/core` and `git diff --check` (CLAUDE.md "Development").

## Review Focus

- **A stale cache mixes architectures** (one OUT_DIR reused for arm64 then x86_64, or a universal wheel left fat): the app must refuse to finish, naming each file built for another architecture. Pinned by Task 5's `test_arch_check_names_every_file_built_for_another_architecture` and by `ferb_check_arch` running in every build.
- **An Intel build on a Mac without Rosetta, or an unknown `--arch`**: a clear refusal before anything is written. Pinned by Task 5's `test_build_app_refuses_an_unknown_architecture_and_intel_without_rosetta`.
- **A later dependency bump drops Intel Mac wheels again** (as cryptography 49 did): the offline suite fails, naming the package, instead of the Intel build failing on release day. Pinned by Task 4's `test_every_locked_runtime_package_installs_on_both_architectures`.
- **The wrong image for the wrong Mac** (an Intel app packed as `FERB-AppleSilicon.dmg`, or a universal app packed at all): the name comes from the app's own architecture, and a fat app is refused. Pinned by Task 6's `test_dmg_is_named_for_the_apps_architecture_and_says_how_to_open_it_the_first_time`.
- **The trimmed Codex fails at runtime** (it needed `codex-code-mode-host`, `voice` or `zsh` after all): every build runs `codex debug models --bundled` from the trimmed copy (Task 3); the real-bundle test repeats it (Task 3); a real Codex task runs in the owner's live check (Task 7).

---

## Measurement (2026-10-25, before any change)

A release build of `9ebe9e6` (plus other agents' uncommitted edits, so `9ebe9e6-dirty`), FERB 0.27.0.dev1, build 202610251155, arm64, ad-hoc signed. It was built with `nice -n 10 scripts/build-app.sh <scratch>/pkg-measure/out` in 37 s, read-only to the running notch and `~/.ferb`. Sizes are file bytes / 10^6 (`du -sh` says 669M, in MiB).

| Part | MB | Share |
|---|---:|---:|
| **Whole FERB.app** | **682.3** | 100% |
| Codex native binary `Resources/node/node_modules/@openai/codex-darwin-arm64/vendor/aarch64-apple-darwin/bin/codex` (OpenAI's Developer ID signature, kept as is) | 238.2 | 34.9% |
| Claude Code CLI `site-packages/claude_agent_sdk/_bundled/claude` (Anthropic's Developer ID signature, kept as is) | 225.0 | 33.0% |
| Python packages without Claude Code (site-packages; 23.9 of it is the `.pyc` bytecode build-app.sh compiles) | 83.5 | 12.2% |
| Codex `bin/codex-code-mode-host` | 65.3 | 9.6% |
| Python standard library (10.7 of it `.pyc`) | 21.3 | 3.1% |
| Codex `codex-resources/voice` (GStreamer voice host and dylibs) | 20.6 | 3.0% |
| `libpython3.12.dylib` | 17.9 | 2.6% |
| Codex `codex-path/rg` (ripgrep) | 4.0 | 0.6% |
| Notch app `Contents/MacOS/FERBNotch` | 3.6 | 0.5% |
| Codex `codex-resources/zsh` | 0.8 | 0.1% |
| Helpers: agent `FERB.app`, `FERB Voice.app`, `ferb-secrets` | 0.4 | 0.1% |
| Everything else (Python `bin`, notices, plists, Codex package JSON) | 1.7 | 0.2% |

The largest Python packages: cryptography 12.4, pyobjc-core 10.5 (7.8 of it the `objc/*.dSYM` debug symbols), pydantic-core 4.5, ferb 4.0, wcwidth 3.1, tiktoken 3.0, pyobjc Cocoa 2.9, pyobjc Quartz 2.1. Universal (arm64 + x86_64) extension modules: 31 files carrying 6 MB of x86_64 code in the arm64 app.

**Download today:** the disk image (`hdiutil` ULMO) is 197.5 MB.

**Node:** FERB already starts Codex's native binary directly, not through node (`ferb/runtimes/codex_exec.py:211-218`: "The native binary, not the node wrapper: launchd has no node on PATH"). The bundle ships no node runtime, only the platform package's folder, so there is no node to remove. The `node_modules/...` layout stays because `~/.ferb/node` links to it (`ferb/device/install.py:225-246`) and a checkout's npm install uses the same layout.

**Intel (measured from the registry, not built):** Codex's `darwin-x64` 0.157.1 package unpacks to 352.2 MB: `bin/codex` 256.9, `bin/codex-code-mode-host` 69.3, voice 20.8, rg 4.4, zsh 0.8. The x86_64 `claude_agent_sdk` wheel is 98.1 MB compressed (arm64: 93.7), so its CLI is about 235 MB. cryptography 50.0.1 has **no** Intel Mac wheel: `uv pip install --dry-run --python-platform x86_64-apple-darwin --only-binary :all: -r <export>` fails on it. Its last release with Intel Mac wheels is 48.0.1, and with that pin all 51 runtime packages resolve to x86_64/universal2 wheels.

## Size proposal for the owner (Task 1 asks this)

What FERB never runs, removed by Task 3. The savings are arm64 measurements:

| Removed | MB | Why it's safe |
|---|---:|---|
| Codex `bin/codex-code-mode-host` | 65.3 | FERB passes `--disable code_mode_host` on every Codex start (`codex_exec.py:53,286-287`, `codex_stream.py:199-200`) |
| Codex `codex-resources/voice` | 20.6 | FERB never starts Codex voice. Its dylibs can't load anyway: `signing.sh` re-signs every `.dylib`, and OpenAI's hardened, library-validated voice host refuses another signer's library |
| Codex `codex-resources/zsh` | 0.8 | FERB disables Codex's shell tools (`shell_tool`, `unified_exec`, `shell_snapshot` in `DISABLED`) |
| `objc/*.dSYM` debug symbols | 7.8 | debugger data, never loaded |
| site-packages `*/tests`, `jsonschema/benchmarks`, `claude_agent_sdk/testing` | 1.3 | test code no runtime import reaches (checked by grep: only `jsonschema/benchmarks` imports `jsonschema.tests`, and both go) |
| stdlib `lib2to3`, `pydoc_data`, `config-3.12-darwin` (a `python.o` and Makefile), `turtle.py`, with their bytecode | 2.5 | not imported by FERB or its dependencies (grep) |
| x86_64 slices of universal extension modules (arm64 app; arm64 slices in the Intel app) | 6.0 | each app runs one architecture |
| **Total** | **101.7 measured** | arm64 app **580.6 MB**: this plan's `ferb_trim_app` run on a copy of the measured build. The trimmed Codex still printed its catalog (`codex debug models --bundled`: 11 models) and `ferb_check_arch` passed. Disk image ≈ 170 MB (a trimmed copy without thinning measured 584.0 MB / 171.7 MB) |

Kept on purpose: Codex's binary (the default runtime) and `rg` (4 MB, possibly used by Codex's own file search; not proven unused); the `.pyc` bytecode (FERB never writes into its signed bundle, so without it every start would compile every module in memory); `*.dist-info` (licenses and `importlib.metadata`); `PyObjCTools/TestSupport.py` (not a tests folder).

The one large optional cut is the **owner's call (A1)**: the bundled Claude Code CLI, 225 MB. Without it the arm64 app measured 359.0 MB (353 after thinning) and its disk image 94.9 MB. But then "Use Claude" in setup only works if the user installs Claude Code separately, and FERB would run whatever `claude` version it finds (the SDK falls back to `shutil.which("claude")`, `subprocess_cli.py:255-300`) instead of a pinned one. That needs its own design. Recommendation: keep it.

Proposed size target (A2): **arm64 ≤ 600 MB, Intel ≤ 630 MB** with Claude Code kept (580.6 MB measured / ≈610 MB estimated; downloads ≈170 / ≈185 MB). If the owner drops Claude Code: arm64 ≤ 370 MB, Intel ≤ 390 MB. Task 3 enforces the chosen numbers in the real-bundle test.

## Owner questions (asked at the end of Task 1; record the answers, quoted, in Task 7)

- **A1 — Claude Code inside FERB.app?** Keep the bundled, pinned Claude Code CLI (+225 MB app, about +77 MB download) so "Use Claude" works with nothing else to install? Recommended: keep. If "drop": stop after Task 1; dropping it needs its own design (how Claude users get a pinned CLI).
- **A2 — Size target:** arm64 ≤ 600 MB and Intel ≤ 630 MB (downloads ≈170 / ≈185 MB), or other numbers?
- **A3 — cryptography on Intel:** cryptography 49+ ships no Intel Mac wheels. Either (a) the Intel app uses cryptography **48.0.1**, the last release with them (FERB uses only AES-GCM, HKDF and SHA-256 from it, `ferb/identity/seal.py`; mcp's `pyjwt[crypto]` needs it too), while Apple silicon keeps 50.0.1; or (b) build 50.0.1 from source for every Intel release, which needs a Rust and OpenSSL x86_64 toolchain on the release Mac. Recommended: (a). If (b): stop at Task 4; it needs its own plan.
- **A4 — Testing Intel:** your Mac is an Apple M4, and Rosetta runs x86_64 code but isn't a real Intel test (Apple silicon can't run an Intel macOS VM). Is there a real Intel Mac with macOS 14.2 or later to test on? If not, choose: (a) publish `FERB-Intel.dmg` after Rosetta checks only, labelled "not yet tested on an Intel Mac"; or (b) hold the Intel download until it is tested.

## Interfaces this phase produces

- `scripts/build-app.sh [--arch arm64|x86_64] [OUT_DIR]` builds `OUT_DIR/FERB.app` (default `build/app-<arch>`) for one architecture (default: the build Mac's). `--sign-only APP` is unchanged.
- `scripts/build-dmg.sh` with no arguments builds both apps into `$FERB_DMG_DIR/app-<arch>` and writes `$FERB_DMG_DIR/FERB-AppleSilicon.dmg` and `$FERB_DMG_DIR/FERB-Intel.dmg` (`FERB_DMG_DIR` defaults to `build/`). `scripts/build-dmg.sh APP [DMG]` packs one app; the default name comes from its architecture.
- `scripts/app-parts.sh` (sourced, zsh) provides:
  - `ferb_arch_names ARCH`, which sets `FERB_UV_ARCH`, `FERB_NPM_CPU`, `FERB_CODEX_NPM`, `FERB_CODEX_TRIPLE` and `FERB_CODEX_BIN` (relative to `Contents/`);
  - `ferb_trim_app APP ARCH`;
  - `ferb_check_arch APP ARCH`;
  - `ferb_size_report APP`.
- `ferb.runtimes.codex_exec.native_bin(machine: str | None = None) -> Path` gives the Codex binary inside a node folder. `_NATIVE_BIN = native_bin()` stays as the name the rest of FERB uses.
- Phase B (signing) and Phase G (updates) sign and publish whatever these scripts produce; nothing here changes `scripts/signing.sh`.

## Files

- Create `scripts/app-parts.sh`: the parts of FERB.app per architecture (names, trim, thin, check, size).
- Create `tests/support/macho.py`: tiny real Mach-O files per architecture, for packaging tests.
- Modify `scripts/build-app.sh`: `--arch`, per-architecture Python/Swift/clang/Codex, trim + check + size report.
- Modify `scripts/build-dmg.sh`: two images, names by architecture, the first-open instruction file.
- Modify `macapp/FERB/build.sh`: build for `$ARCH` (default: this Mac's).
- Modify `ferb/runtimes/codex_exec.py`: `native_bin()` per architecture.
- Modify `ferb/device/install.py`: comment only (`BUNDLE_NODE`).
- Modify `pyproject.toml` and `uv.lock`: the Intel-only cryptography pin.
- Modify `tests/test_packaging.py`, `tests/support/app_bundle.py` and `tests/test_setup_flow.py`.
- Modify docs: `README.md`, `ARCHITECTURE.md`, `CHANGELOG.md`, `docs/OWNER_ACCEPTANCE.md`, `docs/0.27_DECISIONS.toml`.

---

### Task 1: Size report, and the owner's size target

**Files:**
- Create: `scripts/app-parts.sh`
- Create: `tests/support/macho.py`
- Modify: `scripts/build-app.sh:15` (source the helper), `scripts/build-app.sh:152` (print the report)
- Test: `tests/test_packaging.py`

**Interfaces:**
- Produces: `ferb_size_report APP` prints `FERB.app (<lipo archs of MacOS/FERBNotch>): <total> MB`, then one line per part, `%8.1f MB  <name>`, largest first. The part names are exactly `Codex (Resources/node)`, `Claude Code (claude_agent_sdk/_bundled)`, `Python packages (site-packages without Claude Code)`, `Python standard library`, `libpython`, `Notch app (MacOS)`, `Helpers (agent, voice, secrets)` and `Everything else`.
- Produces: `tests/support/macho.py` `compile_macho(dest: Path, *archs: str) -> Path`.

- [ ] **Step 1: Write the test support module**

Create `tests/support/macho.py`:

```python
"""Tiny real Mach-O executables for packaging tests: one per architecture, or one universal file holding several.
Built with the Xcode clang; nothing here signs or installs anything."""

import shutil
import subprocess
from pathlib import Path

import pytest

SOURCE = "int main(void) { return 0; }\n"


def clang() -> list[str]:
    if shutil.which("clang"):
        return ["clang"]
    if shutil.which("xcrun"):
        return ["xcrun", "clang"]
    pytest.skip("needs clang (Xcode or the Command Line Tools)")


def compile_macho(dest: Path, *archs: str) -> Path:
    """`dest`, an executable built for `archs` (two or more: a universal file); replaces whatever is there."""
    dest.parent.mkdir(parents=True, exist_ok=True)
    src = dest.with_name(dest.name + ".c")
    src.write_text(SOURCE)
    flags = [flag for arch in archs for flag in ("-arch", arch)]
    subprocess.run([*clang(), *flags, "-o", str(dest), str(src)], check=True, capture_output=True)
    src.unlink()
    return dest
```

- [ ] **Step 2: Write the failing test**

In `tests/test_packaging.py`, add `import shlex` to the imports, `from tests.support.macho import compile_macho` after `from ferb.device.install import APP_BUNDLE_ID`, and `APP_PARTS = ROOT / "scripts" / "app-parts.sh"` after `BUILD_DMG`. Add `ROOT / "scripts" / "app-parts.sh"` to the tuple in `test_build_scripts_are_executable_zsh_that_parses` (it is sourced, so it stays out of the `os.access` line). Then add, after that test:

```python
def _parts(code: str) -> subprocess.CompletedProcess[str]:
    """zsh code run after sourcing scripts/app-parts.sh, in a clean zsh with the system PATH."""
    return subprocess.run(["/bin/zsh", "-f", "-c", f"source {shlex.quote(str(APP_PARTS))}; {code}"],
                          capture_output=True, text=True, env={"PATH": "/usr/bin:/bin:/usr/sbin:/sbin"})


def _sized(path: Path, size: int) -> Path:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(b"\0" * size)
    return path


def test_size_report_lists_ferb_apps_parts_largest_first(tmp_path: Path) -> None:
    app = tmp_path / "FERB.app"
    c = app / "Contents"
    site = c / "Resources/python/lib/python3.12/site-packages"
    _sized(c / "Resources/node/node_modules/@openai/codex-darwin-arm64/vendor/aarch64-apple-darwin/bin/codex", 3_000_000)
    _sized(site / "claude_agent_sdk/_bundled/claude", 2_000_000)
    _sized(site / "claude_agent_sdk/__init__.py", 100_000)
    _sized(site / "regex/_regex.cpython-312-darwin.so", 400_000)
    _sized(c / "Resources/python/lib/python3.12/json/__init__.py", 300_000)
    _sized(c / "Resources/python/lib/libpython3.12.dylib", 700_000)
    _sized(c / "Helpers/ferb-secrets", 100_000)
    _sized(c / "Info.plist", 1_000)
    notch = compile_macho(c / "MacOS/FERBNotch", "arm64").stat().st_size
    done = _parts(f"ferb_size_report {shlex.quote(str(app))}")
    assert done.returncode == 0, done.stderr
    lines = done.stdout.splitlines()
    total = 3_000_000 + 2_000_000 + 100_000 + 400_000 + 300_000 + 700_000 + 100_000 + 1_000 + notch
    assert lines[0] == f"FERB.app (arm64): {total / 1e6:.1f} MB"
    rows = [line.split(" MB  ", 1) for line in lines[1:]]
    named = {name: float(mb) for mb, name in rows}
    assert named == {"Codex (Resources/node)": 3.0, "Claude Code (claude_agent_sdk/_bundled)": 2.0,
                     "Python packages (site-packages without Claude Code)": 0.5, "Python standard library": 0.3,
                     "libpython": 0.7, "Helpers (agent, voice, secrets)": 0.1,
                     "Notch app (MacOS)": round(notch / 1e6, 1), "Everything else": 0.0}
    sizes = [float(mb) for mb, _ in rows]
    assert sizes == sorted(sizes, reverse=True)
```

- [ ] **Step 3: Run the test to verify it fails**

Run: `uv run pytest tests/test_packaging.py -k "size_report or zsh_that_parses" -v`
Expected: both FAIL, because `scripts/app-parts.sh` doesn't exist.

- [ ] **Step 4: Write `scripts/app-parts.sh`**

```zsh
# FERB.app's parts per architecture (L34), sourced by build-app.sh (zsh). Nothing here builds, signs or downloads.

# _ferb_bytes PATH…: the total size in bytes of the regular files under each PATH (0 when there are none)
_ferb_bytes() {
  local p
  for p in "$@"; do
    if [[ -e $p ]]; then find "$p" -type f -print0; fi
  done | xargs -0 stat -f %z | awk '{ s += $1 } END { printf "%.0f\n", s }'
}

# ferb_size_report APP: FERB.app's size by part in MB (10^6 bytes), the largest first (L34's measurement)
ferb_size_report() {
  local app=${1:A}
  local c="$app/Contents" py="$app/Contents/Resources/python"
  local std="$py/lib/python3.12" site="$py/lib/python3.12/site-packages"
  local total codex claude site_all std_all libpy notch helpers arch
  total=$(_ferb_bytes "$app")
  codex=$(_ferb_bytes "$c/Resources/node")
  claude=$(_ferb_bytes "$site/claude_agent_sdk/_bundled")
  site_all=$(_ferb_bytes "$site")
  std_all=$(_ferb_bytes "$std")
  libpy=$(_ferb_bytes "$py/lib/libpython3.12.dylib")
  notch=$(_ferb_bytes "$c/MacOS")
  helpers=$(_ferb_bytes "$c/Helpers")
  arch=$(lipo -archs "$c/MacOS/FERBNotch" 2>/dev/null || print unknown)
  local -a rows
  rows=("$codex|Codex (Resources/node)"
        "$claude|Claude Code (claude_agent_sdk/_bundled)"
        "$(( site_all - claude ))|Python packages (site-packages without Claude Code)"
        "$(( std_all - site_all ))|Python standard library"
        "$libpy|libpython"
        "$notch|Notch app (MacOS)"
        "$helpers|Helpers (agent, voice, secrets)"
        "$(( total - codex - std_all - libpy - notch - helpers ))|Everything else")
  printf 'FERB.app (%s): %.1f MB\n' "$arch" $(( total / 1e6 ))
  local bytes name
  print -l -- $rows | sort -t'|' -k1,1 -rn | while IFS='|' read -r bytes name; do
    printf '%8.1f MB  %s\n' $(( bytes / 1e6 )) "$name"
  done
}
```

- [ ] **Step 5: Print the report from every build**

In `scripts/build-app.sh`, after line 15 (`source "$ROOT/scripts/signing.sh"`), add:

```zsh
source "$ROOT/scripts/app-parts.sh"
```

After the last line (`print "Built: $APP ..."`), add:

```zsh
ferb_size_report "$APP"
```

- [ ] **Step 6: Run the tests to verify they pass**

Run: `uv run pytest tests/test_packaging.py -k "size_report or zsh_that_parses" -v`
Expected: PASS.

- [ ] **Step 7: Full checks and commit**

Run: `uv run pytest -q && uv run mypy ferb/core && git diff --check`
Expected: all pass.

```bash
git add scripts/app-parts.sh tests/support/macho.py scripts/build-app.sh tests/test_packaging.py
git diff --cached --stat
git commit -m "feat(L34): FERB.app's size by part after every build (packaging A, task 1)

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

- [ ] **Step 8: Owner checkpoint (no code)**

Show the owner the plan's "Measurement" table and "Size proposal" (copy them, don't summarize), then ask questions A1–A4 exactly as written under "Owner questions". Record the answers word for word under a new heading `## Owner answers (Task 1)` at the end of this plan file and commit only that file:

```bash
git add docs/superpowers/plans/2026-10-25-packaging-a-shrink-two-arch.md
git commit -m "docs(L34): the owner's packaging A answers (size target, Claude Code, Intel)

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

If A1 is "drop" or A3 is "(b)", stop and report to the controller; those need a new plan. With A1 "keep" and A3 "(a)", continue.

---

### Task 2: Codex's binary by architecture

**Files:**
- Modify: `ferb/runtimes/codex_exec.py:23-31` (import), `:52` (`_NATIVE_BIN`)
- Modify: `ferb/device/install.py:42` (comment)
- Modify: `tests/support/app_bundle.py:10`
- Modify: `tests/test_setup_flow.py:577, 592-593`
- Test: `tests/test_packaging.py`

**Interfaces:**
- Produces: `native_bin(machine: str | None = None) -> Path`, which returns `node_modules/@openai/codex-darwin-arm64/vendor/aarch64-apple-darwin/bin/codex` for `"arm64"` and `node_modules/@openai/codex-darwin-x64/vendor/x86_64-apple-darwin/bin/codex` for `"x86_64"`; `None` means `platform.machine()`. `_NATIVE_BIN = native_bin()`.

- [ ] **Step 1: Write the failing test**

Add to `tests/test_packaging.py`:

```python
def test_codex_binary_is_the_pinned_package_for_each_architecture() -> None:
    """The daemon starts the Codex binary of its own architecture (an Intel FERB.app's Python reports x86_64, also
    under Rosetta), and package-lock.json pins that architecture's package at FERB's Codex version."""
    import json
    import platform
    from ferb.runtimes.codex_exec import CODEX_VERSION, _NATIVE_BIN, native_bin
    assert native_bin("arm64") == Path("node_modules/@openai/codex-darwin-arm64/vendor/aarch64-apple-darwin/bin/codex")
    assert native_bin("x86_64") == Path("node_modules/@openai/codex-darwin-x64/vendor/x86_64-apple-darwin/bin/codex")
    assert _NATIVE_BIN == native_bin(platform.machine())
    lock = json.loads((ROOT / "package-lock.json").read_text())["packages"]
    for cpu in ("arm64", "x64"):
        entry = lock[f"node_modules/@openai/codex-darwin-{cpu}"]
        assert entry["version"] == f"{CODEX_VERSION}-darwin-{cpu}" and entry["integrity"].startswith("sha512-")
```

- [ ] **Step 2: Run it to verify it fails**

Run: `uv run pytest tests/test_packaging.py::test_codex_binary_is_the_pinned_package_for_each_architecture -v`
Expected: FAIL with `ImportError: cannot import name 'native_bin'`.

- [ ] **Step 3: Implement `native_bin`**

In `ferb/runtimes/codex_exec.py`, add `import platform` after `import os` (line 25). Replace line 52:

```python
_NATIVE_BIN = Path("node_modules/@openai/codex-darwin-arm64/vendor/aarch64-apple-darwin/bin/codex")
```

with:

```python
# Codex's npm platform package per Mac architecture (package-lock.json pins both): its folder and its binary's target
# triple. scripts/app-parts.sh ferb_arch_names names the same binary for FERB.app.
_CODEX_PACKAGES = {"arm64": ("codex-darwin-arm64", "aarch64-apple-darwin"),
                   "x86_64": ("codex-darwin-x64", "x86_64-apple-darwin")}


def native_bin(machine: str | None = None) -> Path:
    """The native Codex binary inside a node folder, for `machine` or this process's architecture (arm64, or x86_64
    on an Intel Mac and in an Intel FERB.app under Rosetta). Any other name is no Mac FERB runs on: it gets the arm64
    path, which is then simply not installed."""
    package, triple = _CODEX_PACKAGES.get(machine or platform.machine(), _CODEX_PACKAGES["arm64"])
    return Path(f"node_modules/@openai/{package}/vendor/{triple}/bin/codex")


_NATIVE_BIN = native_bin()
```

In `ferb/device/install.py:42`, change the comment to:

```python
BUNDLE_NODE = Path("Contents/Resources/node")                 # node_modules/@openai/codex-darwin-<arm64|x64>
```

In `tests/support/app_bundle.py:10`, replace the line with:

```python
from ferb.runtimes.codex_exec import _NATIVE_BIN as CODEX_BIN   # this Mac's architecture, as the installer looks
```

(Keep `from pathlib import Path`: the module still uses it.)

In `tests/test_setup_flow.py`, add `from ferb.runtimes.codex_exec import _NATIVE_BIN` to the imports (after line 13). Replace line 577 with:

```python
    binary = world["home"] / "node" / _NATIVE_BIN
```

and lines 592-593 with:

```python
    assert world["run"].ran(str(auth.parent.parent / "node" / _NATIVE_BIN), "login")
```

- [ ] **Step 4: Run the tests**

Run: `uv run pytest tests/test_packaging.py tests/test_install.py tests/test_setup_flow.py tests/test_cli.py -q`
Expected: PASS.

- [ ] **Step 5: Full checks and commit**

Run: `uv run pytest -q && uv run mypy ferb/core && git diff --check`

```bash
git add ferb/runtimes/codex_exec.py ferb/device/install.py tests/support/app_bundle.py tests/test_setup_flow.py tests/test_packaging.py
git diff --cached --stat
git commit -m "feat(L34): the daemon starts the Codex binary of its own architecture (packaging A, task 2)

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 3: Trim what FERB never runs

**Files:**
- Modify: `scripts/app-parts.sh` (add `ferb_arch_names`, `ferb_trim_app`)
- Modify: `scripts/build-app.sh:35` (the architecture), before `print "==> Signing ..."` (trim and Codex start check)
- Test: `tests/test_packaging.py` (offline trim test; real-bundle fixture and test)

**Interfaces:**
- Consumes: `native_bin(machine)` (Task 2), `_parts`, `_sized` and `compile_macho` (Task 1).
- Produces: `ferb_arch_names ARCH`, which returns 1 for anything but `arm64` or `x86_64` and otherwise sets:
  - `FERB_UV_ARCH`: `aarch64` or `x86_64`;
  - `FERB_NPM_CPU`: `arm64` or `x64`;
  - `FERB_CODEX_NPM`: `codex-darwin-$FERB_NPM_CPU`;
  - `FERB_CODEX_TRIPLE`: `aarch64-apple-darwin` or `x86_64-apple-darwin`;
  - `FERB_CODEX_BIN`: `Resources/node/node_modules/@openai/$FERB_CODEX_NPM/vendor/$FERB_CODEX_TRIPLE/bin/codex`.
- Produces: `ferb_trim_app APP ARCH`.
- Produces: a module-scoped pytest fixture `real_bundle` (a built FERB.app, or a skip), `_bundle_arch(app: Path) -> str`, and `MAX_APP_MB: dict[str, int]`.

- [ ] **Step 1: Write the failing offline tests**

Add to `tests/test_packaging.py`:

```python
def test_the_build_and_the_daemon_name_the_same_codex_binary_for_each_architecture() -> None:
    from ferb.runtimes.codex_exec import native_bin
    for arch in ("arm64", "x86_64"):
        done = _parts(f"ferb_arch_names {arch} && print -r -- $FERB_CODEX_BIN")
        assert done.stdout.strip() == f"Resources/node/{native_bin(arch)}", done.stderr
    assert _parts("ferb_arch_names ppc").returncode != 0


def test_trim_removes_only_what_ferb_never_runs_and_keeps_one_architecture(tmp_path: Path) -> None:
    from ferb.runtimes.codex_exec import native_bin
    for arch in ("arm64", "x86_64"):
        app = tmp_path / arch / "FERB.app"
        c = app / "Contents"
        codex = c / "Resources/node" / native_bin(arch)
        vendor = codex.parent.parent
        std = c / "Resources/python/lib/python3.12"
        site = std / "site-packages"
        gone = [vendor / "bin/codex-code-mode-host", vendor / "codex-resources/voice/bin/codex-voice-host",
                vendor / "codex-resources/voice/lib/libglib-2.0.0.dylib", vendor / "codex-resources/zsh/bin/zsh",
                std / "lib2to3/__init__.py", std / "pydoc_data/topics.py", std / "config-3.12-darwin/python.o",
                std / "turtle.py", std / "__pycache__/turtle.cpython-312.pyc", site / "regex/tests/test_regex.py",
                site / "jsonschema/benchmarks/issue232.py", site / "claude_agent_sdk/testing/__init__.py",
                site / "objc/_objc.cpython-312-darwin.so.dSYM/Contents/Info.plist"]
        kept = [codex, vendor / "codex-path/rg", vendor / "codex-package.json", std / "json/__init__.py",
                std / "unittest/__init__.py", site / "regex/__init__.py", site / "claude_agent_sdk/_bundled/claude",
                site / "PyObjCTools/TestSupport.py", site / "cryptography-50.0.1.dist-info/licenses/LICENSE",
                site / "jsonschema/protocols.py"]
        for f in gone + kept:
            _sized(f, 10)
        fat = compile_macho(site / "objc/_objc.cpython-312-darwin.so", "arm64", "x86_64")
        done = _parts(f"ferb_trim_app {shlex.quote(str(app))} {arch}")
        assert done.returncode == 0, done.stderr
        assert [str(f.relative_to(app)) for f in gone if f.exists()] == [], arch
        assert [str(f.relative_to(app)) for f in kept if not f.exists()] == [], arch
        archs = subprocess.run(["lipo", "-archs", str(fat)], capture_output=True, text=True).stdout.split()
        assert archs == [arch]                                    # a universal module keeps only this app's slice
```

- [ ] **Step 2: Run them to verify they fail**

Run: `uv run pytest tests/test_packaging.py -k "same_codex_binary or trim_removes" -v`
Expected: FAIL, with `ferb_arch_names: command not found` / `ferb_trim_app: command not found` in stderr.

- [ ] **Step 3: Add the functions to `scripts/app-parts.sh`**

Append:

```zsh
# ferb_arch_names ARCH: the names one architecture goes by (ARCH: arm64 or x86_64; anything else returns 1).
# FERB_UV_ARCH: uv's standalone CPython builds. FERB_NPM_CPU, FERB_CODEX_NPM: Codex's npm platform package.
# FERB_CODEX_TRIPLE: its binary's folder. FERB_CODEX_BIN: that binary in FERB.app, relative to Contents
# (ferb/runtimes/codex_exec.py native_bin names the same file).
ferb_arch_names() {
  case ${1:-} in
    arm64)  FERB_UV_ARCH=aarch64; FERB_NPM_CPU=arm64; FERB_CODEX_TRIPLE=aarch64-apple-darwin ;;
    x86_64) FERB_UV_ARCH=x86_64;  FERB_NPM_CPU=x64;   FERB_CODEX_TRIPLE=x86_64-apple-darwin ;;
    *) return 1 ;;
  esac
  FERB_CODEX_NPM="codex-darwin-$FERB_NPM_CPU"
  FERB_CODEX_BIN="Resources/node/node_modules/@openai/$FERB_CODEX_NPM/vendor/$FERB_CODEX_TRIPLE/bin/codex"
}

# ferb_trim_app APP ARCH: remove what FERB never runs, and keep one architecture in every universal Mach-O file.
# Run before signing (scripts/signing.sh signs what is left).
ferb_trim_app() {
  local app=${1:?} arch=${2:?}
  ferb_arch_names "$arch" || return 1
  local c="$app/Contents"
  local vendor="$c/Resources/node/node_modules/@openai/$FERB_CODEX_NPM/vendor/$FERB_CODEX_TRIPLE"
  local std="$c/Resources/python/lib/python3.12"
  local site="$std/site-packages"
  # Codex parts FERB turns off on every start (codex_exec.py DISABLED: code_mode_host and the shell tools) or never
  # starts (voice; its dylibs are re-signed by signing.sh, so OpenAI's library-validated voice host couldn't load them)
  rm -rf "${vendor:?}/bin/codex-code-mode-host" "${vendor:?}/codex-resources/voice" "${vendor:?}/codex-resources/zsh"
  # Python parts nothing imports: 2to3, pydoc's topic texts, the build configuration (python.o, Makefile), turtle,
  # test suites and benchmarks, the SDK's test helpers, and debug symbols
  rm -rf "${std:?}"/{lib2to3,pydoc_data,config-3.12-darwin,turtle.py} "${std:?}"/__pycache__/turtle.*(N) \
         "${site:?}/jsonschema/benchmarks" "${site:?}/claude_agent_sdk/testing"
  find "$site" \( -name '*.dSYM' -o \( -type d -name tests \) \) -prune -exec rm -rf {} +
  # One architecture per app
  local f
  local -a files
  files=("${(@f)$(find "$c" -type f \( -name '*.so' -o -name '*.dylib' -o -perm -u+x \) -print)}")
  for f in $files; do
    [[ -n $f && "$(file -b "$f")" == "Mach-O universal"* ]] || continue
    lipo "$f" -thin "$arch" -output "$f.thin" || { print -u2 "$f has no $arch code"; return 1 }
    mv "$f.thin" "$f"
  done
}
```

- [ ] **Step 4: Run the offline tests to verify they pass**

Run: `uv run pytest tests/test_packaging.py -k "same_codex_binary or trim_removes" -v`
Expected: PASS.

- [ ] **Step 5: Call it from `build-app.sh`**

In `scripts/build-app.sh`, replace line 35:

```zsh
[[ "$(uname -m)" == arm64 ]] || die "FERB.app is built for Apple silicon (arm64)"
```

with:

```zsh
ARCH=arm64                            # FERB.app's one architecture
[[ "$(uname -m)" == arm64 ]] || die "FERB.app is built for Apple silicon (arm64)"
ferb_arch_names "$ARCH"
```

Immediately before `print "==> Signing (identity: $SIGN_IDENTITY)"`, add:

```zsh
print "==> Trimming what FERB never runs ($ARCH only)"
ferb_trim_app "$APP" "$ARCH"
# The trimmed Codex must still start: it prints its own model catalog offline, from an empty home (codex_exec.py
# bundled_catalog does the same at the daemon's start)
CHECK=$(mktemp -d "$WORK/codex-check.XXXXXX")
(cd "$CHECK" && HOME="$CHECK" CODEX_HOME="$CHECK" "$CONTENTS/$FERB_CODEX_BIN" debug models --bundled > models.json) \
  && "$PY" -I -c 'import json, sys; assert json.load(open(sys.argv[1]))["models"]' "$CHECK/models.json" \
  || die "Codex doesn't start without its trimmed parts"
rm -rf "${CHECK:?}"
```

- [ ] **Step 6: Write the real-bundle test, pinned to the owner's target**

In `tests/test_packaging.py`, add `import json` to the imports. Below `_real_bundle`, add:

```python
@pytest.fixture(scope="module")
def real_bundle(tmp_path_factory: pytest.TempPathFactory) -> Path:
    """One built FERB.app for every real-bundle test (FERB_APP_BUNDLE, or FERB_BUILD_APP=1 builds it once)."""
    return _real_bundle(tmp_path_factory)


def _bundle_arch(app: Path) -> str:
    return subprocess.run(["lipo", "-archs", str(app / "Contents/MacOS/FERBNotch")], capture_output=True,
                          text=True).stdout.strip()
```

Change the existing real test's signature and first line from:

```python
def test_real_bundle_structure_signature_and_embedded_cli(tmp_path_factory: pytest.TempPathFactory,
                                                          tmp_path: Path) -> None:
    app = _real_bundle(tmp_path_factory)
```

to:

```python
def test_real_bundle_structure_signature_and_embedded_cli(real_bundle: Path, tmp_path: Path) -> None:
    from ferb.runtimes.codex_exec import native_bin
    app = real_bundle
```

In that test's path tuple, replace the entry
`"Resources/node/node_modules/@openai/codex-darwin-arm64/vendor/aarch64-apple-darwin/bin/codex"` with
`"Resources/node/" + str(native_bin(_bundle_arch(app)))`.

Then add the new test. Use the owner's A2 numbers in `MAX_APP_MB`; the values below are the proposal:

```python
# The owner's size target (L34, packaging plan A, Task 1 answer A2), in MB of 10^6 bytes
MAX_APP_MB = {"arm64": 600, "x86_64": 630}


def test_real_bundle_holds_only_what_ferb_runs(real_bundle: Path, tmp_path: Path) -> None:
    from ferb.runtimes.codex_exec import native_bin
    app = real_bundle
    c = app / "Contents"
    arch = _bundle_arch(app)
    assert arch in MAX_APP_MB, arch                                          # one architecture per app
    codex = c / "Resources/node" / native_bin(arch)
    vendor = codex.parent.parent
    for rel in ("bin/codex-code-mode-host", "codex-resources/voice", "codex-resources/zsh"):
        assert not (vendor / rel).exists(), rel
    std = c / "Resources/python/lib/python3.12"
    for rel in ("lib2to3", "pydoc_data", "config-3.12-darwin", "turtle.py", "site-packages/claude_agent_sdk/testing",
                "site-packages/jsonschema/benchmarks"):
        assert not (std / rel).exists(), rel
    assert not list(std.glob("site-packages/**/*.dSYM")) and not list(std.glob("site-packages/*/tests"))
    assert (std / "site-packages/claude_agent_sdk/_bundled/claude").is_file()   # Claude Code stays (owner, A1)
    env = {"HOME": str(tmp_path), "CODEX_HOME": str(tmp_path), "PATH": "/usr/bin:/bin"}
    models = subprocess.run([str(codex), "debug", "models", "--bundled"], capture_output=True, text=True, env=env,
                            cwd=tmp_path)
    assert models.returncode == 0 and json.loads(models.stdout)["models"], models.stderr
    probe = "import platform, claude_agent_sdk, cryptography, jsonschema, regex, referencing; print(platform.machine())"
    seen = subprocess.run([str(c / "Resources/python/bin/python3.12"), "-I", "-B", "-c", probe], capture_output=True,
                          text=True, env={**env, "FERB_HOME": str(tmp_path / ".ferb")})
    assert seen.returncode == 0 and seen.stdout.strip() == arch, seen.stderr
    size = sum(f.stat().st_size for f in app.rglob("*") if f.is_file() and not f.is_symlink())
    assert size <= MAX_APP_MB[arch] * 1_000_000, f"{size / 1e6:.1f} MB for {arch}"
```

- [ ] **Step 7: Build for real and run the real-bundle tests**

```bash
rm -rf /private/tmp/ferb-pkg/arm64
nice -n 10 scripts/build-app.sh /private/tmp/ferb-pkg/arm64
FERB_APP_BUNDLE=/private/tmp/ferb-pkg/arm64/FERB.app uv run pytest tests/test_packaging.py -k real -v
```

Expected: the build ends with `ferb_size_report`'s table, about 581 MB in total (580.6 MB on the measured copy), and no `Codex doesn't start` error. Both real tests PASS. Save the printed table for Task 7.

- [ ] **Step 8: Full checks and commit**

Run: `uv run pytest -q && uv run mypy ferb/core && git diff --check`

```bash
git add scripts/app-parts.sh scripts/build-app.sh tests/test_packaging.py
git diff --cached --stat
git commit -m "feat(L34): FERB.app leaves out what FERB never runs: Codex's code-mode host, voice and zsh, Python tests, docs and debug symbols (packaging A, task 3)

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 4: Locked dependencies install on Intel

Skip this task if the owner chose A3 (b); then stop and report.

**Files:**
- Modify: `pyproject.toml:10`
- Modify: `uv.lock` (by `uv lock` only)
- Test: `tests/test_packaging.py`

**Interfaces:**
- Produces: `_runtime_packages(arch: str) -> list[dict[str, Any]]` and `_wheel_fits(filename: str, arch: str) -> bool` (test helpers).

- [ ] **Step 1: Write the failing test**

In `tests/test_packaging.py`, add `from typing import Any` to the imports, then:

```python
LOCK = ROOT / "uv.lock"


def _wheel_fits(filename: str, arch: str) -> bool:
    """A wheel that CPython 3.12 on macOS `arch` (arm64 or x86_64) can install."""
    python, abi, platform = filename[: -len(".whl")].split("-")[-3:]
    if platform == "any":
        return True
    mac = any(tag.startswith("macosx_") and tag.endswith((f"_{arch}", "_universal2")) for tag in platform.split("."))
    cpython = any(tag in ("py3", "cp312") or (abi == "abi3" and tag.startswith("cp3") and int(tag[3:]) <= 12)
                  for tag in python.split("."))
    return mac and cpython


def _runtime_packages(arch: str) -> list[dict[str, Any]]:
    """Every package uv.lock installs into FERB.app's Python on macOS `arch`: ferb's dependencies and theirs, with the
    extras they ask for, markers evaluated for that Mac (no dev group, no optional memory extra)."""
    import tomllib
    from packaging.markers import Marker
    env = {"sys_platform": "darwin", "platform_system": "Darwin", "platform_machine": arch, "os_name": "posix",
           "python_version": "3.12", "python_full_version": "3.12.13", "implementation_name": "cpython",
           "platform_python_implementation": "CPython", "extra": ""}

    def applies(marker: str | None) -> bool:
        return marker is None or Marker(marker).evaluate(env)

    by_name: dict[str, list[dict[str, Any]]] = {}
    for p in tomllib.loads(LOCK.read_text())["package"]:
        by_name.setdefault(p["name"], []).append(p)

    def pick(dep: dict[str, Any]) -> dict[str, Any]:
        found = [p for p in by_name[dep["name"]] if dep.get("version", p["version"]) == p["version"]
                 and any(applies(m) for m in p.get("resolution-markers", [None]))]
        assert len(found) == 1, (arch, dep, [p["version"] for p in found])
        return found[0]

    seen: dict[tuple[str, str], dict[str, Any]] = {}
    walked: set[tuple[str, str, tuple[str, ...]]] = set()
    todo: list[tuple[dict[str, Any], tuple[str, ...]]] = [(by_name["ferb"][0], ())]
    while todo:
        pkg, extras = todo.pop()
        deps = list(pkg.get("dependencies", []))
        for extra in extras:
            deps += pkg.get("optional-dependencies", {}).get(extra, [])
        for dep in deps:
            if not applies(dep.get("marker")):
                continue
            p = pick(dep)
            key = (p["name"], p["version"], tuple(dep.get("extra", [])))
            if key in walked:
                continue
            walked.add(key)
            seen[(p["name"], p["version"])] = p
            todo.append((p, key[2]))
    return list(seen.values())


def test_every_locked_runtime_package_installs_on_both_architectures() -> None:
    """FERB.app ships for Apple silicon and Intel (L34), so each package its Python loads needs a wheel for both; a
    source-only package would need a compiler toolchain at release time (cryptography 49 dropped Intel Mac wheels)."""
    for arch in ("arm64", "x86_64"):
        packages = _runtime_packages(arch)
        assert len(packages) > 40, arch                                       # the walk found the whole tree
        missing = sorted(f'{p["name"]} {p["version"]}' for p in packages
                         if not any(_wheel_fits(w["url"].rsplit("/", 1)[1], arch) for w in p.get("wheels", [])))
        assert missing == [], (arch, missing)
```

- [ ] **Step 2: Run it to verify it fails**

Run: `uv run pytest tests/test_packaging.py::test_every_locked_runtime_package_installs_on_both_architectures -v`
Expected: FAIL with `('x86_64', ['cryptography 50.0.1'])`.

- [ ] **Step 3: Pin cryptography per architecture**

In `pyproject.toml`, replace line 10 (`    "cryptography==50.0.1",`) with:

```toml
    "cryptography==50.0.1; sys_platform != 'darwin' or platform_machine != 'x86_64'",
    "cryptography==48.0.1; sys_platform == 'darwin' and platform_machine == 'x86_64'",   # last with Intel Mac wheels
```

Then:

```bash
uv lock
git diff --stat uv.lock
grep -n 'name = "cryptography"' uv.lock
uv sync --group dev
uv run python -c "import cryptography; print(cryptography.__version__)"
```

Expected: `uv.lock` changes only the cryptography entries, their `resolution-markers` and the references to them; two `name = "cryptography"` entries (48.0.1 and 50.0.1); the dev venv still prints `50.0.1`.

- [ ] **Step 4: Run the test to verify it passes**

Run: `uv run pytest tests/test_packaging.py::test_every_locked_runtime_package_installs_on_both_architectures -v`
Expected: PASS.

- [ ] **Step 5: Full checks and commit**

Run: `uv run pytest -q && uv run mypy ferb/core && git diff --check`

```bash
git add pyproject.toml uv.lock tests/test_packaging.py
git diff --cached --stat
git commit -m "build(L34): the Intel app uses cryptography 48.0.1, the last release with Intel Mac wheels; Apple silicon keeps 50.0.1 (packaging A, task 4)

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 5: `build-app.sh --arch arm64|x86_64`

**Files:**
- Modify: `scripts/app-parts.sh` (add `ferb_check_arch`)
- Modify: `scripts/build-app.sh` (header, arguments, Notch, Python, agent, Codex, secret helper, voice helper, trim block)
- Modify: `macapp/FERB/build.sh:8`
- Test: `tests/test_packaging.py`

**Interfaces:**
- Consumes: `ferb_arch_names`, `ferb_trim_app` and `FERB_CODEX_BIN` (Task 3); `_stubs(tmp_path)` (existing, `tests/test_packaging.py:97`).
- Produces: `ferb_check_arch APP ARCH`, which returns 0 when every Mach-O file is exactly `ARCH`, and otherwise prints `Not <ARCH>-only:` and one `  <path in app>: <lipo archs>` line per file, then returns 1. Also `build-app.sh [--arch arm64|x86_64] [OUT_DIR]`.

- [ ] **Step 1: Write the failing tests**

Add to `tests/test_packaging.py`:

```python
def test_arch_check_names_every_file_built_for_another_architecture(tmp_path: Path) -> None:
    app = tmp_path / "FERB.app"
    c = app / "Contents"
    compile_macho(c / "MacOS/FERBNotch", "arm64")
    compile_macho(c / "Resources/python/lib/libpython3.12.dylib", "arm64")
    launcher = _sized(c / "Resources/bin/ferb", 10)
    launcher.chmod(0o755)                                                     # an executable script: not Mach-O
    assert _parts(f"ferb_check_arch {shlex.quote(str(app))} arm64").returncode == 0
    compile_macho(c / "Helpers/ferb-secrets", "x86_64")
    compile_macho(c / "Resources/python/lib/python3.12/site-packages/objc/_objc.cpython-312-darwin.so",
                  "arm64", "x86_64")
    done = _parts(f"ferb_check_arch {shlex.quote(str(app))} arm64")
    assert done.returncode != 0
    assert "Contents/Helpers/ferb-secrets: x86_64" in done.stderr
    assert "_objc.cpython-312-darwin.so: " in done.stderr and "MacOS/FERBNotch" not in done.stderr


def test_build_app_refuses_an_unknown_architecture_and_intel_without_rosetta(tmp_path: Path) -> None:
    env = _stubs(tmp_path)
    out = tmp_path / "out"
    bad = subprocess.run([str(BUILD_APP), "--arch", "ppc", str(out)], capture_output=True, text=True, env=env)
    assert bad.returncode != 0 and "arm64 or x86_64" in bad.stderr
    alone = subprocess.run([str(BUILD_APP), "--arch"], capture_output=True, text=True, env=env)
    assert alone.returncode != 0 and "usage" in alone.stderr
    stubs = tmp_path / "stubs"
    (stubs / "uname").write_text('#!/bin/zsh\nif [[ "$1" == -m ]]; then print arm64; else /usr/bin/uname "$@"; fi\n')
    (stubs / "arch").write_text("#!/bin/zsh\nexit 1\n")                       # Rosetta isn't installed
    for name in ("uname", "arch"):
        (stubs / name).chmod(0o755)
    intel = subprocess.run([str(BUILD_APP), "--arch", "x86_64", str(out)], capture_output=True, text=True, env=env)
    assert intel.returncode != 0 and "Rosetta" in intel.stderr
    assert not out.exists()                                                   # refused before anything was written
```

- [ ] **Step 2: Run them to verify they fail**

Run: `uv run pytest tests/test_packaging.py -k "arch_check_names or refuses_an_unknown_architecture" -v`
Expected: FAIL. `ferb_check_arch` is missing; `--arch ppc` is taken as the output folder, so the build starts and fails without "arm64 or x86_64".

- [ ] **Step 3: Add `ferb_check_arch` to `scripts/app-parts.sh`**

```zsh
# ferb_check_arch APP ARCH: every Mach-O file in FERB.app is ARCH code only; otherwise names each one that isn't
ferb_check_arch() {
  local app=${1:A} arch=${2:?} f got
  local -a files bad
  files=("${(@f)$(find "$app/Contents" -type f \( -name '*.so' -o -name '*.dylib' -o -perm -u+x \) -print)}")
  for f in $files; do
    [[ -n $f && "$(file -b "$f")" == Mach-O* ]] || continue
    got=$(lipo -archs "$f" 2>/dev/null || true)
    [[ $got == $arch ]] || bad+=("${f#$app/}: ${got:-unreadable}")
  done
  (( ${#bad} == 0 )) && return 0
  print -u2 "Not $arch-only:"
  print -u2 -l -- "  "${^bad}
  return 1
}
```

- [ ] **Step 4: Make `build-app.sh` build one architecture of the owner's choice**

In `scripts/build-app.sh`, replace the header's usage line 4:

```zsh
#   scripts/build-app.sh [OUT_DIR]          build OUT_DIR/FERB.app (default build/app), then sign it
```

with:

```zsh
#   scripts/build-app.sh [--arch arm64|x86_64] [OUT_DIR]
#                                           build OUT_DIR/FERB.app (default build/app-<arch>) for one architecture
#                                           (default: this Mac's; an Intel app builds on Apple silicon under Rosetta)
```

Replace lines 31-38 (from `OUT="${${1:-$ROOT/build/app}:A}"` through the `done` of the tool loop, including Task 3's `ARCH=arm64`, uname check and `ferb_arch_names` lines) with:

```zsh
ARCH=$(uname -m)                      # FERB.app's one architecture: arm64 (Apple silicon) or x86_64 (Intel)
if [[ "${1:-}" == --arch ]]; then
  (( $# >= 2 )) || die "usage: build-app.sh [--arch arm64|x86_64] [OUT_DIR]"
  ARCH=$2
  shift 2
fi
ferb_arch_names "$ARCH" || die "--arch is arm64 or x86_64, not '$ARCH'"
if [[ "$ARCH" != "$(uname -m)" ]]; then
  # The Intel app is built on Apple silicon by running its Intel Python and Codex under Rosetta
  [[ "$ARCH" == x86_64 ]] || die "an arm64 FERB.app is built on Apple silicon"
  arch -x86_64 /usr/bin/true 2>/dev/null || die "building the Intel app here needs Rosetta: softwareupdate --install-rosetta"
fi
OUT="${${1:-$ROOT/build/app-$ARCH}:A}"
APP="$OUT/FERB.app"
WORK="$OUT/.work"                     # build cache (Swift, Python download, npm); `rm -rf` it for a clean build
ferb_check_notary_credentials
for tool in uv swift xcrun npm codesign ditto install_name_tool lipo; do
  command -v $tool >/dev/null || die "needs $tool"
done
```

Replace the Notch block's `swift build` line and the two-line `cp` after it (originally lines 58-60) with:

```zsh
SWIFT=(swift build --package-path "$NOTCH" -c release --arch "$ARCH" --scratch-path "$WORK/swift")
"${SWIFT[@]}" -Xswiftc -file-prefix-map -Xswiftc "$ROOT=."
cp "$("${SWIFT[@]}" --show-bin-path)/FERBNotch" "$CONTENTS/MacOS/FERBNotch"
```

and its header line `print "==> Notch UI (main executable)"` with `print "==> Notch UI (main executable, $ARCH)"`.

Replace the Python block's first lines (70-73 and 77):

```zsh
print "==> Python $CPYTHON (uv's standalone CPython; relocatable)"
uv python install "$CPYTHON" --install-dir "$WORK/python" --no-bin --quiet
PY_SRC="$WORK/python/cpython-$CPYTHON-macos-aarch64-none"
[[ -x "$PY_SRC/bin/python$PY_MINOR" ]] || die "uv installed no CPython $CPYTHON under $WORK/python"
```

```zsh
[[ "$("$PY" -I -c 'import platform; print(platform.python_version())')" == "$CPYTHON" ]] || die "not CPython $CPYTHON"
```

with:

```zsh
print "==> Python $CPYTHON for $ARCH (uv's standalone CPython; relocatable)"
PY_KEY="cpython-$CPYTHON-macos-$FERB_UV_ARCH-none"
uv python install "$PY_KEY" --install-dir "$WORK/python" --no-bin --quiet
PY_SRC="$WORK/python/$PY_KEY"
[[ -x "$PY_SRC/bin/python$PY_MINOR" ]] || die "uv installed no CPython $CPYTHON for $ARCH under $WORK/python"
```

```zsh
[[ "$("$PY" -I -c 'import platform; print(platform.python_version(), platform.machine())')" == "$CPYTHON $ARCH" ]] \
  || die "not CPython $CPYTHON for $ARCH"
```

(`uv pip install --python "$PY"` then picks that interpreter's wheels by itself: x86_64 under Rosetta.)

In the agent block (line 114), change `xcrun clang -O2 -arch arm64` to `xcrun clang -O2 -arch "$ARCH"`.

Replace the Codex block (lines 127-135) with:

```zsh
print "==> Codex $CODEX_VERSION for $ARCH (pinned in package-lock.json)"
mkdir -p "$WORK/node"
cp "$ROOT/package.json" "$ROOT/package-lock.json" "$WORK/node/"
(cd "$WORK/node" && npm ci --ignore-scripts --no-audit --no-fund --loglevel=error --os=darwin --cpu="$FERB_NPM_CPU")
CODEX_PKG="$WORK/node/node_modules/@openai/$FERB_CODEX_NPM"
[[ -d "$CODEX_PKG" ]] || die "npm installed no $FERB_CODEX_NPM"
got=$("$CODEX_PKG/vendor/$FERB_CODEX_TRIPLE/bin/codex" --version | awk '{ print $NF }')
[[ "$got" == "$CODEX_VERSION" ]] || die "package-lock.json has Codex $got, ferb pins $CODEX_VERSION"
mkdir -p "$CONTENTS/Resources/node/node_modules/@openai"
ditto "$CODEX_PKG" "$CONTENTS/Resources/node/node_modules/@openai/$FERB_CODEX_NPM"
```

In the secret helper block (line 138), change `-target arm64-apple-macos14.2` to `-target "$ARCH-apple-macos14.2"`.

In the voice helper block (line 144), change `IDENTITY=- "$ROOT/macapp/FERB/build.sh" "$VOICE" >/dev/null` to `ARCH="$ARCH" IDENTITY=- "$ROOT/macapp/FERB/build.sh" "$VOICE" >/dev/null`.

In Task 3's trim block, after `rm -rf "${CHECK:?}"`, add:

```zsh
ferb_check_arch "$APP" "$ARCH" || die "FERB.app ($ARCH) holds code for another architecture"
```

In `macapp/FERB/build.sh`, replace line 2 and line 8:

```sh
# Build FERB.app into $1 (default: ./build/FERB.app) and sign it with the local certificate (or $IDENTITY).
```

```sh
swiftc -O -target arm64-apple-macos14.2 -o "$APP/Contents/MacOS/FERB" main.swift home.swift \
```

with:

```sh
# Build FERB.app into $1 (default: ./build/FERB.app) for $ARCH (default: this Mac's) and sign it with the local
# certificate (or $IDENTITY).
```

```sh
swiftc -O -target "${ARCH:-$(uname -m)}-apple-macos14.2" -o "$APP/Contents/MacOS/FERB" main.swift home.swift \
```

- [ ] **Step 5: Run the offline tests**

Run: `uv run pytest tests/test_packaging.py -v`
Expected: PASS (the real-bundle tests skip without `FERB_APP_BUNDLE`).

- [ ] **Step 6: Check the architecture in the real-bundle test too**

In `test_real_bundle_holds_only_what_ferb_runs`, after the size assertion, add:

```python
    check = _parts(f"ferb_check_arch {shlex.quote(str(app))} {arch}")
    assert check.returncode == 0, check.stderr
```

- [ ] **Step 7: Build both architectures for real and run the real-bundle tests on each**

```bash
rm -rf /private/tmp/ferb-pkg
nice -n 10 scripts/build-app.sh --arch arm64 /private/tmp/ferb-pkg/arm64
nice -n 10 scripts/build-app.sh --arch x86_64 /private/tmp/ferb-pkg/x86_64
for a in arm64 x86_64; do
  FERB_APP_BUNDLE=/private/tmp/ferb-pkg/$a/FERB.app uv run pytest tests/test_packaging.py -k real -v
done
```

Expected:
- The x86_64 build prints `Python 3.12.13 for x86_64`, `Codex 0.157.1 for x86_64`, no `Not x86_64-only` list, and a size table within the A2 Intel target.
- Every real test passes for both bundles. The x86_64 bundle runs under Rosetta here; that is not the Intel test (Task 7).
- `uv` may print that it downloaded `cpython-3.12.13-macos-x86_64-none`; that's expected.

Save both size tables for Task 7.

- [ ] **Step 8: Full checks and commit**

Run: `uv run pytest -q && uv run mypy ferb/core && git diff --check`

```bash
git add scripts/app-parts.sh scripts/build-app.sh macapp/FERB/build.sh tests/test_packaging.py
git diff --cached --stat
git commit -m "feat(L34): build-app.sh --arch arm64|x86_64: one architecture per FERB.app, checked file by file (packaging A, task 5)

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 6: Two disk images

**Files:**
- Modify: `scripts/build-dmg.sh` (whole file)
- Test: `tests/test_packaging.py`

**Interfaces:**
- Consumes: `build-app.sh --arch` (Task 5); `_stubs`, `_fake_app` (existing); `compile_macho` (Task 1).
- Produces: `build-dmg.sh` with no arguments writes `$FERB_DMG_DIR/FERB-AppleSilicon.dmg` and `$FERB_DMG_DIR/FERB-Intel.dmg`. `build-dmg.sh APP [DMG]` packs one app. Each image holds `FERB.app`, `Applications -> /Applications` and the file `First open — System Settings → Privacy & Security → Open Anyway.txt`.

- [ ] **Step 1: Write the failing test**

Add to `tests/test_packaging.py`:

```python
FIRST_OPEN = "First open — System Settings → Privacy & Security → Open Anyway.txt"


def test_dmg_is_named_for_the_apps_architecture_and_says_how_to_open_it_the_first_time(tmp_path: Path) -> None:
    env = {**_stubs(tmp_path), "FERB_DMG_DIR": str(tmp_path / "dist")}
    for arch, name in (("arm64", "FERB-AppleSilicon.dmg"), ("x86_64", "FERB-Intel.dmg")):
        app = _fake_app(tmp_path / arch)
        compile_macho(app / "Contents/MacOS/FERBNotch", arch)
        done = subprocess.run([str(BUILD_DMG), str(app)], capture_output=True, text=True, env=env)
        assert done.returncode == 0, done.stderr + done.stdout
        assert (tmp_path / "dist" / name).is_file(), name
    staging = (tmp_path / "staging.txt").read_text()
    assert FIRST_OPEN in staging and "Applications -> /Applications" in staging and "FERB.app" in staging
    universal = _fake_app(tmp_path / "fat")              # copies of /usr/bin/true: x86_64 and arm64e in one file
    done = subprocess.run([str(BUILD_DMG), str(universal)], capture_output=True, text=True, env=env)
    assert done.returncode != 0 and "one-architecture" in done.stderr
    assert sorted(p.name for p in (tmp_path / "dist").iterdir()) == ["FERB-AppleSilicon.dmg", "FERB-Intel.dmg"]
```

- [ ] **Step 2: Run it to verify it fails**

Run: `uv run pytest tests/test_packaging.py::test_dmg_is_named_for_the_apps_architecture_and_says_how_to_open_it_the_first_time -v`
Expected: FAIL. The current script names the image `build/FERB-<version>.dmg` and ignores `FERB_DMG_DIR`.

- [ ] **Step 3: Rewrite `scripts/build-dmg.sh`**

```zsh
#!/bin/zsh -f
# Package FERB.app into disk images with an Applications link (L27, L34): one per architecture. Ad-hoc by default;
# with a Developer ID SIGN_IDENTITY each image is also signed, notarized and stapled (scripts/signing.sh).
#
#   scripts/build-dmg.sh             build both apps (scripts/build-app.sh --arch arm64, then x86_64) into
#                                    $FERB_DMG_DIR/app-<arch>, then FERB-AppleSilicon.dmg and FERB-Intel.dmg there
#   scripts/build-dmg.sh APP [DMG]   one image of an assembled app; default $FERB_DMG_DIR/<name for its architecture>
#
# FERB_DMG_DIR defaults to build/.
set -euo pipefail
ROOT="${0:A:h:h}"
source "$ROOT/scripts/signing.sh"

die() { print -u2 "build-dmg.sh: $*"; exit 1 }
DMG_DIR="${${FERB_DMG_DIR:-$ROOT/build}:A}"
FIRST_OPEN="First open — System Settings → Privacy & Security → Open Anyway.txt"
STAGE=""
trap 'if [[ -n "$STAGE" ]]; then rm -rf "$STAGE"; fi' EXIT

dmg_name() {                          # $1: an architecture, as lipo names it
  case ${1:-} in
    arm64)  print FERB-AppleSilicon.dmg ;;
    x86_64) print FERB-Intel.dmg ;;
    *) return 1 ;;
  esac
}

make_dmg() {                          # $1: an assembled FERB.app, $2: the image to write
  local app=${1:A} dmg=${2:A}
  STAGE="${dmg:h}/.dmg-staging"
  rm -rf "${STAGE:?}" "${dmg:?}"
  mkdir -p "$STAGE"                   # its parent too: the image's folder
  ditto "$app" "$STAGE/FERB.app"
  ln -s /Applications "$STAGE/Applications"
  print -l -- "FERB isn't signed by an Apple-registered developer, so macOS stops it the first time:" \
    "1. Drag FERB to Applications and open it from there." \
    "2. Open System Settings → Privacy & Security, scroll to Security and click Open Anyway next to FERB." \
    "3. Enter your password. FERB opens, and macOS remembers your choice." > "$STAGE/$FIRST_OPEN"
  hdiutil create -volname FERB -srcfolder "$STAGE" -fs HFS+ -format ULMO -ov "$dmg" >/dev/null
  rm -rf "${STAGE:?}"
  STAGE=""
  if ferb_developer_id; then
    codesign --force --sign "$SIGN_IDENTITY" --timestamp "$dmg"
    ferb_notarize "$dmg" "$dmg"
  fi
  print "Built: $dmg ($(du -h "$dmg" | cut -f1))"
}

ferb_check_notary_credentials
if (( $# == 0 )); then
  for arch in arm64 x86_64; do
    "$ROOT/scripts/build-app.sh" --arch $arch "$DMG_DIR/app-$arch"
    make_dmg "$DMG_DIR/app-$arch/FERB.app" "$DMG_DIR/$(dmg_name $arch)"
  done
  exit 0
fi
APP="$1"
[[ -d "$APP" ]] || die "no app at $APP"
if (( $# >= 2 )); then
  DMG="$2"
else
  arch=$(lipo -archs "$APP/Contents/MacOS/FERBNotch" 2>/dev/null || true)
  name=$(dmg_name "$arch") || die "$APP isn't a one-architecture FERB.app (lipo says '${arch:-nothing}')"
  DMG="$DMG_DIR/$name"
fi
make_dmg "$APP" "$DMG"
```

- [ ] **Step 4: Run the DMG tests**

Run: `uv run pytest tests/test_packaging.py -k dmg -v`
Expected: PASS for the new test and for the existing `test_dmg_holds_the_app_and_an_applications_link_and_is_notarized_only_with_a_developer_id`, which passes `APP DMG` explicitly.

- [ ] **Step 5: Make both images for real**

```bash
FERB_DMG_DIR=/private/tmp/ferb-pkg/dist nice -n 10 scripts/build-dmg.sh
ls -l /private/tmp/ferb-pkg/dist/*.dmg
hdiutil attach -nobrowse -readonly /private/tmp/ferb-pkg/dist/FERB-AppleSilicon.dmg -mountpoint /private/tmp/ferb-pkg/mnt
ls -la /private/tmp/ferb-pkg/mnt && cat "/private/tmp/ferb-pkg/mnt/First open — System Settings → Privacy & Security → Open Anyway.txt"
hdiutil detach /private/tmp/ferb-pkg/mnt
```

Expected: two images, both apps built, each image showing FERB.app, the Applications link and the first-open file with its three steps. Record both image sizes for Task 7.

- [ ] **Step 6: Full checks and commit**

Run: `uv run pytest -q && uv run mypy ferb/core && git diff --check`

```bash
git add scripts/build-dmg.sh tests/test_packaging.py
git diff --cached --stat
git commit -m "feat(L34): build-dmg.sh makes FERB-AppleSilicon.dmg and FERB-Intel.dmg, each saying how to open FERB the first time (packaging A, task 6)

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 7: Docs, the decision record and the owner's live check

**Files:**
- Modify: `README.md` (Setup requirement line 17; disk image steps 22-23; build commands 50-53; Current limitations 599-601)
- Modify: `ARCHITECTURE.md` ("Packaged FERB.app" bullets, around 2904-2929)
- Modify: `CHANGELOG.md` (new entry at the top)
- Modify: `docs/0.27_DECISIONS.toml` (L34)
- Modify: `docs/OWNER_ACCEPTANCE.md` (new section at the end)

- [ ] **Step 1: README**

- Line 17: replace "Requires macOS 14.2 or later on Apple silicon" with "Requires macOS 14.2 or later, on Apple silicon (`FERB-AppleSilicon.dmg`) or an Intel Mac (`FERB-Intel.dmg`),".
- Step 1 of "FERB.app (disk image)": replace "Open `FERB-<version>.dmg`" with "Open `FERB-AppleSilicon.dmg` (M1 or later) or `FERB-Intel.dmg`; the disk image's first-open note repeats step 2".
- Lines 50-53: replace "Build the app and disk image with `scripts/build-app.sh` and `scripts/build-dmg.sh`" with "`scripts/build-dmg.sh` builds both apps and both disk images; `scripts/build-app.sh [--arch arm64|x86_64] [OUT_DIR]` builds one app (an Intel app builds on Apple silicon under Rosetta) and prints its size by part". Also change the `--sign-only` example path `build/app/FERB.app` to `build/app-arm64/FERB.app`.
- Current limitations: replace "The app is about 666 MB." with the measured totals from Task 5 Step 7 and Task 6 Step 5, for example "The app is about 581 MB on Apple silicon and 610 MB on Intel (downloads 170 MB and 185 MB)." Write the numbers the size tables printed. Add the Intel testing status the owner chose in A4, quoted from their answer.

- [ ] **Step 2: ARCHITECTURE.md "Packaged FERB.app"**

Replace the **Bundle** bullet with:

```markdown
- **Bundle.** `com.ferb.FERB`, "FERB", the notch UI as main executable; one architecture per app (arm64 or x86_64,
  `scripts/build-app.sh --arch`, checked file by file by `ferb_check_arch`); uv's standalone CPython 3.12.13 for
  that architecture with `ferb` and its locked dependencies in `Contents/Resources/python` (precompiled, never
  written; no lib2to3, pydoc_data, build configuration, test suites or debug symbols); the `ferb` launcher; Codex's
  platform package for that architecture in `Contents/Resources/node` without the code-mode host, voice and zsh
  (FERB disables or never starts them; `codex debug models --bundled` proves each build still starts);
  `Contents/Helpers/ferb-secrets`; `Contents/Helpers/FERB Voice.app`; and the agent `Contents/Helpers/FERB.app`.
  `scripts/app-parts.sh` names each architecture's parts, trims, thins universal files and prints the size table.
  The Intel app uses cryptography 48.0.1 (the last with Intel Mac wheels); Apple silicon uses 50.0.1.
```

In the **Residuals** bullet, replace "the app is about 666 MB" with the same measured totals as the README.

- [ ] **Step 3: CHANGELOG.md**

Insert above the first `## ` entry:

```markdown
## 0.27.0.dev1 FERB for everyone, phase A (L34) — 2026-10-25 — two architectures, smaller app; owner live check pending

Spec `docs/superpowers/specs/2026-10-24-ferb-for-everyone-design.md`; plan
`docs/superpowers/plans/2026-10-25-packaging-a-shrink-two-arch.md` (owner's answers at its end).
- FERB.app leaves out what FERB never runs: Codex's code-mode host (FERB disables it on every start), voice and
  zsh, Python's lib2to3, pydoc topics, build configuration, test suites and debug symbols, and the other
  architecture's half of universal modules. Apple silicon: 682.3 MB before, 580.6 MB after (download 197.5 MB
  before, 170.0 MB after); Intel: 610.0 MB (download 185.0 MB).
- `scripts/build-app.sh --arch arm64|x86_64`; `scripts/build-dmg.sh` makes `FERB-AppleSilicon.dmg` and
  `FERB-Intel.dmg`, each with a first-open note. The daemon starts the Codex binary of its own architecture.
- The Intel app uses cryptography 48.0.1, the last release with Intel Mac wheels (owner, A3).
- Checkout validation: 0 failed in `uv run pytest -q`, mypy clean. Real-bundle tests passed for both builds (the
  Intel one under Rosetta). Installed: no. Owner live check: pending (`docs/OWNER_ACCEPTANCE.md`).
```

Before committing, change the sizes above to the totals that Task 5 Step 7's size tables and Task 6 Step 5's
`ls -l` printed (the numbers here are the plan's estimates). Also replace "0 failed" with the summary line
`uv run pytest -q` printed.

- [ ] **Step 4: The decision record**

Run `grep -n '^\[L34\]' docs/0.27_DECISIONS.toml`. If an `[L34]` entry exists, add `"tests/test_packaging.py"` to its `tests` if missing and append the phase A sentence below to its `note`. Otherwise add after the `[L35]` entry:

```toml
[L34]
milestone = "L"
tests = ["tests/test_packaging.py"]
note = "Owner 2026-10-24: free items only (no Developer ID or notarization for now), updates install by themselves, two DMGs (\"Two DMGs\"), the notch stays the main interface; 2026-10-25 approved the design: \"all 7 parts in full, just design them bare min. focus on correct working\". Spec docs/superpowers/specs/2026-10-24-ferb-for-everyone-design.md; one plan per phase in docs/superpowers/plans/2026-10-25-packaging-*.md. Phase A: the owner's Task 1 answers are recorded at the end of the phase A plan."
summary = "FERB for everyone: two disk images (Apple silicon, Intel) of a trimmed FERB.app, a stable self-made signing certificate, native setup, real notifications and menu bar state, the FERB window and Settings, launch at login, and automatic updates."
```

Then append the owner's A1–A4 answers, quoted, to the `note`.

Run: `uv run pytest tests/test_decision_coverage.py -q`
Expected: PASS.

- [ ] **Step 5: OWNER_ACCEPTANCE.md**

Append:

```markdown
## FERB for everyone, phase A (L34) — pending live checks (2026-10-25)

Checkout validation is in the changelog entry; nothing here is installed on the owner's account. Run on a second
macOS user account (System Settings → Users & Groups → Add User), never the owner's own; pause between supervised
actions and record what the owner saw.

- Apple silicon: copy `FERB-AppleSilicon.dmg` to `/Users/Shared`, and in the second account open it, drag FERB into
  `~/Applications` (make the folder first; the image's Applications link is the shared `/Applications`), open it,
  click Open Anyway once in Privacy & Security, finish setup, sign in to ChatGPT, then ask FERB: "make a folder
  ferb-pkg-test on my Desktop with a file hello.txt that says hi". Expected: the task finishes and the file exists
  (Codex works without its trimmed parts). Record the first-open note the disk image showed.
- Intel, as the owner chose in A4: on a real Intel Mac, the same steps with `FERB-Intel.dmg`, plus whether the
  notch draws at the top of a notchless screen. Without one, the same steps under Rosetta on the second account
  (remove the Apple silicon FERB first: `~/.local/bin/ferb uninstall`, then trash `~/Applications/FERB.app`),
  recorded as "Rosetta only, not an Intel test".
- Afterwards: `~/.local/bin/ferb uninstall` and trash `~/Applications/FERB.app` in the second account; delete
  `~/Desktop/ferb-pkg-test` there.
```

- [ ] **Step 6: Full checks and commit**

Run: `uv run pytest -q && uv run mypy ferb/core && git diff --check`

```bash
git add README.md ARCHITECTURE.md CHANGELOG.md docs/0.27_DECISIONS.toml docs/OWNER_ACCEPTANCE.md
git diff --cached --stat
git commit -m "docs(L34): packaging phase A: two disk images, the trimmed app's size, the owner's live check

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
git push origin main
```

- [ ] **Step 7: Owner live check and verdict**

Hand the owner the phase A section of `docs/OWNER_ACCEPTANCE.md`. Report three things separately: checkout validation (test summary), installation status (nothing installed on the owner's account) and the owner's live results. Record what the owner saw under that section, with `ferb verdict TASK pass|fail` for the live task run in the second account.
