"""ORCH-624: a site's Submit that is a script button, not a form (live 2026-11-12, task aa3cb86a, hack2skill.com): the
worker clicked it declaring a send to the organizers, the owner said Yes to that send, the site took the submission,
and the literal `submitted` check (ORCH-576) found no submit record and failed the task. Through the real daemon,
gateway and scripted model: that click counts as the recorded submit; a claim with no such call still fails."""
import json
from typing import Any

from tests.conftest import FakeCap, RunningDaemon, chat, cond, done, install, outcome, sup, task, tool, verdict
from tests.support.confirm import FerbAnswers

SITE = "hack2skill.example"
FORM = cond("The Buildathon form is submitted", "submitted", site=SITE)
SAID = "Submitted the Startup Buildathon form."


def _form_task() -> dict[str, Any]:
    return task("Fill and submit the Buildathon form", conds=(FORM,))


def _clicks(daemon: RunningDaemon, **declared: Any) -> Any:
    def go(token: str) -> str:
        assert not tool(daemon.sock, token, "probe.click", to="Buildathon organizers", site=SITE, **declared)["is_error"]
        return done(SAID)
    return go


# decision: ORCH-624, ORCH-576
def test_a_submit_button_the_owner_said_yes_to_counts_as_the_submit(daemon: RunningDaemon) -> None:
    daemon.daemon.registry.capabilities["probe.click"] = FakeCap("probe.click", "click", {"probe"})
    install(daemon, [sup("task", "Submitting.", _form_task())],
            {"worker": _clicks(daemon, effect="send"), "worker.verify": verdict(True)})
    with FerbAnswers(daemon, yes=True) as owner:
        final = outcome(chat(daemon, "fill and submit the buildathon form"))
    assert owner.asked, "the declared send to someone the owner didn't name is asked about"
    assert final["status"] == "done" and final["verification"][0]["met"] is True, final   # before ORCH-624 code found it false


# decision: ORCH-624, ORCH-576, ORCH-609
def test_a_claimed_submit_with_no_such_call_still_fails(daemon: RunningDaemon) -> None:
    """The check's purpose stays: the worker says it submitted, a plain click with no send ran (no question, no
    declaration), and nothing at the site was submitted in FERB's records."""
    daemon.daemon.registry.capabilities["probe.click"] = FakeCap("probe.click", "click", {"probe"})
    install(daemon, [sup("task", "Submitting.", _form_task())],
            {"worker": _clicks(daemon), "worker.verify": verdict(True)})
    final = outcome(chat(daemon, "fill and submit the buildathon form"))
    assert final["status"] == "failed" and final["verification"][0]["path"] == "code"
    assert final["answer"].split("\n\n")[0] == f"I couldn't finish this task. No form on {SITE} was submitted."
    install(daemon, [sup("task", "Submitting.", _form_task())],
            {"worker": done(SAID), "worker.verify": verdict(True)})
    final = outcome(chat(daemon, "fill and submit the buildathon form", session="s2"))
    assert final["status"] == "failed" and json.dumps(final["verification"]).count('"met": false') == 1
