"""L34 Phase G, rule 10: FERB's shell can't change FERB.app's own preferences (Sparkle's update switches live there),
neither through cfprefsd (`defaults write`) nor by writing the plist file. Measured with the real sandbox-exec on a
throwaway preference domain; the owner's real FERB domains are never written."""

import shutil
import subprocess
import uuid
from pathlib import Path

import pytest

from ferb.act.shell.sandbox import APP_PREFERENCE_DOMAINS
from tests.conftest import RunningDaemon
from tests.support.sandbox_probes import run_under

pytestmark = pytest.mark.skipif(not shutil.which("sandbox-exec"), reason="sandbox-exec is not available here")


def test_ferb_apps_preference_domains_are_listed() -> None:
    assert APP_PREFERENCE_DOMAINS == ("com.ferb.FERB", "com.ferb.app", "com.ferb.daemon", "dev.ferb.notch")


def test_both_profiles_deny_them(daemon: RunningDaemon) -> None:
    sb = daemon.daemon.shells.sandbox                                    # type: ignore[union-attr]
    for text in (sb.profile(), sb.profile(apps=True)):
        assert '(deny user-preference-write (preference-domain "com.ferb.FERB"))' in text
        assert 'Library/Preferences/com.ferb.FERB.plist"))' in text


def test_the_shell_cannot_write_ferb_apps_preferences(daemon: RunningDaemon, monkeypatch: pytest.MonkeyPatch) -> None:
    domain = f"com.ferb.sandboxtest.{uuid.uuid4().hex[:8]}"
    sb = daemon.daemon.shells.sandbox                                    # type: ignore[union-attr]
    monkeypatch.setattr(sb, "app_domains", (domain,))
    plist = Path.home() / "Library" / "Preferences" / f"{domain}.plist"
    try:
        assert run_under(sb, ["/usr/bin/true"]).returncode == 0          # the profile compiles
        run_under(sb, ["/usr/bin/defaults", "write", domain, "SUAutomaticallyUpdate", "-bool", "NO"])
        read = subprocess.run(["/usr/bin/defaults", "read", domain, "SUAutomaticallyUpdate"],
                              capture_output=True, text=True)
        assert read.returncode != 0, "the sandboxed write reached cfprefsd"
        made = run_under(sb, ["/usr/bin/plutil", "-create", "xml1", str(plist)])
        assert made.returncode != 0 and not plist.exists()
        other = run_under(sb, ["/usr/bin/defaults", "read", "-g", "AppleLocale"])
        assert "Operation not permitted" not in other.stderr             # other preferences stay readable
    finally:
        subprocess.run(["/usr/bin/defaults", "delete", domain], capture_output=True)
        plist.unlink(missing_ok=True)
