"""Installer (section 8 steps 5-6) with a fake command runner: plist content, idempotent install, --update,
uninstall and the purge safety rules. Nothing real is installed here (see test_launchd_real.py)."""

import json
import math
import shlex
from pathlib import Path

import pytest

import ferb.device.install as install_mod
from ferb.device.install import MARKER, InstallError, Installer, code_hash
from tests.conftest import FakeRunner, SteppedTime


@pytest.fixture
def inst(tmp_path: Path) -> Installer:
    home = tmp_path / ".ferb"
    return Installer(home, label="com.ferb.test", run=FakeRunner(home), launch_agents=tmp_path / "LaunchAgents")


def test_plist_points_into_ferb_home_and_restarts_only_after_crashes(inst: Installer) -> None:
    p = inst.plist()
    assert p["ProgramArguments"] == [str(inst.home / "venv" / "bin" / "python"), "-m", "ferb.daemon"]
    assert p["EnvironmentVariables"] == {"FERB_HOME": str(inst.home)}
    assert p["KeepAlive"] == {"SuccessfulExit": False}
    assert p["LimitLoadToSessionType"] == "Aqua"
    assert str(p["StandardErrorPath"]).startswith(str(inst.home / "logs"))
    assert "Desktop" not in str(p["ProgramArguments"])          # never runs code from the repo


def test_install_is_idempotent(inst: Installer) -> None:
    r: FakeRunner = inst.run  # type: ignore[assignment]
    inst.install()
    assert (r.ran("uv", "python", "install"), r.ran("uv", "venv"), r.ran("uv", "pip", "install")) == (1, 1, 1)
    assert r.ran("launchctl", "bootstrap") == 1 and inst.plist_path.exists() and (inst.home / MARKER).exists()
    assert inst.granted_binary() == inst.interpreter().resolve()
    inst.install()                                               # rerun: no new Python, no new venv, restart
    assert (r.ran("uv", "python", "install"), r.ran("uv", "venv")) == (1, 1)
    assert r.ran("uv", "pip", "install") == 2 and r.ran("launchctl", "kickstart", "-k") == 1


def test_installed_secret_helper_requires_stable_certificate_identity(inst: Installer) -> None:
    inst.install_secret_helper()
    helper = inst.home / 'bin' / 'ferb-secrets'
    assert helper.stat().st_mode & 0o777 == 0o700
    assert 'certificate leaf' in helper.with_suffix('.requirement').read_text()
    real = inst.run
    def unsigned(cmd):
        result = real(cmd)
        if cmd[:3] == ['/usr/bin/codesign', '-d', '-r-']:
            result.stdout = 'designated => identifier "com.ferb.secrets"\n'
        return result
    inst.run = unsigned
    with pytest.raises(InstallError, match='stable certificate identity'):
        inst.install_secret_helper()


def test_venv_on_another_interpreter_is_rebuilt_on_ferbs_own(inst: Installer, tmp_path: Path) -> None:
    other = tmp_path / "other-python"
    other.write_text("")
    (inst.venv / "bin").mkdir(parents=True)
    (inst.venv / "bin" / "python").symlink_to(other)
    inst.install()
    assert inst.venv_python.resolve() == inst.interpreter().resolve()


def test_update_only_reinstalls_ferb_and_restarts(inst: Installer) -> None:
    r: FakeRunner = inst.run  # type: ignore[assignment]
    with pytest.raises(InstallError, match="run `ferb setup` first"):
        inst.install(update_only=True)
    inst.install()
    before = len(r.cmds)
    inst.install(update_only=True)
    new = r.cmds[before:]
    assert [c[:3] for c in new if c[0] == "uv"] == [["uv", "pip", "install"]]
    assert any(c[:3] == ["launchctl", "kickstart", "-k"] for c in new)


