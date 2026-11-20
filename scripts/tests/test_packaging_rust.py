"""FERB.app with the Rust engine (engine/PLAN.md part 13; scripts/build-app-rust.sh and scripts/rust-parts.sh): the
pins it reads from the Rust sources, the Claude Code lock, the places in the bundle, the runtime Markdown it carries,
its checks for Python left behind, its size cap, the notices, and (with `FERB_RUST_APP_BUNDLE=/path/FERB.app`) a built
bundle, including its agent's refusals and a daemon on a temporary home through the development override. Ported from
tests/test_packaging_rust.py to stdlib unittest; the pins are checked against the other sources that name them.

Nothing here builds the app, installs, registers a service, runs launchctl or reads ~/.ferb."""

from __future__ import annotations

import base64
import json
import os
import plistlib
import re
import shlex
import shutil
import signal
import socket
import subprocess
import tempfile
import time
import unittest
from pathlib import Path

from scripts.tests.support import (BUILD_APP, BUILD_DMG, CRATES, DEVID_SHA, RELEASE_PIECES, ROOT, RUST_PARTS, ent,
                                   SYSTEM_PATH, TempCase, compile_macho, devid_env, lipo_archs, rust_const, sign_only,
                                   stubs, version_tuple, workspace_version, zsh, zsh_vars)

AGENT_PLIST = ROOT / "apps" / "mac" / "FERBAgent" / "Info.plist"
ARCHS = ("arm64", "x86_64")


