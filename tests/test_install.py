"""Installer (section 8 steps 5-6) with a fake command runner: plist content, idempotent install, --update,
uninstall and the purge safety rules. Nothing real is installed here (see test_launchd_real.py)."""

import json
from pathlib import Path

import pytest

from ferb.device.install import MARKER, InstallError, Installer, code_hash
from tests.conftest import FakeRunner


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
