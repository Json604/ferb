"""FERB.app with the Rust engine (rust/PLAN.md part 13; scripts/build-app-rust.sh and scripts/rust-parts.sh): the
pins it reads from the Rust sources, the Claude Code lock, the places in the bundle, the runtime Markdown it carries,
its checks for Python left behind, the notices, and (with `FERB_RUST_APP_BUNDLE=/path/FERB.app`) a built bundle,
including its agent's refusals and a daemon on a temporary home through the development override.

Nothing here builds the app, installs, registers a service, runs launchctl or reads ~/.ferb."""

import json
import os
import plistlib
import shlex
import shutil
import signal
import socket
import subprocess
import time
from pathlib import Path

import pytest

from tests.support.macho import compile_macho

ROOT = Path(__file__).resolve().parent.parent
BUILD = ROOT / "scripts" / "build-app-rust.sh"
PARTS = ROOT / "scripts" / "rust-parts.sh"
CRATES = ROOT / "rust" / "crates"
AGENT_PLIST = ROOT / "ui" / "mac" / "FERBAgent" / "Info.plist"


def _zsh(code: str, **env: str) -> subprocess.CompletedProcess[str]:
    """zsh code run after sourcing app-parts.sh, signing.sh (ad hoc) and rust-parts.sh, with the system PATH."""
    src = " ".join(f"source {shlex.quote(str(ROOT / 'scripts' / s))};"
                   for s in ("app-parts.sh", "signing.sh", "rust-parts.sh"))
    return subprocess.run(["/bin/zsh", "-f", "-c", f"ROOT={shlex.quote(str(ROOT))}; {src} {code}"],
                          capture_output=True, text=True,
                          env={"PATH": "/usr/bin:/bin:/usr/sbin:/sbin", "HOME": "/nonexistent-home", **env})


def _vars(code: str, *names: str) -> dict[str, str]:
    done = _zsh(code + "; " + " ".join(f"print -r -- \"{n}=${n}\";" for n in names))
    assert done.returncode == 0, done.stderr
    return dict(line.split("=", 1) for line in done.stdout.splitlines() if "=" in line)


def test_the_rust_build_scripts_are_executable_zsh_that_parses() -> None:
    for script in (BUILD, PARTS):
        assert subprocess.run(["/bin/zsh", "-n", str(script)], capture_output=True).returncode == 0, script
    assert os.access(BUILD, os.X_OK)


# decision: ENG-7
def test_the_rust_builds_pins_come_from_the_rust_sources_and_name_todays_programs() -> None:
    """The Rust build reads its version and pins from the Rust code that enforces them, never from Python files; until
    the cutover both builds ship the same Codex, Claude Code and extension minimum."""
    from ferb.act.browser.autoinstall import STORE_EXTENSION_ID
    from ferb.act.browser.ext_protocol import MIN_EXTENSION_VERSION
    from ferb.runtimes.claude_sdk import CLAUDE_CODE_VERSION
    from ferb.runtimes.codex_exec import CODEX_VERSION
    pins = _vars("ferb_rust_pins $ROOT", "FERB_VERSION", "FERB_CODEX_VERSION", "FERB_CLAUDE_VERSION",
                 "FERB_MIN_EXTENSION", "FERB_STORE_EXTENSION")
    assert pins["FERB_CODEX_VERSION"] == CODEX_VERSION
    assert pins["FERB_CLAUDE_VERSION"] == CLAUDE_CODE_VERSION
    assert pins["FERB_MIN_EXTENSION"] == MIN_EXTENSION_VERSION
    assert pins["FERB_STORE_EXTENSION"] == STORE_EXTENSION_ID
    import tomllib
    workspace = tomllib.loads((ROOT / "rust" / "Cargo.toml").read_text())["workspace"]["package"]["version"]
    assert pins["FERB_VERSION"] == workspace
    # ferbd's own copy of the Codex pin is the runtimes crate's
    again = _zsh("ferb_rust_const $ROOT/rust/crates/ferbd/src/compose/runtimes.rs CODEX_VERSION")
    assert again.stdout.strip() == CODEX_VERSION
    # the release checks read the same constants for a Rust app, Python's for a Python app
    for engine in ("python", "rust"):
        got = _vars(f"ferb_release_constants $ROOT {engine}", "FERB_STORE_EXTENSION", "FERB_MIN_EXTENSION")
        assert (got["FERB_STORE_EXTENSION"], got["FERB_MIN_EXTENSION"]) == (STORE_EXTENSION_ID, MIN_EXTENSION_VERSION)