class RustBuild(TempCase):
    def test_the_rust_build_scripts_are_executable_zsh_that_parses(self) -> None:
        for script in (BUILD_APP, RUST_PARTS):
            self.assertEqual(subprocess.run(["/bin/zsh", "-n", str(script)], capture_output=True).returncode, 0,
                             script)
        self.assertTrue(os.access(BUILD_APP, os.X_OK))

    # decision: ENG-7
    def test_the_rust_builds_pins_come_from_the_rust_sources_and_name_todays_programs(self) -> None:
        """The Rust build reads its version and pins from the Rust code that enforces them, never from Python files,
        and they agree with the other places that name them: the npm locks, the extension's manifest, the workspace
        version and ferbd's own copy of the Codex pin."""
        pins = zsh_vars(self, "ferb_rust_pins $ROOT", "FERB_VERSION", "FERB_CODEX_VERSION", "FERB_CLAUDE_VERSION",
                        "FERB_MIN_EXTENSION", "FERB_STORE_EXTENSION")
        lock = json.loads((ROOT / "package-lock.json").read_text())["packages"]
        self.assertEqual(lock["node_modules/@openai/codex"]["version"], pins["FERB_CODEX_VERSION"])
        manifest = json.loads((ROOT / "engine/packaging/package.json").read_text())
        self.assertEqual(manifest["devDependencies"]["@anthropic-ai/claude-code"], pins["FERB_CLAUDE_VERSION"])
        extension = json.loads((ROOT / "extension/manifest.json").read_text())["version"]
        self.assertGreaterEqual(version_tuple(extension), version_tuple(pins["FERB_MIN_EXTENSION"]))
        self.assertRegex(pins["FERB_STORE_EXTENSION"], r"^[a-p]{32}$")       # a Chrome extension id
        self.assertEqual(pins["FERB_VERSION"], workspace_version())
        # ferbd's own copy of the Codex pin is the runtimes crate's
        again = zsh("ferb_rust_const $ROOT/engine/crates/ferbd/src/compose/runtimes.rs CODEX_VERSION")
        self.assertEqual(again.stdout.strip(), pins["FERB_CODEX_VERSION"])
        # the release checks read the same constants; a Python app's engine is refused
        got = zsh_vars(self, "ferb_release_constants $ROOT", "FERB_STORE_EXTENSION", "FERB_MIN_EXTENSION")
        self.assertEqual((got["FERB_STORE_EXTENSION"], got["FERB_MIN_EXTENSION"]),
                         (pins["FERB_STORE_EXTENSION"], pins["FERB_MIN_EXTENSION"]))
        self.assertNotEqual(zsh("ferb_release_constants $ROOT python").returncode, 0)

    def test_a_constant_is_read_only_when_named_exactly_once(self) -> None:
        src = self.tmp / "pins.rs"
        src.write_text('pub const A: &str = "1.2.3";\npub const B: &str = "x";\npub const B: &str = "y";\n')
        self.assertEqual(zsh(f"ferb_rust_const {src} A").stdout.strip(), "1.2.3")
        self.assertNotEqual(zsh(f"ferb_rust_const {src} B").returncode, 0)    # two: never a guess
        self.assertNotEqual(zsh(f"ferb_rust_const {src} C").returncode, 0)
        self.assertNotEqual(zsh(f"ferb_rust_const {self.tmp / 'none.rs'} A").returncode, 0)

    # decision: ENG-7
    def test_claude_code_is_locked_like_codex_for_both_architectures(self) -> None:
        """engine/packaging pins npm's Claude Code at the version the engine checks, each Mac's platform package with its
        registry integrity, as package-lock.json pins Codex."""
        version = rust_const(CRATES / "ferb-runtimes/src/claude/transport.rs", "CLAUDE_CODE_VERSION")
        manifest = json.loads((ROOT / "engine/packaging/package.json").read_text())
        self.assertEqual(manifest["devDependencies"], {"@anthropic-ai/claude-code": version})
        lock = json.loads((ROOT / "engine/packaging/package-lock.json").read_text())["packages"]
        for cpu in ("arm64", "x64"):
            entry = lock[f"node_modules/@anthropic-ai/claude-code-darwin-{cpu}"]
            self.assertTrue(entry["version"] == version and entry["integrity"].startswith("sha512-"), cpu)
            self.assertTrue(entry["resolved"].startswith("https://registry.npmjs.org/@anthropic-ai/claude-code-darwin-"))
            self.assertTrue(entry["os"] == ["darwin"] and entry["optional"] is True, cpu)

    def test_codex_is_the_pinned_package_for_each_architecture(self) -> None:
        """package-lock.json pins each Mac's Codex platform package at ferb-runtimes' CODEX_VERSION, with its registry
        integrity (from tests/test_packaging.py's codex pin test)."""
        version = rust_const(CRATES / "ferb-runtimes/src/codex/mod.rs", "CODEX_VERSION")
        lock = json.loads((ROOT / "package-lock.json").read_text())["packages"]
        for cpu in ("arm64", "x64"):
            entry = lock[f"node_modules/@openai/codex-darwin-{cpu}"]
            self.assertTrue(entry["version"] == f"{version}-darwin-{cpu}" and entry["integrity"].startswith("sha512-"),
                            cpu)

    def test_the_rust_bundles_places_are_the_ones_the_engine_names(self) -> None:
        """The agent, native host, `ferb` program and Claude Code sit where ferb-device and ferbd look for them."""
        install = CRATES / "ferb-device/src/install.rs"
        named = zsh_vars(self, "ferb_rust_arch_names arm64", "FERB_RUST_AGENT", "FERB_RUST_NATIVE_HOST",
                         "FERB_RUST_CLI", "FERB_CLAUDE_BIN", "FERB_RUST_TARGET")

        def const(name: str) -> str:
            return zsh(f"ferb_rust_const {install} {name}").stdout.strip()

        self.assertEqual("Contents/" + named["FERB_RUST_AGENT"], const("BUNDLE_AGENT"))
        self.assertEqual("Contents/" + named["FERB_RUST_NATIVE_HOST"], const("BUNDLE_RUST_NATIVE_HOST"))
        self.assertEqual("Contents/" + named["FERB_RUST_CLI"], const("BUNDLE_CLI"))
        # the phone link's helper: where the daemon starts it (ferbd compose/link.rs BUNDLED) and accepts it by its
        # signature (ferb-rpc link_peer.rs HELPER)
        link = zsh_vars(self, "ferb_rust_arch_names arm64", "FERB_RUST_LINK")["FERB_RUST_LINK"]
        self.assertEqual("Contents/" + link,
                         zsh(f"ferb_rust_const {CRATES / 'ferbd/src/compose/link.rs'} BUNDLED").stdout.strip())
        self.assertEqual("Contents/" + link,
                         zsh(f"ferb_rust_const {CRATES / 'ferb-rpc/src/link_peer.rs'} HELPER").stdout.strip())
        runtimes = (CRATES / "ferbd/src/compose/runtimes.rs").read_text()
        self.assertIn('format!("node/node_modules/@anthropic-ai/{package}/claude")', runtimes)
        self.assertEqual(named["FERB_CLAUDE_BIN"], "Resources/node/node_modules/@anthropic-ai/claude-code-darwin-arm64/claude")
        self.assertEqual(named["FERB_RUST_TARGET"], "aarch64-apple-darwin")
        intel = zsh_vars(self, "ferb_rust_arch_names x86_64", "FERB_CLAUDE_BIN", "FERB_RUST_TARGET", "FERB_CODEX_BIN")
        self.assertTrue(intel["FERB_CLAUDE_BIN"].endswith("claude-code-darwin-x64/claude"))
        self.assertEqual(intel["FERB_RUST_TARGET"], "x86_64-apple-darwin")
        self.assertNotEqual(zsh("ferb_rust_arch_names ppc").returncode, 0)

    def test_the_app_carries_every_prompt_the_engine_loads_and_no_python(self) -> None:
        """Every runtime Markdown path a Rust crate loads by name is in the list the build copies byte for byte, and
        the list holds Markdown only."""
        listed = set(zsh("ferb_rust_runtime_markdown $ROOT").stdout.split())
        self.assertTrue(listed and all(p.endswith(".md") for p in listed))
        self.assertTrue("orchestrator/prompts/supervisor.md" in listed and "abilities/google_docs/SKILL.md" in listed)
        loaded: set = set()
        for src in CRATES.glob("*/src/**/*.rs"):
            loaded |= set(re.findall(r'"((?:memory|orchestrator/prompts)/[A-Za-z_/]+\.md)"', src.read_text()))
        self.assertTrue(loaded and loaded <= listed, sorted(loaded - listed))

    def test_the_model_pin_is_ferb_memorys(self) -> None:
        """ferb_model_pin reads ferb-memory's embeddings.rs: the same repository, revision, files and SHA-256s, read
        here independently of the shell's parsing."""
        src = (CRATES / "ferb-memory/src/embeddings.rs").read_text()
        repository = rust_const(CRATES / "ferb-memory/src/embeddings.rs", "MODEL_REPOSITORY")
        revision = rust_const(CRATES / "ferb-memory/src/embeddings.rs", "MODEL_REVISION")
        block = re.search(r"pub const MODEL_FILES: \[\(&str, &str\); (\d+)\] = \[(.*?)\n\];", src, re.S)
        self.assertIsNotNone(block)
        files = dict(re.findall(r'\(\s*"([^"]+)",\s*"([0-9a-f]{64})",?\s*\)', block.group(2)))
        self.assertEqual(len(files), int(block.group(1)))
        got = zsh("ferb_model_pin $ROOT && print -r -- $FERB_MODEL_REPOSITORY $FERB_MODEL_REVISION "
                  "&& print -l -- $FERB_MODEL_FILES")
        lines = got.stdout.splitlines()
        self.assertEqual(lines[0], f"{repository} {revision}", got.stderr)
        self.assertEqual(dict(line.split(" ") for line in lines[1:]), files)   # the same files, the same SHA-256s

    def test_python_left_names_every_python_part(self) -> None:
        clean = self.tmp / "clean/FERB.app"
        compile_macho(clean / "Contents/MacOS/FERBNotch", "arm64")
        (clean / "Contents/Resources/ferb/memory").mkdir(parents=True)
        (clean / "Contents/Resources/ferb/memory/profile.md").write_text("x")
        self.assertEqual(zsh(f"ferb_rust_python_left {clean}").stdout, "")
        dirty = self.tmp / "dirty/FERB.app"
        shutil.copytree(clean, dirty)
        (dirty / "Contents/Resources/ferb/abilities").mkdir()
        (dirty / "Contents/Resources/ferb/abilities/tools.py").write_text("x")
        (dirty / "Contents/Resources/lib/__pycache__").mkdir(parents=True)
        (dirty / "Contents/Resources/lib/site-packages").mkdir()
        compile_macho(dirty / "Contents/Resources/python/bin/python3.12", "arm64")
        left = zsh(f"ferb_rust_python_left {dirty}").stdout.splitlines()
        for part in ("Contents/Resources/ferb/abilities/tools.py", "Contents/Resources/lib/__pycache__",
                     "Contents/Resources/lib/site-packages", "Contents/Resources/python"):
            self.assertIn(part, left)

    def test_rust_trim_drops_codexs_unused_parts_and_keeps_one_architecture(self) -> None:
        app = self.tmp / "FERB.app"
        vendor = app / "Contents/Resources/node/node_modules/@openai/codex-darwin-arm64/vendor/aarch64-apple-darwin"
        for rel in ("bin/codex-code-mode-host", "codex-resources/voice/x", "codex-resources/zsh/bin/zsh",
                    "codex-path/rg"):
            (vendor / rel).parent.mkdir(parents=True, exist_ok=True)
            (vendor / rel).write_text("x")
        fat = compile_macho(app / "Contents/Helpers/ferb-native-host", "arm64", "x86_64")
        done = zsh(f"ferb_rust_trim {app} arm64")
        self.assertEqual(done.returncode, 0, done.stderr)
        self.assertFalse((vendor / "bin/codex-code-mode-host").exists() or (vendor / "codex-resources/voice").exists())
        self.assertFalse((vendor / "codex-resources/zsh").exists())
        self.assertTrue((vendor / "codex-path/rg").exists())
        self.assertEqual(lipo_archs(fat), ["arm64"])

    def test_the_build_refuses_an_unknown_architecture(self) -> None:
        done = subprocess.run([str(BUILD_APP), "--arch", "ppc", str(self.tmp / "out")], capture_output=True,
                              text=True, env={"PATH": SYSTEM_PATH, "HOME": str(self.tmp)})
        self.assertTrue(done.returncode != 0 and "--arch is arm64 or x86_64" in done.stderr, done.stderr)
        self.assertFalse((self.tmp / "out").exists())

    def test_the_rust_size_report_lists_the_apps_parts_largest_first(self) -> None:
        """ferb_rust_size_report (what build-app-rust.sh prints last): each part in MB of 10^6 bytes, the largest
        first, and the rest as "Everything else" (from tests/test_packaging.py's report test, for the Rust app)."""
        app = self.tmp / "FERB.app"
        c = app / "Contents"
        sizes = {"Resources/node/node_modules/@anthropic-ai/claude-code-darwin-arm64/claude": 4_000_000,
                 "Resources/node/node_modules/@openai/codex-darwin-arm64/codex": 3_000_000,
                 "Helpers/FERB.app/Contents/MacOS/FERB": 2_000_000, "Resources/bin/ferb": 900_000,
                 "Helpers/ferb-native-host": 300_000, "Helpers/ferb-secrets": 100_000,
                 "Resources/models/embeddings/m/model.safetensors": 1_500_000,
                 "Frameworks/Sparkle.framework/Sparkle": 600_000, "Resources/ferb/memory/profile.md": 150_000,
                 "Resources/extensions/chromium/background.js": 50_000, "Info.plist": 1_000}
        for rel, size in sizes.items():
            (c / rel).parent.mkdir(parents=True, exist_ok=True)
            (c / rel).write_bytes(b"\0" * size)
        notch = compile_macho(c / "MacOS/FERBNotch", "arm64").stat().st_size
        done = zsh(f"ferb_rust_size_report {shlex.quote(str(app))}")
        self.assertEqual(done.returncode, 0, done.stderr)
        lines = done.stdout.splitlines()
        self.assertEqual(lines[0], f"FERB.app (arm64, Rust): {(sum(sizes.values()) + notch) / 1e6:.1f} MB")
        rows = [line.split(" MB  ", 1) for line in lines[1:]]
        named = {name: float(mb) for mb, name in rows}
        self.assertEqual(named, {"Claude Code (Resources/node/@anthropic-ai)": 4.0, "Codex (Resources/node/@openai)": 3.0,
                                 "Daemon: ferbd as the agent app (Helpers/FERB.app)": 2.0,
                                 "ferb program (Resources/bin)": 0.9, "Helpers (native host, secrets, phone link)": 0.4,
                                 "Embedding model (Resources/models)": 1.5, "Notch app (MacOS)": round(notch / 1e6, 1),
                                 "Sparkle (Frameworks)": 0.6,
                                 "Runtime Markdown and extension (Resources/ferb, extensions)": 0.2,
                                 "Everything else": 0.0})
        mbs = [float(mb) for mb, _ in rows]
        self.assertEqual(mbs, sorted(mbs, reverse=True))

    # decision: REL-429
    def test_the_build_reports_the_size_and_has_no_size_cap(self) -> None:
        """The owner dropped the size cap (REL-429): build-app-rust.sh prints the size report and nothing refuses an
        app for its size."""
        script = BUILD_APP.read_text()
        self.assertIn('ferb_rust_size_report "$APP"', script)
        self.assertNotIn("size_cap", script)
        self.assertNotEqual(zsh("typeset -f ferb_rust_size_cap").returncode, 0)

    def test_the_notices_name_every_rust_crate_the_app_ships(self) -> None:
        """THIRD_PARTY_NOTICES.md's generated section is what scripts/rust_notices.py writes from the Cargo.lock files
        (needs cargo and its offline registry cache; skipped without)."""
        cargo = shutil.which("cargo") or str(Path.home() / ".cargo/bin/cargo")
        if not Path(cargo).exists():
            raise unittest.SkipTest("needs cargo")
        env = {**os.environ, "PATH": f"{Path(cargo).parent}:{os.environ.get('PATH', '')}"}
        done = subprocess.run(["python3", "-I", str(ROOT / "scripts/rust_notices.py"), "--check"], capture_output=True,
                              text=True, env=env, timeout=300)
        if done.returncode != 0 and "out of date" not in done.stderr:
            raise unittest.SkipTest("cargo metadata couldn't run offline: " + done.stderr[-300:])
        self.assertEqual(done.returncode, 0, done.stderr)
        self.assertIn("Cua AI, Inc.", (ROOT / "THIRD_PARTY_NOTICES.md").read_text())   # earlier entries stay

    # decision: REL-424
    def test_a_local_developer_id_build_is_signed_by_the_team_but_never_notarized_and_never_shipped(self) -> None:
        """FERB_NOTARIZE=0 with SIGN_IDENTITY=devid: every piece is signed by the team (so macOS keeps the permissions
        of the developer's own Mac across builds), nothing reaches Apple's notary and no credentials are needed. Such
        an app carries no stapled ticket: build-dmg.sh refuses the switch, and ferb_check_devid_app (what an appcast
        checks) still refuses the app."""
        local = {k: v for k, v in devid_env().items() if k != "NOTARY_KEYCHAIN_PROFILE"}
        app, calls, done = sign_only(self.tmp, **local, FERB_NOTARIZE="0")
        self.assertEqual(done.returncode, 0, done.stderr + done.stdout)
        sign_calls = [c for c in calls if "|--sign|" in c and "--verify" not in c]
        self.assertTrue(sign_calls and all(f"|--sign|{DEVID_SHA}|" in c and "|--timestamp|" in c for c in sign_calls))
        self.assertFalse(any(c.startswith("xcrun") for c in calls))           # nothing sent to Apple, nothing stapled
        self.assertIn("Not notarized", done.stdout)
        # the phone link's helper: its own identifier, network entitlements only, hardened like every piece (PHN-441)
        helper = app / "Contents/Helpers/ferb-link"
        call = next(c for c in sign_calls if c.endswith(f"|{helper}"))
        self.assertIn("|--identifier|com.ferb.link|", call)
        self.assertIn("|--options|runtime|", call)
        given = ent(self.tmp, helper)
        self.assertIn("com.apple.security.network.client", given)
        self.assertIn("com.apple.security.network.server", given)
        self.assertEqual(given.count("<key>"), 2, given)
        (self.tmp / "plain").mkdir()
        without = sign_only(self.tmp / "plain", **local)[2]                   # without the switch: credentials needed
        self.assertTrue(without.returncode != 0 and "NOTARY_KEYCHAIN_PROFILE" in without.stderr)

        (self.tmp / "dmg").mkdir()
        env = {**stubs(self.tmp / "dmg"), **devid_env(), "FERB_NOTARIZE": "0", "FERB_DMG_DIR": str(self.tmp / "dist")}
        del env["FERB_DEV_DMG"]
        refused = subprocess.run([str(BUILD_DMG), str(app)], capture_output=True, text=True, env=env)
        self.assertTrue(refused.returncode != 0 and "always notarized" in refused.stderr, refused.stderr)
        self.assertFalse((self.tmp / "dist").exists())

        (self.tmp / "check").mkdir()
        check = {**stubs(self.tmp / "check"), **devid_env(), "FERB_NOTARIZE": "0", "STUB_STAPLE_MISSING": "1",
                 "ROOT": str(ROOT)}
        unstapled = subprocess.run(["/bin/zsh", "-f", "-c", f'source "$ROOT/scripts/signing.sh" && '
                                    f'ferb_check_devid_app {shlex.quote(str(app))}'],
                                   capture_output=True, text=True, env=check)
        self.assertTrue(unstapled.returncode != 0 and "no stapled notarization ticket" in unstapled.stderr)


