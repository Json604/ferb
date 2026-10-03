"""Real model + extension contacts regression against a local fake chat in disposable ChromeLab.
A unique contact is called directly; ambiguous contacts are resolved by an owner choice. No real contacts,
installed profiles or installed native-host configuration are used."""

from typing import Any

import pytest

from tests.conftest import Answers, RunningDaemon
from tests.chromelab import lab, rd, site, chat, problems, shown  # noqa: F401

pytestmark = pytest.mark.real
def test_requested_call_to_one_matching_contact_needs_no_repeat_permission(rd: RunningDaemon, site: tuple[str, list[str]]) -> None:
    url, called = site
    called.clear()
    watch = Answers(rd, choice=None)
    try:
        evs = chat(rd, f"in my Lab profile, open {url}/one and video call Rahul")
    finally:
        watch.stop()
    out = shown(evs)
    assert called == ["Rahul Verma"], (called, out)
    assert not watch.asked, "the request authorizes the call and there is only one matching contact"


def test_two_matching_contacts_are_asked_about(rd: RunningDaemon, site: tuple[str, list[str]]) -> None:
    url, called = site
    called.clear()
    since = rd.daemon.clock.now().isoformat()
    pick = Answers(rd, choice=1)
    try:
        evs = chat(rd, f"in my Lab profile, open {url}/two and video call Rahul")
    finally:
        pick.stop()
    asked = [ap for ap in pick.asked if ap.options]
    assert asked and asked[0].reason == "Which Rahul did you mean?", (pick.asked, called, shown(evs), problems(rd, since))
    assert {o.split(" · ")[0] for o in asked[0].options} == {"Rahul Sharma", "Rahul Verma"}, asked[0].options
    first = asked[0].options[0].split(" · ")[0]
    assert called == [first], (called, shown(evs), problems(rd, since))                     # the one the owner picked, nobody else