def test_a_constant_is_read_only_when_named_exactly_once(tmp_path: Path) -> None:
    src = tmp_path / "pins.rs"
    src.write_text('pub const A: &str = "1.2.3";\npub const B: &str = "x";\npub const B: &str = "y";\n')
    assert _zsh(f"ferb_rust_const {src} A").stdout.strip() == "1.2.3"
    assert _zsh(f"ferb_rust_const {src} B").returncode != 0               # two: never a guess
    assert _zsh(f"ferb_rust_const {src} C").returncode != 0
    assert _zsh(f"ferb_rust_const {tmp_path / 'none.rs'} A").returncode != 0


# decision: ENG-7
def test_claude_code_is_locked_like_codex_for_both_architectures() -> None:
    """rust/packaging pins npm's Claude Code at the version the engine checks, each Mac's platform package with its
    registry integrity, as package-lock.json pins Codex."""
    from ferb.runtimes.claude_sdk import CLAUDE_CODE_VERSION
    manifest = json.loads((ROOT / "rust/packaging/package.json").read_text())
    assert manifest["devDependencies"] == {"@anthropic-ai/claude-code": CLAUDE_CODE_VERSION}
    lock = json.loads((ROOT / "rust/packaging/package-lock.json").read_text())["packages"]
    for cpu in ("arm64", "x64"):
        entry = lock[f"node_modules/@anthropic-ai/claude-code-darwin-{cpu}"]
        assert entry["version"] == CLAUDE_CODE_VERSION and entry["integrity"].startswith("sha512-")
        assert entry["resolved"].startswith("https://registry.npmjs.org/@anthropic-ai/claude-code-darwin-")
        assert entry["os"] == ["darwin"] and entry["optional"] is True


def test_the_rust_bundles_places_are_the_ones_the_engine_names() -> None:
    """The agent, native host, `ferb` program and Claude Code sit where ferb-device and ferbd look for them."""
    install = CRATES / "ferb-device/src/install.rs"
    named = _vars("ferb_rust_arch_names arm64", "FERB_RUST_AGENT", "FERB_RUST_NATIVE_HOST", "FERB_RUST_CLI",
                  "FERB_CLAUDE_BIN", "FERB_RUST_TARGET")
    const = lambda name: _zsh(f"ferb_rust_const {install} {name}").stdout.strip()   # noqa: E731
    assert "Contents/" + named["FERB_RUST_AGENT"] == const("BUNDLE_AGENT")
    assert "Contents/" + named["FERB_RUST_NATIVE_HOST"] == const("BUNDLE_RUST_NATIVE_HOST")
    assert "Contents/" + named["FERB_RUST_CLI"] == const("BUNDLE_CLI")
    runtimes = (CRATES / "ferbd/src/compose/runtimes.rs").read_text()
    assert 'format!("node/node_modules/@anthropic-ai/{package}/claude")' in runtimes
    assert named["FERB_CLAUDE_BIN"] == "Resources/node/node_modules/@anthropic-ai/claude-code-darwin-arm64/claude"
    assert named["FERB_RUST_TARGET"] == "aarch64-apple-darwin"
    intel = _vars("ferb_rust_arch_names x86_64", "FERB_CLAUDE_BIN", "FERB_RUST_TARGET", "FERB_CODEX_BIN")
    assert intel["FERB_CLAUDE_BIN"].endswith("claude-code-darwin-x64/claude")
    assert intel["FERB_RUST_TARGET"] == "x86_64-apple-darwin"
    assert _zsh("ferb_rust_arch_names ppc").returncode != 0


def test_the_app_carries_every_prompt_the_engine_loads_and_no_python() -> None:
    """Every runtime Markdown path a Rust crate loads by name is in the list the build copies byte for byte, and the
    list holds Markdown only."""
    import re
    listed = set(_zsh("ferb_rust_runtime_markdown $ROOT").stdout.split())
    assert listed and all(p.endswith(".md") for p in listed)
    assert "orchestrator/prompts/supervisor.md" in listed and "abilities/google_docs/SKILL.md" in listed
    loaded = set()
    for src in CRATES.glob("*/src/**/*.rs"):
        loaded |= set(re.findall(r'"((?:memory|orchestrator/prompts)/[A-Za-z_/]+\.md)"', src.read_text()))
    assert loaded and loaded <= listed, sorted(loaded - listed)


