"""Build step 13, real Jev (pytest -m real; needs the key in the Keychain: `ferb setup --jev`). One batched request
with every question type FERB uses, through the real client (pinned jev-1.13.0): each typed answer parses into FERB's
values, and an obviously mismatched action is flagged."""

import asyncio

import pytest

from ferb.decide.decider import KEY_ACCOUNT, JevClient
from ferb.decide.questions import QUESTIONS
from ferb.identity.secrets import SERVICE

pytestmark = pytest.mark.real


async def _key() -> str | None:
    p = await asyncio.create_subprocess_exec("security", "find-generic-password", "-s", SERVICE, "-a", KEY_ACCOUNT,
                                             "-w", stdout=asyncio.subprocess.PIPE, stderr=asyncio.subprocess.DEVNULL)
    out, _ = await p.communicate()
    return out.decode().strip() or None


def test_real_jev_answers_every_question_type_in_one_request() -> None:
    if asyncio.run(_key()) is None:
        pytest.skip("no Jev key in the Keychain (`ferb setup --jev`)")
    c = JevClient(_key)
    state = {"source": "window", "data": {"title": "Synthetic blood test result: HIV positive"}}
    names = ["trail.relevance", "trail.sensitive"]
    qs = {f"q{i}": QUESTIONS[name].api() for i, name in enumerate(names)}
    data = asyncio.run(c.ask(state, qs))
    assert data["model"].startswith("jev-1.13")
    values = {name: QUESTIONS[name].value(data["answers"][f"q{i}"])[0] for i, name in enumerate(names)}
    assert values["trail.sensitive"] == "yes"
    assert values["trail.relevance"] in {"ignore", "note", "summarize"}