def test_work_that_needs_the_daemon_stopped_runs_after_the_install_and_before_the_new_start(inst: Installer) -> None:
    """Plan 2026-10-23 Task 1: setup's store clean-out runs once everything is installed, with the agent unloaded, and
    the agent is loaded again even when that work fails (M5: never left unloaded)."""
    r: FakeRunner = inst.run  # type: ignore[assignment]
    inst.install()
    before = len(r.cmds)
    seen: list[tuple[bool, int]] = []
    inst.install(update_only=True, while_stopped=lambda: seen.append((r.loaded, len(r.cmds))))
    [(loaded, at)] = seen
    new = r.cmds[before:]
    assert not loaded and ["uv", "pip", "install"] in [c[:3] for c in r.cmds[before:at]]
    assert ["launchctl", "bootout"] in [c[:2] for c in r.cmds[before:at]]
    assert r.cmds[at - 1] == ["/bin/kill", "-0", "4242"]                   # its process was seen gone right before
    assert ["launchctl", "bootstrap"] in [c[:2] for c in r.cmds[at:]] and r.loaded
    assert not any(c[:2] == ["launchctl", "kickstart"] for c in new)

    def broken() -> None:
        raise RuntimeError("disk I/O error")
    with pytest.raises(RuntimeError):
        inst.install(update_only=True, while_stopped=broken)
    assert r.loaded


def test_unload_waits_until_the_stopped_daemons_process_has_exited(inst: Installer,
                                                                   monkeypatch: pytest.MonkeyPatch) -> None:
    """L28, C1: launchd can drop the agent while its process is still in its shutdown tail, writing the store; unload
    waits for the process too, within the same 10 s it gives launchd."""
    r: FakeRunner = inst.run  # type: ignore[assignment]
    clock = SteppedTime()
    monkeypatch.setattr(install_mod, "time", clock)
    inst.install()
    r.tail = 3                                                   # it answers three more probes after the bootout
    inst.unload()
    assert r.ran("/bin/kill", "-0", "4242") == 4 and 0 < clock.now < 10
    assert inst.exiting() is None


def test_unload_stops_waiting_at_its_bound_and_names_the_process_still_running(
        inst: Installer, monkeypatch: pytest.MonkeyPatch) -> None:
    r: FakeRunner = inst.run  # type: ignore[assignment]
    clock = SteppedTime()
    monkeypatch.setattr(install_mod, "time", clock)
    inst.install()
    r.tail = math.inf                                            # it never exits
    inst.unload()
    assert 10 <= clock.now < 10.2 and inst.exiting() == 4242
    inst.unload()                                                # nothing loaded now: no process of its own to wait for
    assert inst.exiting() is None


def test_uninstall_removes_agent_and_plist_but_keeps_home(inst: Installer) -> None:
    inst.install()
    removed = inst.uninstall()
    assert not inst.plist_path.exists() and not inst.loaded()
    assert inst.home.exists() and len(removed) == 2


def test_purge_deletes_home_only_with_marker(inst: Installer) -> None:
    inst.install()
    inst.uninstall(purge=True)
    assert not inst.home.exists()


def test_purge_refuses_without_marker_or_on_dangerous_paths(tmp_path: Path) -> None:
    home = tmp_path / "somefolder"
    home.mkdir()
    (home / "important.txt").write_text("keep me")
    i = Installer(home, label="com.ferb.test", run=FakeRunner(home), launch_agents=tmp_path / "LA")
    with pytest.raises(InstallError, match="marker"):
        i.uninstall(purge=True)
    assert (home / "important.txt").exists()
    for danger in (Path.home(), Path("/"), Path.home() / "Desktop"):
        d = Installer(danger, label="com.ferb.test", run=FakeRunner(danger), launch_agents=tmp_path / "LA")
        with pytest.raises(InstallError, match="refusing"):
            d._check_purge_target()


def test_code_hash_changes_with_code(tmp_path: Path) -> None:
    pkg = tmp_path / "ferb"
    (pkg / "sub").mkdir(parents=True)
    (pkg / "a.py").write_text("x = 1\n")
    (pkg / "sub" / "b.sql").write_text("SELECT 1;\n")
    h1 = code_hash(pkg)
    (pkg / "__pycache__").mkdir()
    (pkg / "__pycache__" / "a.pyc").write_bytes(b"junk")
    assert code_hash(pkg) == h1                                  # caches don't count
    (pkg / "a.py").write_text("x = 2\n")
    assert code_hash(pkg) != h1