# ---------- a built Rust bundle (FERB_RUST_APP_BUNDLE=/path/FERB.app; scripts/build-app-rust.sh) ----------

def _rust_bundle() -> Path:
    given = os.environ.get("FERB_RUST_APP_BUNDLE")
    if not given:
        raise unittest.SkipTest("set FERB_RUST_APP_BUNDLE to a FERB.app built by scripts/build-app-rust.sh")
    app = Path(given).resolve()
    if not (app / "Contents/Info.plist").is_file():
        raise AssertionError(f"{app} isn't an app bundle")
    return app


def _designated_requirement(codesign_output: str) -> str:
    """The designated requirement from `codesign -d -r-`; ad-hoc code reports its implicit one as a comment."""
    for line in codesign_output.splitlines():
        for prefix in ("designated =>", "# designated =>"):
            if line.startswith(prefix):
                return line[len(prefix):].strip()
    return ""


def _socket_call(path: Path, method: str, params: dict | None = None) -> dict:
    with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as s:
        s.settimeout(10)
        s.connect(str(path))
        f = s.makefile("rwb")
        answer: dict = {}
        for i, (m, p) in enumerate((("auth.hello", {"role": "cli"}), (method, params or {}))):
            f.write(json.dumps({"jsonrpc": "2.0", "id": i, "method": m, "params": p}).encode() + b"\n")
            f.flush()
            answer = json.loads(f.readline())
        return answer