def test_the_model_pin_is_ferb_memorys() -> None:
    from ferb.memory.embeddings import MODEL
    got = _zsh("ferb_model_pin $ROOT && print -r -- $FERB_MODEL_REPOSITORY $FERB_MODEL_REVISION && print -l -- $FERB_MODEL_FILES")
    lines = got.stdout.splitlines()
    assert lines[0] == f"{MODEL['repository']} {MODEL['revision']}"
    assert dict(line.split(" ") for line in lines[1:]) == MODEL["files"]          # the same files, the same SHA-256s


def test_python_left_names_every_python_part(tmp_path: Path) -> None:
    clean = tmp_path / "clean/FERB.app"
    compile_macho(clean / "Contents/MacOS/FERBNotch", "arm64")
    (clean / "Contents/Resources/ferb/memory").mkdir(parents=True)
    (clean / "Contents/Resources/ferb/memory/profile.md").write_text("x")
    assert _zsh(f"ferb_rust_python_left {clean}").stdout == ""
    dirty = tmp_path / "dirty/FERB.app"
    shutil.copytree(clean, dirty)
    (dirty / "Contents/Resources/ferb/abilities").mkdir()
    (dirty / "Contents/Resources/ferb/abilities/tools.py").write_text("x")
    (dirty / "Contents/Resources/lib/__pycache__").mkdir(parents=True)
    (dirty / "Contents/Resources/lib/site-packages").mkdir()
    compile_macho(dirty / "Contents/Resources/python/bin/python3.12", "arm64")
    left = _zsh(f"ferb_rust_python_left {dirty}").stdout.splitlines()
    for part in ("Contents/Resources/ferb/abilities/tools.py", "Contents/Resources/lib/__pycache__",
                 "Contents/Resources/lib/site-packages", "Contents/Resources/python"):
        assert part in left, left


def test_rust_trim_drops_codexs_unused_parts_and_keeps_one_architecture(tmp_path: Path) -> None:
    app = tmp_path / "FERB.app"
    vendor = app / "Contents/Resources/node/node_modules/@openai/codex-darwin-arm64/vendor/aarch64-apple-darwin"
    for rel in ("bin/codex-code-mode-host", "codex-resources/voice/x", "codex-resources/zsh/bin/zsh", "codex-path/rg"):
        (vendor / rel).parent.mkdir(parents=True, exist_ok=True)
        (vendor / rel).write_text("x")
    fat = compile_macho(app / "Contents/Helpers/ferb-native-host", "arm64", "x86_64")
    done = _zsh(f"ferb_rust_trim {app} arm64")
    assert done.returncode == 0, done.stderr
    assert not (vendor / "bin/codex-code-mode-host").exists() and not (vendor / "codex-resources/voice").exists()
    assert (vendor / "codex-path/rg").exists()
    assert subprocess.run(["lipo", "-archs", str(fat)], capture_output=True, text=True).stdout.split() == ["arm64"]


def test_the_build_refuses_an_unknown_architecture(tmp_path: Path) -> None:
    done = subprocess.run([str(BUILD), "--arch", "ppc", str(tmp_path / "out")], capture_output=True, text=True,
                          env={"PATH": "/usr/bin:/bin:/usr/sbin:/sbin", "HOME": str(tmp_path)})
    assert done.returncode != 0 and "--arch is arm64 or x86_64" in done.stderr
    assert not (tmp_path / "out").exists()


def test_the_notices_name_every_rust_crate_the_app_ships() -> None:
    """THIRD_PARTY_NOTICES.md's generated section is what scripts/rust_notices.py writes from the Cargo.lock files
    (needs cargo and its offline registry cache; skipped without)."""
    cargo = shutil.which("cargo") or str(Path.home() / ".cargo/bin/cargo")
    if not Path(cargo).exists():
        pytest.skip("needs cargo")
    env = {**os.environ, "PATH": f"{Path(cargo).parent}:{os.environ.get('PATH', '')}"}
    done = subprocess.run(["python3", "-I", str(ROOT / "scripts/rust_notices.py"), "--check"], capture_output=True,
                          text=True, env=env, timeout=300)
    if done.returncode != 0 and "out of date" not in done.stderr:
        pytest.skip("cargo metadata couldn't run offline: " + done.stderr[-300:])
    assert done.returncode == 0, done.stderr
    assert "Cua AI, Inc." in (ROOT / "THIRD_PARTY_NOTICES.md").read_text()          # earlier entries stay


# ---------- a built Rust bundle (FERB_RUST_APP_BUNDLE=/path/FERB.app; scripts/build-app-rust.sh) ----------