def test_code_hash_covers_prompts_and_equivalent_packaged_extension(tmp_path):
    import shutil
    checkout, installed = tmp_path / 'checkout', tmp_path / 'installed'
    package = checkout / 'ferb'
    package.mkdir(parents=True)
    (package / 'prompt.md').write_text('first prompt')
    (package / 'module.py').write_text('pass')
    extension = checkout / 'extensions' / 'chromium'
    extension.mkdir(parents=True)
    (extension / 'manifest.json').write_text('{"version":"1"}')
    (extension / 'background.js').write_text('first extension')
    shutil.copytree(package, installed)
    shutil.copytree(extension, installed / '_extension')
    assert code_hash(package) == code_hash(installed)
    before = code_hash(package)
    (package / 'prompt.md').write_text('second prompt')
    assert code_hash(package) != before
    before = code_hash(package)
    (extension / 'background.js').write_text('second extension')
    assert code_hash(package) != before


def test_native_host_manifest_goes_to_every_listed_folder_and_uninstall_removes_it(tmp_path: Path) -> None:
    """Section 8: Chrome's folder (read by Chrome and Brave) plus Brave's own; the launcher lives under ~/.ferb."""
    from ferb.device.install import NATIVE_HOST_MANIFEST
    dirs = (tmp_path / "Chrome" / "NativeMessagingHosts", tmp_path / "Brave" / "NativeMessagingHosts")
    inst = Installer(tmp_path / ".ferb", label="com.ferb.test", run=FakeRunner(tmp_path / ".ferb"),
                     launch_agents=tmp_path / "LA", native_hosts_dirs=dirs, extension_ids=("abc",))
    written = inst.write_native_host()
    assert [w.parent for w in written] == list(dirs)
    m = json.loads(written[0].read_text())
    assert m["allowed_origins"] == ["chrome-extension://abc/"] and m["path"] == str(inst.native_host_launcher)
    assert str(tmp_path / ".ferb") in str(inst.native_host_launcher)          # never a protected folder
    inst.uninstall()
    assert not any((d / NATIVE_HOST_MANIFEST).exists() for d in dirs)
    assert Installer(tmp_path / "x").write_native_host() == []               # no folders given: writes nothing


# ---------- packaged FERB.app (L27): the daemon, helpers and `ferb` run from inside the bundle ----------

from tests.support.app_bundle import AGENT, CODEX_BIN, PINNED, BundleRunner, make_ferb_app  # noqa: E402


def bundled(tmp_path: Path, **runner: object) -> Installer:
    app = make_ferb_app(tmp_path / "Applications")
    home = tmp_path / ".ferb"
    return Installer(home, label="com.ferb.test", run=BundleRunner(home, **runner),  # type: ignore[arg-type]
                     launch_agents=tmp_path / "LaunchAgents", bundle=app, cli_dir=tmp_path / "bin",
                     native_hosts_dirs=(tmp_path / "Chrome" / "NativeMessagingHosts",), extension_ids=("abc",))


def test_app_bundle_is_found_only_from_the_interpreter_inside_ferb_app(tmp_path: Path) -> None:
    from ferb.device.install import app_bundle
    app = make_ferb_app(tmp_path / "Applications")
    assert app_bundle(app / "Contents" / "Resources" / "python") == app
    assert app_bundle(tmp_path / ".ferb" / "venv") is None                     # the source install
    assert app_bundle(tmp_path / "python") is None
    assert app_bundle() is None                                                 # this checkout's own venv


def test_bundle_plist_runs_the_daemon_as_the_nested_ferb_agent_app(tmp_path: Path) -> None:
    """launchd starts FERB's agent app (a native stub that embeds the app's Python, isolated, never writing bytecode),
    so macOS names FERB, not python3.12, in permission prompts, Privacy & Security and Login Items."""
    inst = bundled(tmp_path)
    p = inst.plist()
    agent = inst.bundle / AGENT  # type: ignore[operator]
    assert p["ProgramArguments"] == [str(agent), "-m", "ferb.daemon"]
    assert p["EnvironmentVariables"] == {"FERB_HOME": str(inst.home), "FERB_APP_BUILD": "202610140201"}
    assert p["AssociatedBundleIdentifiers"] == ["com.ferb.FERB"]          # Login Items shows it under FERB
    assert inst.granted_binary() == agent.resolve()                         # what the daemon reports as its executable
    assert inst.grant_target() == inst.bundle / "Contents" / "Helpers" / "FERB.app"  # type: ignore[operator]
    assert str(p["StandardErrorPath"]).startswith(str(inst.home / "logs"))   # the same ~/.ferb home and logs


