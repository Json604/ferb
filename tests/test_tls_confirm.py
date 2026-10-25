"""L39 (owner, 2026-10-25, rule 8): a shell command that turns off TLS certificate checks asks the owner first,
showing the exact command; a No runs nothing; the answer covers that exact command for the task tree, across retries.
The live case: task 39e35d57 ran `pip install … --trusted-host pypi.org --trusted-host files.pythonhosted.org`
without asking."""

from pathlib import Path
from typing import Any

from tests.conftest import Answers, FakeCap, RunningDaemon, gateway_request, gateway_setup, gateway_tool_call
from tests.support.live_cases import LIVE_PIP as LIVE

QUESTION = ("Run this with certificate checks turned off? `{command}` — this skips the check that the site is really "
            "{host}.")


def _shell(daemon: RunningDaemon) -> tuple[Any, FakeCap]:
    sh = FakeCap("probe.shell", "run_shell", {"probe"})
    grant, _ = gateway_setup(daemon, {"probe"}, gateway_request("unzip all these 3, make sure they are correct"), [sh])
    return grant, sh


def _one_line(text: str) -> str:
    return " ".join(text.split())


def test_the_live_pip_command_asks_with_the_exact_command_and_a_no_runs_nothing(daemon: RunningDaemon) -> None:
    grant, sh = _shell(daemon)
    owner = Answers(daemon, granted=True, choice=2)                          # No
    r = gateway_tool_call(daemon, grant, "probe.shell", text=LIVE)
    again = gateway_tool_call(daemon, grant, "probe.shell", text=LIVE)       # a retry: the No carries, not asked again
    owner.stop()
    assert r["is_error"] and sh.calls == 0 and again["is_error"]
    [asked] = owner.asked
    expected = QUESTION.format(command=_one_line(LIVE), host="pypi.org or files.pythonhosted.org")
    assert expected in asked.reason and asked.summary == expected
    assert "already said no" in again["content"]["error"]


def test_a_yes_runs_it_and_covers_its_retries(daemon: RunningDaemon) -> None:
    grant, sh = _shell(daemon)
    owner = Answers(daemon, granted=True, choice=1)                          # Yes
    r = gateway_tool_call(daemon, grant, "probe.shell", text=LIVE)
    again = gateway_tool_call(daemon, grant, "probe.shell", text=LIVE)
    owner.stop()
    assert not r["is_error"] and not again["is_error"] and sh.calls == 2 and len(owner.asked) == 1


def test_an_ordinary_pip_install_asks_nothing(daemon: RunningDaemon) -> None:
    grant, sh = _shell(daemon)
    owner = Answers(daemon, granted=True, choice=2)
    r = gateway_tool_call(daemon, grant, "probe.shell", text="python3 -m pip install --quiet --target /tmp/x PyMuPDF")
    owner.stop()
    assert not r["is_error"] and sh.calls == 1 and owner.asked == []


def test_a_command_with_no_named_site_says_so(daemon: RunningDaemon) -> None:
    grant, sh = _shell(daemon)
    owner = Answers(daemon, granted=True, choice=2)
    gateway_tool_call(daemon, grant, "probe.shell", text="PYTHONHTTPSVERIFY=0 python3 fetch.py")
    owner.stop()
    assert owner.asked[0].summary.endswith("this skips the check that each site it reaches is really that site.")


def test_the_real_shell_runs_nothing_after_a_no(daemon: RunningDaemon, tmp_path: Path) -> None:
    from tests.mackit import gateway_call as call, grant_task as task
    from ferb.core.types import Identity
    marker = tmp_path / "ran"
    g = task(daemon, "tls-real", Identity.OWNER, {"shell"}, "check curl")
    owner = Answers(daemon, granted=True, choice=2)
    r = call(daemon, g, "shell.run", command=f"touch '{marker}'; curl -k --version >/dev/null")
    owner.stop()
    assert r["is_error"] and not marker.exists() and len(owner.asked) == 1


def test_an_applescript_do_shell_script_that_turns_checks_off_asks_too(daemon: RunningDaemon) -> None:
    """I3: mac.applescript's `do shell script` runs a shell command; its switches ask like shell.run's."""
    script = FakeCap("probe.script", "applescript", {"probe"})
    grant, _ = gateway_setup(daemon, {"probe"}, gateway_request("save the page"), [script])
    owner = Answers(daemon, granted=True, choice=2)
    r = gateway_tool_call(daemon, grant, "probe.script", text='do shell script "curl -k https://example.com -o /tmp/a"')
    owner.stop()
    assert r["is_error"] and script.calls == 0 and len(owner.asked) == 1
    assert "skips the check that the site is really example.com." in owner.asked[0].summary