@pytest.fixture(scope="module")
def rust_bundle() -> Path:
    given = os.environ.get("FERB_RUST_APP_BUNDLE")
    if not given:
        pytest.skip("set FERB_RUST_APP_BUNDLE to a FERB.app built by scripts/build-app-rust.sh")
    app = Path(given).resolve()
    assert (app / "Contents/Info.plist").is_file(), app
    return app


def test_rust_bundle_holds_no_python_and_every_piece_is_signed(rust_bundle: Path) -> None:
    app = rust_bundle
    assert _zsh(f"ferb_rust_python_left {shlex.quote(str(app))}").stdout == ""
    verify = subprocess.run(["codesign", "--verify", "--deep", "--strict", str(app)], capture_output=True, text=True)
    assert verify.returncode == 0, verify.stderr
    signed = _zsh(f"ferb_check_signed_code {shlex.quote(str(app))}")
    assert signed.returncode == 0, signed.stderr
    c = app / "Contents"
    for rel in ("MacOS/FERBNotch", "Helpers/FERB.app/Contents/MacOS/FERB", "Helpers/ferb-native-host",
                "Resources/bin/ferb", "Helpers/ferb-secrets", "Resources/extensions/chromium/manifest.json",
                "Resources/ferb/orchestrator/prompts/supervisor.md", "Library/LaunchAgents/com.ferb.daemon.plist"):
        assert (c / rel).is_file(), rel
    for rel in _zsh("ferb_rust_runtime_markdown $ROOT").stdout.split():           # byte for byte
        assert (c / "Resources/ferb" / rel).read_bytes() == (ROOT / "ferb" / rel).read_bytes(), rel


# decision: ENG-7
def test_rust_bundles_agent_is_todays_agent_app_with_ferbd_inside(rust_bundle: Path) -> None:
    """The nested app keeps the id, names and usage descriptions macOS attaches FERB's permissions to, and its
    service definition starts `FERB daemon`."""
    agent = rust_bundle / "Contents/Helpers/FERB.app"
    info = plistlib.loads((agent / "Contents/Info.plist").read_bytes())
    want = plistlib.loads(AGENT_PLIST.read_bytes())
    for key in ("CFBundleShortVersionString", "CFBundleVersion"):
        info.pop(key)
        want.pop(key)
    assert info == want
    shown = subprocess.run(["codesign", "-d", "--entitlements", "-", "--xml", str(agent)], capture_output=True)
    ents = plistlib.loads(shown.stdout) if shown.stdout.strip() else {}
    assert ents.get("com.apple.security.automation.apple-events") is True
    ident = subprocess.run(["codesign", "-dv", str(agent)], capture_output=True, text=True).stderr
    assert "Identifier=com.ferb.daemon" in ident
    service = plistlib.loads((rust_bundle / "Contents/Library/LaunchAgents/com.ferb.daemon.plist").read_bytes())
    assert service["ProgramArguments"] == ["FERB", "daemon"]
    assert service["BundleProgram"] == "Contents/Helpers/FERB.app/Contents/MacOS/FERB"


def _socket_call(path: Path, method: str, params: dict | None = None) -> dict:
    with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as s:
        s.settimeout(10)
        s.connect(str(path))
        f = s.makefile("rwb")
        for i, (m, p) in enumerate((("auth.hello", {"role": "cli"}), (method, params or {}))):
            f.write(json.dumps({"jsonrpc": "2.0", "id": i, "method": m, "params": p}).encode() + b"\n")
            f.flush()
            answer = json.loads(f.readline())
        return answer