def test_bundle_install_needs_no_uv_swiftc_or_npm_and_installs_from_the_bundle(tmp_path: Path) -> None:
    inst = bundled(tmp_path)
    r: BundleRunner = inst.run  # type: ignore[assignment]
    inst.install()
    ran = [c[0] for c in r.cmds]
    assert not {"uv", "xcrun", "npm"} & set(ran), r.cmds
    assert r.ran("/usr/bin/codesign", "--verify", "--deep", "--strict", str(inst.bundle)) == 1
    assert r.ran("launchctl", "bootstrap") == 1 and inst.plist_path.exists() and (inst.home / MARKER).exists()
    helper = inst.home / "bin" / "ferb-secrets"                               # copied, signature intact
    assert helper.read_bytes() == (inst.bundle / "Contents" / "Helpers" / "ferb-secrets").read_bytes()  # type: ignore[operator]
    assert helper.stat().st_mode & 0o777 == 0o700
    assert helper.with_suffix(".requirement").read_text().strip() == PINNED   # ad-hoc: the exact code is pinned
    launcher = inst.native_host_launcher.read_text()
    python = shlex.quote(f"{inst.bundle}/Contents/Resources/python/bin/python3.12")
    assert f"exec {python} -I -B -m ferb.native_host.main" in launcher
    link = tmp_path / "bin" / "ferb"
    assert link.is_symlink() and link.resolve() == (inst.bundle / "Contents" / "Resources" / "bin" / "ferb").resolve()  # type: ignore[operator]
    node = inst.home / "node"                                                  # the pinned Codex ships in the app
    assert node.is_symlink() and (node / CODEX_BIN).read_bytes().startswith(b"fake")
    assert (inst.home / "extensions" / "chromium" / "manifest.json").is_file()


def test_bundle_install_refuses_a_temporary_read_only_or_damaged_copy(tmp_path: Path) -> None:
    from ferb.device.install import InstallError
    moved = make_ferb_app(tmp_path / "AppTranslocation" / "1A2B" / "d")
    home = tmp_path / ".ferb"
    inst = Installer(home, label="com.ferb.test", run=BundleRunner(home), launch_agents=tmp_path / "LA", bundle=moved)
    with pytest.raises(InstallError, match="move FERB.app to Applications"):
        inst.install()
    inst = bundled(tmp_path / "dmg")
    inst.read_only = lambda path: True                                        # the mounted disk image
    with pytest.raises(InstallError, match="disk image"):
        inst.install()
    inst = bundled(tmp_path / "bad", bad_signature=True)
    with pytest.raises(InstallError, match="signature"):
        inst.install()
    assert not inst.plist_path.exists() and not (tmp_path / "bad" / "bin" / "ferb").exists()
    assert not inst.home.exists()                                             # nothing written before the checks


def test_bundle_update_reloads_the_agent_when_the_app_build_changes(tmp_path: Path) -> None:
    import plistlib
    from ferb.device.install import InstallError
    inst = bundled(tmp_path)
    r: BundleRunner = inst.run  # type: ignore[assignment]
    with pytest.raises(InstallError, match="run `ferb setup` first"):
        inst.install(update_only=True)
    inst.install()
    info = inst.bundle / "Contents" / "Info.plist"  # type: ignore[operator]
    data = plistlib.loads(info.read_bytes())
    info.write_bytes(plistlib.dumps({**data, "CFBundleVersion": "202610150900"}))   # the owner replaced FERB.app
    inst.install(update_only=True)
    assert plistlib.loads(inst.plist_path.read_bytes())["EnvironmentVariables"]["FERB_APP_BUILD"] == "202610150900"
    assert r.ran("launchctl", "bootstrap") == 2
    inst.install(update_only=True)                                            # nothing changed: a restart only
    assert r.ran("launchctl", "bootstrap") == 2 and r.ran("launchctl", "kickstart", "-k") == 1