class BuiltRustBundle(TempCase):
    def test_rust_bundle_holds_no_python_and_every_piece_is_signed(self) -> None:
        app = _rust_bundle()
        self.assertEqual(zsh(f"ferb_rust_python_left {shlex.quote(str(app))}").stdout, "")
        verify = subprocess.run(["codesign", "--verify", "--deep", "--strict", str(app)], capture_output=True,
                                text=True)
        self.assertEqual(verify.returncode, 0, verify.stderr)
        signed = zsh(f"ferb_check_signed_code {shlex.quote(str(app))}")
        self.assertEqual(signed.returncode, 0, signed.stderr)
        c = app / "Contents"
        for rel in ("MacOS/FERBNotch", "Helpers/FERB.app/Contents/MacOS/FERB", "Helpers/ferb-native-host",
                    "Resources/bin/ferb", "Helpers/ferb-secrets", "Helpers/ferb-link", "Helpers/ferb-power",
                    "Library/LaunchDaemons/com.ferb.power.plist",
                    "Resources/extensions/chromium/manifest.json",
                    "Resources/ferb/orchestrator/prompts/supervisor.md", "Library/LaunchAgents/com.ferb.daemon.plist"):
            self.assertTrue((c / rel).is_file(), rel)
        for rel in zsh("ferb_rust_runtime_markdown $ROOT").stdout.split():   # byte for byte
            self.assertEqual((c / "Resources/ferb" / rel).read_bytes(), (ROOT / "engine" / "runtime" / rel).read_bytes(), rel)

    # decision: ENG-7
    def test_rust_bundles_agent_is_todays_agent_app_with_ferbd_inside(self) -> None:
        """The nested app keeps the id, names and usage descriptions macOS attaches FERB's permissions to, and its
        service definition starts `FERB daemon`."""
        bundle = _rust_bundle()
        agent = bundle / "Contents/Helpers/FERB.app"
        info = plistlib.loads((agent / "Contents/Info.plist").read_bytes())
        want = plistlib.loads(AGENT_PLIST.read_bytes())
        for key in ("CFBundleShortVersionString", "CFBundleVersion"):
            info.pop(key)
            want.pop(key)
        self.assertEqual(info, want)
        shown = subprocess.run(["codesign", "-d", "--entitlements", "-", "--xml", str(agent)], capture_output=True)
        ents = plistlib.loads(shown.stdout) if shown.stdout.strip() else {}
        self.assertIs(ents.get("com.apple.security.automation.apple-events"), True)
        ident = subprocess.run(["codesign", "-dv", str(agent)], capture_output=True, text=True).stderr
        self.assertIn("Identifier=com.ferb.daemon", ident)
        service = plistlib.loads((bundle / "Contents/Library/LaunchAgents/com.ferb.daemon.plist").read_bytes())
        self.assertEqual(service["ProgramArguments"], ["FERB", "daemon"])
        self.assertEqual(service["BundleProgram"], "Contents/Helpers/FERB.app/Contents/MacOS/FERB")

    # decision: ENG-7
    def test_rust_bundles_agent_refuses_and_runs_on_a_development_home(self) -> None:
        """Only `daemon`, `mcp` and `grant-probe`; never another FERB_HOME; an ad-hoc build runs on FERB_DEV_HOME (a
        certificate-signed one refuses it), answering ping over its socket and stopping on SIGTERM."""
        agent = _rust_bundle() / "Contents/Helpers/FERB.app/Contents/MacOS/FERB"
        base = {"PATH": "/usr/bin:/bin", "HOME": str(self.tmp / "user")}
        for argv in ([], ["setup", "--service"], ["daemon", "x"], ["-m", "ferb.daemon"], ["--version"]):
            r = subprocess.run([str(agent), *argv], capture_output=True, text=True, env=base, timeout=30)
            self.assertTrue(r.returncode == 64 and r.stdout == "", argv)
        foreign = self.tmp / "foreign"
        foreign.mkdir()
        r = subprocess.run([str(agent), "daemon"], capture_output=True, text=True, timeout=30,
                           env={**base, "FERB_HOME": str(foreign)})
        self.assertTrue(r.returncode == 64 and "FERB_HOME" in r.stderr)
        adhoc = "Signature=adhoc" in subprocess.run(["codesign", "-dv", str(agent)], capture_output=True,
                                                     text=True).stderr
        home = Path(tempfile.mkdtemp(prefix="ferb-"))     # short: a Unix socket path holds about 100 bytes
        try:
            daemon = subprocess.Popen([str(agent), "daemon"], env={**base, "FERB_DEV_HOME": str(home),
                                                                   "HTTPS_PROXY": "http://127.0.0.1:9"},
                                      stdout=subprocess.PIPE, stderr=subprocess.PIPE)
            if not adhoc:
                self.assertEqual(daemon.wait(timeout=30), 64)
                return
            sock = home / "run/ferb.sock"
            try:
                deadline = time.monotonic() + 60
                while not sock.exists() and time.monotonic() < deadline and daemon.poll() is None:
                    time.sleep(0.2)
                self.assertTrue(sock.exists(),
                                daemon.stderr.read().decode() if daemon.poll() is not None else "no socket")
                answer = _socket_call(sock, "ping")
                self.assertIn("result", answer, answer)
                mcp = subprocess.run([str(agent), "mcp"], input=json.dumps(
                    {"jsonrpc": "2.0", "id": 1, "method": "initialize",
                     "params": {"protocolVersion": "2025-06-18", "capabilities": {},
                                "clientInfo": {"name": "t", "version": "0"}}}) + "\n",
                    capture_output=True, text=True, timeout=30,
                    env={**base, "FERB_DEV_HOME": str(home), "FERB_RUN_TOKEN": "not-a-real-token"})
                self.assertTrue(json.loads(mcp.stdout.splitlines()[0])["result"]["serverInfo"], mcp.stdout + mcp.stderr)
            finally:
                daemon.send_signal(signal.SIGTERM)
                self.assertEqual(daemon.wait(timeout=60), 0)
            self.assertFalse(sock.exists())
        finally:
            shutil.rmtree(home, ignore_errors=True)

    # decision: PHN-441
    # decision: PHN-468
    def test_rust_bundles_phone_link_helper_is_signed_as_the_daemon_checks_it(self) -> None:
        """Contents/Helpers/ferb-link: signed `com.ferb.link`, hardened, with the network entitlements alone, one
        architecture as the daemon's, and its designated requirement names that identifier (what ferb-rpc link_peer.rs
        derives from it on disk and checks the running helper against)."""
        bundle = _rust_bundle()
        helper = bundle / "Contents/Helpers/ferb-link"
        details = subprocess.run(["codesign", "-dv", "--verbose=2", str(helper)], capture_output=True,
                                 text=True).stderr
        self.assertIn("Identifier=com.ferb.link", details)
        self.assertRegex(details, r"flags=0x[0-9a-f]+\([a-z,]*runtime")          # hardened (ad hoc: "adhoc,runtime")
        shown = subprocess.run(["codesign", "-d", "--entitlements", "-", "--xml", str(helper)], capture_output=True)
        ents = plistlib.loads(shown.stdout) if shown.stdout.strip() else {}
        self.assertEqual(ents, {"com.apple.security.network.client": True, "com.apple.security.network.server": True})
        shown = subprocess.run(["codesign", "-d", "-r-", str(helper)], capture_output=True, text=True)
        requirement = _designated_requirement(shown.stdout + shown.stderr)
        if "Signature=adhoc" not in details:
            self.assertTrue(requirement.startswith('identifier "com.ferb.link"'), requirement)
        self.assertEqual(lipo_archs(helper), lipo_archs(bundle / "Contents/Helpers/FERB.app/Contents/MacOS/FERB"))

    # decision: PHN-468
    def test_rust_bundles_daemon_answers_the_link_role_only_to_its_helper(self) -> None:
        """An ad-hoc build's daemon on a temporary home (FERB_DEV_HOME, never ~/.ferb, no service registered): the
        bundle's own helper, started by this test with invented keys on its standard input and no relay but a dead
        local address, says hello as the link and reads the daemon's phones (set_is_link passes for exactly it);
        this test program, naming the same role, gets the unknown-role refusal."""
        bundle = _rust_bundle()
        agent = bundle / "Contents/Helpers/FERB.app/Contents/MacOS/FERB"
        helper = bundle / "Contents/Helpers/ferb-link"
        if "Signature=adhoc" not in subprocess.run(["codesign", "-dv", str(agent)], capture_output=True,
                                                   text=True).stderr:
            raise unittest.SkipTest("a certificate-signed daemon refuses FERB_DEV_HOME: re-sign a copy ad hoc "
                                    "(SIGN_IDENTITY=- scripts/sign-app.sh COPY) to run this")
        base = {"PATH": "/usr/bin:/bin", "HOME": str(self.tmp / "user")}
        home = Path(tempfile.mkdtemp(prefix="ferb-"))
        try:
            daemon = subprocess.Popen([str(agent), "daemon"], env={**base, "FERB_DEV_HOME": str(home),
                                                                   "HTTPS_PROXY": "http://127.0.0.1:9"},
                                      stdout=subprocess.PIPE, stderr=subprocess.PIPE)
            sock = home / "run/ferb.sock"
            link = None
            try:
                deadline = time.monotonic() + 60
                while not sock.exists() and time.monotonic() < deadline and daemon.poll() is None:
                    time.sleep(0.2)
                self.assertTrue(sock.exists(), "no socket")
                self.assertIn("result", _socket_call(sock, "ping"))
                with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as s:   # not the helper: refused
                    s.settimeout(10)
                    s.connect(str(sock))
                    f = s.makefile("rwb")
                    f.write(json.dumps({"jsonrpc": "2.0", "id": 0, "method": "auth.hello",
                                        "params": {"role": "link"}}).encode() + b"\n")
                    f.flush()
                    self.assertIn("error", json.loads(f.readline()))
                keys = json.dumps({"keys": {"identity": base64.b64encode(bytes([1]) * 32).decode(),   # invented
                                             "state-key": base64.b64encode(bytes([2]) * 32).decode()}})
                link = subprocess.Popen([str(helper), "run", "--daemon", str(sock), "--state", str(home / "link-test"),
                                         "--shared-relay", "http://127.0.0.1:9", "--no-portmap"],
                                        env=base, stdin=subprocess.PIPE, stderr=subprocess.PIPE)
                assert link.stdin and link.stderr
                link.stdin.write((keys + "\n").encode())
                link.stdin.flush()
                said = link.stderr.readline().decode()
                self.assertIn("serving phones as", said)
                link.stdin.close()
                self.assertEqual(link.wait(timeout=30), 0)
            finally:
                if link and link.poll() is None:
                    link.kill()
                daemon.send_signal(signal.SIGTERM)
                daemon.wait(timeout=60)
        finally:
            shutil.rmtree(home, ignore_errors=True)

    def test_rust_release_bundle_pins_ferbs_certificate_in_every_piece_macos_grants_to(self) -> None:
        """A SIGN_IDENTITY=release build (FERB_RELEASE_BUNDLE=1): each piece macOS grants permissions to pins FERB's
        release certificate in its designated requirement (from tests/test_packaging.py, for the Rust app; the Python
        installer's sealed-requirement check is gone with it)."""
        bundle = _rust_bundle()
        if os.environ.get("FERB_RELEASE_BUNDLE") != "1":
            raise unittest.SkipTest("set FERB_RELEASE_BUNDLE=1 with FERB_RUST_APP_BUNDLE at a SIGN_IDENTITY=release build")
        sha = os.environ.get("FERB_TEST_RELEASE_SHA1") or (ROOT / "scripts/release-cert.sha1").read_text().strip()
        for rel, ident in RELEASE_PIECES:
            piece = bundle / rel if rel else bundle
            shown = subprocess.run(["codesign", "-d", "-r-", str(piece)], capture_output=True, text=True)
            requirement = _designated_requirement(shown.stdout + shown.stderr)
            self.assertRegex(requirement, rf'(?i)^identifier "{re.escape(ident)}" and certificate leaf = H"{sha}"$')
            details = subprocess.run(["codesign", "-dv", "--verbose=2", str(piece)], capture_output=True,
                                     text=True).stderr
            self.assertTrue("Authority=FERB Release Signing" in details and "TeamIdentifier=not set" in details, details)

    # decision: REL-420, REL-421
    def test_rust_devid_bundle_is_team_signed_notarized_and_stapled_in_every_piece_macos_grants_to(self) -> None:
        """A SIGN_IDENTITY=devid build (FERB_DEVID_BUNDLE=1): every piece macOS grants permissions to is the team's,
        timestamped, hardened, without library validation turned off, and the app is stapled and accepted by
        Gatekeeper (from tests/test_packaging.py, for the Rust app)."""
        bundle = _rust_bundle()
        if os.environ.get("FERB_DEVID_BUNDLE") != "1":
            raise unittest.SkipTest("set FERB_DEVID_BUNDLE=1 with FERB_RUST_APP_BUNDLE at a SIGN_IDENTITY=devid build")
        team = (ROOT / "scripts/developer-team.txt").read_text().strip()
        for rel, ident in RELEASE_PIECES:
            piece = bundle / rel if rel else bundle
            shown = subprocess.run(["codesign", "-d", "-r-", str(piece)], capture_output=True, text=True)
            requirement = _designated_requirement(shown.stdout + shown.stderr)
            self.assertTrue(requirement.startswith(f'identifier "{ident}" and anchor apple generic'), requirement)
            self.assertIn(f"certificate leaf[subject.OU] = {team}", requirement.replace('"', ""))
            details = subprocess.run(["codesign", "-dv", "--verbose=2", str(piece)], capture_output=True,
                                     text=True).stderr
            self.assertTrue("Authority=Developer ID Application: " in details and f"TeamIdentifier={team}" in details)
            self.assertTrue("Timestamp=" in details and "flags=0x10000(runtime)" in details, details)
            entitlements = subprocess.run(["codesign", "-d", "--entitlements", "-", "--xml", str(piece)],
                                          capture_output=True, text=True).stdout
            self.assertTrue("disable-library-validation" not in entitlements and "get-task-allow" not in entitlements,
                            piece)
        stapled = subprocess.run(["xcrun", "stapler", "validate", str(bundle)], capture_output=True, text=True)
        self.assertEqual(stapled.returncode, 0, stapled.stdout + stapled.stderr)
        assessed = subprocess.run(["spctl", "-a", "-t", "exec", "-vv", str(bundle)], capture_output=True, text=True)
        self.assertTrue(assessed.returncode == 0 and "source=Notarized Developer ID" in assessed.stderr)
        arch = lipo_archs(bundle / "Contents/MacOS/FERBNotch")
        self.assertEqual(len(arch), 1)


class BuiltRustBundleSize(TempCase):
    def test_rust_bundle_is_one_architecture(self) -> None:
        bundle = _rust_bundle()
        archs = lipo_archs(bundle / "Contents/MacOS/FERBNotch")
        self.assertEqual(len(archs), 1, archs)
        self.assertIn(archs[0], ARCHS)
        check = zsh(f"ferb_check_arch {shlex.quote(str(bundle))} {archs[0]}")
        self.assertEqual(check.returncode, 0, check.stderr)


if __name__ == "__main__":
    unittest.main()