# decision: ENG-7
def test_rust_bundles_agent_refuses_and_runs_on_a_development_home(rust_bundle: Path, tmp_path: Path) -> None:
    """Only `daemon`, `mcp` and `grant-probe`; never another FERB_HOME; an ad-hoc build runs on FERB_DEV_HOME (a
    certificate-signed one refuses it), answering ping over its socket and stopping on SIGTERM."""
    agent = rust_bundle / "Contents/Helpers/FERB.app/Contents/MacOS/FERB"
    base = {"PATH": "/usr/bin:/bin", "HOME": str(tmp_path / "user")}
    for argv in ([], ["setup", "--service"], ["daemon", "x"], ["-m", "ferb.daemon"], ["--version"]):
        r = subprocess.run([str(agent), *argv], capture_output=True, text=True, env=base, timeout=30)
        assert r.returncode == 64 and r.stdout == "", argv
    foreign = tmp_path / "foreign"
    foreign.mkdir()
    r = subprocess.run([str(agent), "daemon"], capture_output=True, text=True, timeout=30,
                       env={**base, "FERB_HOME": str(foreign)})
    assert r.returncode == 64 and "FERB_HOME" in r.stderr
    adhoc = "Signature=adhoc" in subprocess.run(["codesign", "-dv", str(agent)], capture_output=True,
                                                 text=True).stderr
    import tempfile
    home = Path(tempfile.mkdtemp(prefix="ferb-"))     # short: a Unix socket path holds about 100 bytes
    try:
        daemon = subprocess.Popen([str(agent), "daemon"], env={**base, "FERB_DEV_HOME": str(home),
                                                               "HTTPS_PROXY": "http://127.0.0.1:9"},
                                  stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        if not adhoc:
            assert daemon.wait(timeout=30) == 64
            return
        sock = home / "run/ferb.sock"
        try:
            deadline = time.monotonic() + 60
            while not sock.exists() and time.monotonic() < deadline and daemon.poll() is None:
                time.sleep(0.2)
            assert sock.exists(), daemon.stderr.read().decode() if daemon.poll() is not None else "no socket"
            answer = _socket_call(sock, "ping")
            assert "result" in answer, answer
            mcp = subprocess.run([str(agent), "mcp"], input=json.dumps(
                {"jsonrpc": "2.0", "id": 1, "method": "initialize",
                 "params": {"protocolVersion": "2025-06-18", "capabilities": {},
                            "clientInfo": {"name": "t", "version": "0"}}}) + "\n",
                capture_output=True, text=True, timeout=30,
                env={**base, "FERB_DEV_HOME": str(home), "FERB_RUN_TOKEN": "not-a-real-token"})
            assert json.loads(mcp.stdout.splitlines()[0])["result"]["serverInfo"], mcp.stdout + mcp.stderr
        finally:
            daemon.send_signal(signal.SIGTERM)
            assert daemon.wait(timeout=60) == 0
        assert not sock.exists()
    finally:
        shutil.rmtree(home, ignore_errors=True)


# decision: REL-424
def test_a_local_developer_id_build_is_signed_by_the_team_but_never_notarized_and_never_shipped(
        tmp_path: Path) -> None:
    """FERB_NOTARIZE=0 with SIGN_IDENTITY=devid: every piece is signed by the team (so macOS keeps the permissions of
    the developer's own Mac across builds), nothing reaches Apple's notary and no credentials are needed. Such an app
    carries no stapled ticket: build-dmg.sh refuses the switch, and ferb_check_devid_app (what an appcast checks)
    still refuses the app."""
    from tests.test_packaging import BUILD_DMG, DEVID_SHA, _devid_env, _sign_only, _stubs

    local = {k: v for k, v in _devid_env().items() if k != "NOTARY_KEYCHAIN_PROFILE"}
    app, calls, done = _sign_only(tmp_path, **local, FERB_NOTARIZE="0")
    assert done.returncode == 0, done.stderr + done.stdout
    sign_calls = [c for c in calls if "|--sign|" in c and "--verify" not in c]
    assert sign_calls and all(f"|--sign|{DEVID_SHA}|" in c and "|--timestamp|" in c for c in sign_calls)
    assert not any(c.startswith("xcrun") for c in calls)                     # nothing sent to Apple, nothing stapled
    assert "Not notarized" in done.stdout
    (tmp_path / "plain").mkdir()
    without = _sign_only(tmp_path / "plain", **local)[2]                     # without the switch: credentials needed
    assert without.returncode != 0 and "NOTARY_KEYCHAIN_PROFILE" in without.stderr

    (tmp_path / "dmg").mkdir()
    env = {**_stubs(tmp_path / "dmg"), **_devid_env(), "FERB_NOTARIZE": "0", "FERB_DMG_DIR": str(tmp_path / "dist")}
    del env["FERB_DEV_DMG"]
    refused = subprocess.run([str(BUILD_DMG), str(app)], capture_output=True, text=True, env=env)
    assert refused.returncode != 0 and "always notarized" in refused.stderr
    assert not (tmp_path / "dist").exists()

    (tmp_path / "check").mkdir()
    check = {**_stubs(tmp_path / "check"), **_devid_env(), "FERB_NOTARIZE": "0", "STUB_STAPLE_MISSING": "1",
             "ROOT": str(ROOT)}
    unstapled = subprocess.run(["/bin/zsh", "-f", "-c", f'source "$ROOT/scripts/signing.sh" && '
                                f'ferb_check_devid_app {shlex.quote(str(app))}'],
                               capture_output=True, text=True, env=check)
    assert unstapled.returncode != 0 and "no stapled notarization ticket" in unstapled.stderr