def test_bundle_secret_helper_must_be_exactly_the_code_ferb_apps_signature_seals(tmp_path: Path) -> None:
    """A valid signature alone isn't enough: the helper copied into ~/.ferb must have the code hash FERB.app's own
    (verified) signature records for it, and its pinned requirement is that hash (ad-hoc)."""
    from ferb.device.install import InstallError
    inst = bundled(tmp_path)
    inst.install_secret_helper()
    assert (inst.home / "bin" / "ferb-secrets.requirement").read_text().strip() == PINNED
    swapped = make_ferb_app(tmp_path / "Swapped", sealed="0" * 40)          # the app seals some other helper
    inst.bundle = swapped
    with pytest.raises(InstallError, match="isn't the one FERB.app's signature seals"):
        inst.install_secret_helper()
    inst = bundled(tmp_path / "idonly", requirement='identifier "com.ferb.secrets"')
    with pytest.raises(InstallError, match="neither"):
        inst.install_secret_helper()


def test_bundle_secret_helper_with_a_developer_id_must_carry_the_apps_own_team(tmp_path: Path) -> None:
    from ferb.device.install import InstallError
    dev_id = ('identifier "com.ferb.secrets" and anchor apple generic and certificate 1[field.1.2.840.113635.100.6.2.6]'
              ' /* exists */ and certificate leaf[field.1.2.840.113635.100.6.1.13] /* exists */ and certificate '
              'leaf[subject.OU] = {}')
    inst = bundled(tmp_path, requirement=dev_id.format("TEAM123456"), team="TEAM123456")
    inst.install_secret_helper()
    assert "TEAM123456" in (inst.home / "bin" / "ferb-secrets.requirement").read_text()
    inst = bundled(tmp_path / "other", requirement=dev_id.format("OTHERTEAM1"), team="TEAM123456")
    with pytest.raises(InstallError, match="neither"):
        inst.install_secret_helper()


def test_cli_link_never_replaces_a_ferb_that_ferb_app_did_not_make(tmp_path: Path) -> None:
    inst = bundled(tmp_path)
    (tmp_path / "bin").mkdir()
    other = tmp_path / "bin" / "ferb"
    other.write_text("#!/bin/sh\necho mine\n")                               # e.g. the owner's own script
    inst.install()
    assert other.read_text() == "#!/bin/sh\necho mine\n"
    assert "left" in (inst.cli_status() or "") and str(other) in (inst.cli_status() or "")
    other.unlink()
    other.symlink_to(tmp_path / "elsewhere" / "ferb")                       # someone else's link: left too
    inst.install()
    assert other.readlink() == tmp_path / "elsewhere" / "ferb"


def test_existing_pinned_codex_is_kept_and_a_stale_one_is_replaced_by_the_apps(tmp_path: Path) -> None:
    from ferb.runtimes.codex_exec import CODEX_VERSION
    inst = bundled(tmp_path, codex_version=f"codex-cli {CODEX_VERSION}\n")
    pinned = inst.home / "node" / CODEX_BIN
    pinned.parent.mkdir(parents=True)
    pinned.write_text("owner's npm install")
    inst.ensure_codex()
    assert not (inst.home / "node").is_symlink() and pinned.read_text() == "owner's npm install"
    inst.run.codex_version = "codex-cli 0.1.0\n"  # type: ignore[attr-defined]
    inst.ensure_codex()
    assert (inst.home / "node").is_symlink()


def test_source_codex_install_replaces_a_link_into_ferb_app(tmp_path: Path, inst: Installer) -> None:
    """A checkout install keeps its own pinned Codex: a link into FERB.app is replaced even when that Codex is the
    pinned version, and npm never writes through it."""
    from ferb.runtimes.codex_exec import CODEX_VERSION
    app = make_ferb_app(tmp_path / "Applications")
    (inst.home / "node").parent.mkdir(parents=True, exist_ok=True)
    (inst.home / "node").symlink_to(app / "Contents" / "Resources" / "node")
    real = inst.run

    def pinned(cmd: list[str]) -> object:
        result = real(cmd)
        if cmd[1:] == ["--version"]:
            result.stdout = f"codex-cli {CODEX_VERSION}\n"
        return result
    inst.run = pinned  # type: ignore[assignment]
    inst.ensure_codex()                                                       # source mode: npm into its own folder
    assert not (inst.home / "node").is_symlink()
    assert real.ran("npm", "install") == 1  # type: ignore[attr-defined]
    assert (app / "Contents" / "Resources" / "node" / CODEX_BIN).read_bytes().startswith(b"fake")   # untouched


def test_a_checkout_update_after_an_app_install_moves_launchd_back_to_the_checkout(tmp_path: Path) -> None:
    import plistlib
    app_inst = bundled(tmp_path)
    app_inst.install()
    source = Installer(app_inst.home, label="com.ferb.test", run=app_inst.run, launch_agents=tmp_path / "LaunchAgents")
    app_inst.run.requirement = 'identifier "com.ferb.secrets" and certificate leaf = H"local"'  # type: ignore[attr-defined]
    (source.venv / "bin").mkdir(parents=True)
    (source.venv / "bin" / "python").write_text("")
    before = app_inst.run.ran("launchctl", "bootstrap")  # type: ignore[attr-defined]
    assert source.active_program() == str(app_inst.bundle / AGENT)  # type: ignore[operator]
    source.install(update_only=True)
    agent = plistlib.loads(source.plist_path.read_bytes())
    assert agent["ProgramArguments"][0] == str(source.venv_python) and source.active_program() == str(source.venv_python)
    assert app_inst.run.ran("launchctl", "bootstrap") == before + 1  # type: ignore[attr-defined]   # reloaded


def test_uninstall_removes_the_cli_link_ferb_app_made(tmp_path: Path) -> None:
    inst = bundled(tmp_path)
    inst.install()
    removed = inst.uninstall()
    assert not (tmp_path / "bin" / "ferb").exists() and str(tmp_path / "bin" / "ferb") in removed


def test_voice_app_lives_in_the_bundle_when_ferb_runs_from_one(tmp_path: Path) -> None:
    from ferb.device.install import voice_app
    app = make_ferb_app(tmp_path / "Applications")
    assert voice_app(tmp_path / ".ferb", None) == tmp_path / ".ferb" / "FERB.app"
    assert voice_app(tmp_path / ".ferb", app) == app / "Contents" / "Helpers" / "FERB Voice.app"


def test_designated_requirement_reads_explicit_and_implicit_lines() -> None:
    from ferb.device.install import designated_requirement, trusted_requirement
    assert designated_requirement('Executable=/x\n# designated => cdhash H"ab"\n') == 'cdhash H"ab"'
    assert designated_requirement('designated => identifier "x" and certificate leaf = H"cd"\n') == \
        'identifier "x" and certificate leaf = H"cd"'
    assert designated_requirement("no signature\n") == ""
    assert trusted_requirement('identifier "x" and certificate leaf = H"cd"', pinned_ok=False)
    assert not trusted_requirement(PINNED, pinned_ok=False)                   # a source install keeps its certificate
    assert trusted_requirement(PINNED, pinned_ok=True)
    assert not trusted_requirement('identifier "com.ferb.secrets"', pinned_ok=True)
    assert not trusted_requirement('cdhash H"ab" or identifier "x"', pinned_ok=True)


def test_daemon_running_from_ferb_app_uses_the_nested_voice_helper(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    from types import SimpleNamespace
    import ferb.device.install as install
    from ferb.daemon.main import Daemon
    app = make_ferb_app(tmp_path / "Applications")
    home = SimpleNamespace(home=tmp_path / ".ferb")
    assert Daemon.voice_bundle.fget(home) == tmp_path / ".ferb" / "FERB.app"  # type: ignore[attr-defined]
    monkeypatch.setattr(install, "app_bundle", lambda prefix=None: app)
    assert Daemon.voice_bundle.fget(home) == app / "Contents" / "Helpers" / "FERB Voice.app"  # type: ignore[attr-defined]


def test_mcp_shim_never_imports_a_ferb_package_planted_in_its_working_folder(tmp_path: Path) -> None:
    """Runtimes start the shim in FERB's workspace, which workers write: `python -m` would put that folder first on
    sys.path, so a planted ferb/ there would run instead of FERB's own (M9)."""
    import subprocess
    from ferb.daemon.main import mcp_shim_command
    (tmp_path / "ferb" / "cli").mkdir(parents=True)
    for rel in ("ferb/__init__.py", "ferb/cli/__init__.py", "ferb/cli/mcp.py"):
        (tmp_path / rel).write_text("print('PLANTED')\n")
    r = subprocess.run(mcp_shim_command(), cwd=tmp_path, capture_output=True, text=True, timeout=60,
                       stdin=subprocess.DEVNULL, env={"PATH": "/usr/bin:/bin", "HOME": str(tmp_path)})
    assert "PLANTED" not in r.stdout + r.stderr and "FERB_RUN_TOKEN" in r.stderr     # FERB's own shim answered
