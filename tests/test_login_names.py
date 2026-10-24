"""L36 §5: a username the owner gives is saved, from their own words only, as the site's login record's username.

Live 2026-10-24 06:31: "the username is …", said while correcting the running visa task, was saved nowhere; at 12:34 a
plan step of a new visa task typed the owner's email as the username."""
import asyncio
import json
import threading
from typing import Any

import pytest

from ferb.core.types import Identity, ToolContext
from ferb.identity.owner_secrets import OwnerSecrets
from ferb.orchestrator.plans import Plan, Step
from ferb.orchestrator.schemas import Remember
from tests.conftest import chat, done, install, outcome, sup, task
from tests.mackit import gateway_call as call, grant_task as grant
from tests.support.hidden_input import asked, keychain_helper
from tests.support.live_steer import StalledWorker, control_turn
from tests.support.memory_items import journal, owner_fact, run

VISA = "https://www.usvisascheduling.com"
SITE = "https://usvisascheduling.com"
ATLAS = "https://atlasauth.b2clogin.com"          # the visa site's single sign-on form (I14)


def entry(content: str = "Asha Rao", origin: str = VISA, quote: str = "the username is Asha Rao") -> Remember:
    return Remember(kind="fact", subject="owner", key="usvisascheduling-username", content=content, quote=quote,
                    login_origin=origin)


def remember(d: Any, items: list[Remember], own: str, sites: frozenset[str]) -> list[dict[str, str]]:
    src = journal(d, "chat.message", {"text": own, "untrusted_parts": 0})
    return list(d.run(d.daemon._remember(items, own, src, sites)))


def username(d: Any, place: str = VISA) -> str | None:
    return run(d, lambda: OwnerSecrets(d.daemon.home).username(place))


def refusals(d: Any) -> list[str]:
    return [str(e.data["why"]) for e in run(d, lambda: d.daemon._journal().of_kind("memory.login_refused"))]


def test_a_username_in_the_owners_words_for_the_tasks_site_is_saved(daemon: Any) -> None:
    lines = remember(daemon, [entry()], "the username is asha rao", frozenset({"usvisascheduling.com"}))
    [item] = run(daemon, lambda: daemon.daemon.memory.items("active"))
    assert (item.origin, item.content) == ("owner_stated", "the username is asha rao")
    assert username(daemon) == "asha rao"                            # the owner's own spelling
    assert lines[0]["line"] == ("Remembered: the username is asha rao\n"
                                "Saved as your login name for www.usvisascheduling.com.")
    record = run(daemon, lambda: OwnerSecrets(daemon.daemon.home).records()[VISA])
    assert record["username_source"] == "memory:" + item.id
    assert refusals(daemon) == []


def test_a_site_the_owner_typed_names_it_too(daemon: Any) -> None:
    remember(daemon, [entry()], "the username is Asha Rao on usvisascheduling.com", frozenset())
    assert username(daemon) == "Asha Rao"


@pytest.mark.parametrize("item, sites, why", [
    (entry(), frozenset(), "the site isn't one you named"),                              # no site named anywhere
    (entry(content="Asha R."), frozenset({"usvisascheduling.com"}),                       # not the owner's words
     "the username isn't in your words"),
    (entry(origin="http://www.usvisascheduling.com"), frozenset({"usvisascheduling.com"}),  # not HTTPS
     "the site isn't an https:// address"),
    (entry(origin="app:com.apple.Notes"), frozenset({"usvisascheduling.com"}),            # an app: ordinary memory
     "the site isn't an https:// address"),
    (entry(origin="https://evil.example"), frozenset({"usvisascheduling.com"}),           # another site
     "the site isn't one you named"),
    (entry(origin="https://ais.usvisascheduling.com"), frozenset({"usvisascheduling.com"}),  # exact, "www." aside
     "the site isn't one you named"),
], ids=["no-site", "not-owner-words", "http", "app", "other-site", "subdomain"])
def test_no_username_is_saved_without_the_owners_words_and_a_named_https_site(daemon: Any, item: Remember,
                                                                             sites: frozenset[str], why: str) -> None:
    """The memory itself is still saved (or offered as FERB's wording); only the login record's username is refused,
    and why is journaled for the trace."""
    lines = remember(daemon, [item], "the username is Asha Rao", sites)
    assert username(daemon, VISA) is None and username(daemon, str(item.login_origin)) is None
    assert "login name" not in json.dumps(lines)
    assert refusals(daemon) == [why]


def test_a_username_the_redactor_hid_is_never_saved(daemon: Any) -> None:
    """Chat text is redacted at capture: a username shaped like a secret reaches the planner as "[redacted]"; FERB never
    saves the marker (or the word inside it) as the account's name."""
    remember(daemon, [entry(content="[redacted]", quote="the username is [redacted]")], "the username is [redacted]",
             frozenset({"usvisascheduling.com"}))
    assert username(daemon) is None
    assert refusals(daemon) == ["the username was hidden as a secret"]


def test_forgetting_the_memory_clears_the_username(daemon: Any) -> None:
    """L36 §5: the login record's username goes with the memory it came from."""
    remember(daemon, [entry()], "the username is Asha Rao", frozenset({"usvisascheduling.com"}))
    [item] = run(daemon, lambda: daemon.daemon.memory.items("active"))
    run(daemon, lambda: daemon.daemon._forget_memory(item.id))
    daemon.run(daemon.daemon._settle_forgets())
    assert username(daemon) is None


def test_retracting_the_memory_clears_the_username(daemon: Any) -> None:
    remember(daemon, [entry()], "the username is Asha Rao", frozenset({"usvisascheduling.com"}))
    [item] = run(daemon, lambda: daemon.daemon.memory.items("active"))
    daemon.run(daemon.daemon._memory_retract(None, {"id": item.id}))
    assert username(daemon) is None
    assert run(daemon, lambda: daemon.daemon.memory.get(item.id)).status == "retracted"


def _ping() -> Any:
    async def pong() -> str:
        return "pong"
    return pong()


@pytest.mark.parametrize("change", ["remember", "retract", "forget"])
def test_a_login_save_in_progress_never_stalls_the_event_loop(daemon: Any, change: str) -> None:
    """Coordinator ruling: setting and clearing a username run in a worker thread, as Keychain saves do. While another
    thread holds the handle file's lock (a hidden input being saved), the change waits there and the event loop keeps
    answering."""
    from ferb.identity import owner_secrets
    item = None
    if change != "remember":
        remember(daemon, [entry()], "the username is Asha Rao", frozenset({"usvisascheduling.com"}))
        [item] = run(daemon, lambda: daemon.daemon.memory.items("active"))
    src = journal(daemon, "chat.message", {"text": "the username is Asha Rao", "untrusted_parts": 0})
    held, release = threading.Event(), threading.Event()

    def save_in_progress() -> None:
        with owner_secrets._write_lock:
            held.set()
            release.wait(10)
    holder = threading.Thread(target=save_in_progress)
    holder.start()
    assert held.wait(5)
    try:
        if change == "remember":
            work = daemon.daemon._remember([entry()], "the username is Asha Rao", src,
                                           frozenset({"usvisascheduling.com"}))
        elif change == "retract":
            work = daemon.daemon._memory_retract(None, {"id": item.id})
        else:
            async def forget() -> None:
                daemon.daemon._forget_memory(item.id)
                await daemon.daemon._settle_forgets()
            work = forget()
        pending = asyncio.run_coroutine_threadsafe(work, daemon.loop)
        assert daemon.run(_ping(), timeout=2) == "pong"                 # the loop answers while the change waits
        assert not pending.done()
    finally:
        release.set()
        holder.join(5)
    pending.result(5)
    assert username(daemon) == ("Asha Rao" if change == "remember" else None)


@pytest.mark.parametrize("words, account", [
    ("check my visa appointment on usvisascheduling.com", "Asha Rao"),
    ("check my visa appointment", None),                                       # the site isn't in the owner's words
    ("sign in at atlasauth.b2clogin.com for usvisascheduling.com", None),      # the form is one of the typed sites
], ids=["typed", "not-typed", "form-typed"])
def test_the_first_sign_in_on_a_form_names_the_account_through_the_tasks_typed_sites(
        daemon: Any, monkeypatch: pytest.MonkeyPatch, words: str, account: str | None) -> None:
    """Coordinator ruling on concern 1: before any entry is saved at the sign-in form, the hidden-input question and
    secrets.list read the username through the current task's owner-typed sites (`Gateway._login_sites`: the same
    provenance as a stored link). They only read: no link is written until an entry is saved."""
    from ferb.policy.targets import Request, Targets
    keychain_helper(monkeypatch)
    run(daemon, lambda: OwnerSecrets(daemon.daemon.home).set_username(SITE, "Asha Rao", "memory:x"))
    owner = grant(daemon, "q", Identity.OWNER, {"task:read"}, words)
    run(daemon, lambda: daemon.daemon.state.requests.record(
        Request("req-q", words, Targets.of(sites=["usvisascheduling.com", "atlasauth.b2clogin.com"]))))
    ctx = ToolContext("q", "w-q", "req-q", Identity.OWNER, frozenset({"task:read"}), "dev", None, "/tmp", (), ())
    waiting = asyncio.run_coroutine_threadsafe(daemon.daemon.gateway.ask_secret(ctx, ATLAS, remember="password"),
                                               daemon.loop)
    ap = asked(daemon)
    try:
        shown = " (account: Asha Rao)" if account else ""
        assert ap.summary == f"For atlasauth.b2clogin.com only{shown}: “password” · saved in your Mac's Keychain"
    finally:
        daemon.call_soon(daemon.daemon.state.approvals.resolve, ap.id, False)
        assert waiting.result(5) is None
    listed = call(daemon, owner, "secrets.list", place=ATLAS)["content"]
    assert listed["username"] == account
    assert ("linked" in listed) == bool(account)
    assert run(daemon, lambda: OwnerSecrets(daemon.daemon.home).records()) == {SITE: {
        "username": "Asha Rao", "username_source": "memory:x", "sites": [], "entries": {}}}     # nothing written


def test_a_new_tasks_site_names_the_site_for_a_username_given_with_it(daemon: Any) -> None:
    """The turn that starts a task: its targets name the site (`Supervisor._turn_sites`), though the owner didn't type
    it."""
    turn = json.loads(sup("task", "Logging in.", task("Log in to usvisascheduling.com", sites=("usvisascheduling.com",))))
    turn["remember"] = [{"kind": "fact", "subject": "owner", "key": "usvisascheduling-username", "content": "Asha Rao",
                         "quote": "the username is Asha Rao", "login_origin": VISA}]
    install(daemon, [json.dumps(turn)], {"worker": done("Signed in.")})
    chat(daemon, "log in to the visa site, the username is Asha Rao")
    assert username(daemon) == "Asha Rao"


def test_a_continue_of_an_ended_task_names_its_saved_site(daemon: Any) -> None:
    """A control on a task that has ended: its saved spec's sites name the site (`Supervisor._turn_sites`)."""
    script = install(daemon, [sup("task", "Logging in.", task("Log in to usvisascheduling.com",
                                                              sites=("usvisascheduling.com",)))],
                     {"worker": done("Signed in.")})
    tree_id = outcome(chat(daemon, "log in to the visa site", session="visa"))["task_id"]
    script.sup.append(control_turn("Log in to the visa site again", tree_id, "continue", remember=[{
        "kind": "fact", "subject": "owner", "key": "usvisascheduling-username", "content": "Asha Rao",
        "quote": "the username is Asha Rao", "login_origin": VISA}]))
    chat(daemon, "the username is Asha Rao, carry on", session="visa")
    assert username(daemon) == "Asha Rao"


def _step_context(d: Any, token: str) -> ToolContext:
    """The ToolContext the gateway gives a run's tool call: its task, worker, request and grant."""
    grant_ = d.daemon.tokens.validate(token)
    rec = d.daemon.tasks.get(grant_.task_id)
    return ToolContext(grant_.task_id, grant_.worker_id, rec.task.request_id, grant_.identity, grant_.scopes, "dev",
                       None, "/tmp", (), ())


def test_a_username_said_in_a_live_correction_reaches_the_next_tasks_login(daemon: Any,
                                                                          monkeypatch: pytest.MonkeyPatch) -> None:
    """The spec's username case, live 2026-10-24: a steer of the running visa task saves the fact and the site's
    username. A later task the owner typed usvisascheduling.com for signs in on the single sign-on form at
    atlasauth.b2clogin.com: its first hidden input already names the account (through the task's typed sites), the
    entry saved there makes the form's record list the site, so the next one names it too and secrets.list for the
    form returns the username after the task; the login step got the fact in its memory."""
    keychain = keychain_helper(monkeypatch)
    for i in range(6):
        owner_fact(daemon, f"hose_{i}", f"the garden hose note {i}")
    worker = StalledWorker(monkeypatch)
    script = install(daemon, [sup("task", "Logging in.", task("Log in to usvisascheduling.com",
                                                              sites=("usvisascheduling.com",)))])
    first: list[dict[str, Any]] = []
    thread = threading.Thread(target=lambda: first.extend(chat(daemon, "log in to the visa site", session="visa")))
    thread.start()
    try:
        assert worker.started.wait(5)
        tree_id = worker.handles[0].task_id.rsplit(".", 1)[0]
        # the planner restates the task without its sites: the sites the task named before the steer still count
        script.sup.append(control_turn("Log in to usvisascheduling.com as Asha Rao", tree_id, "steer", remember=[{
            "kind": "fact", "subject": "owner", "key": "usvisascheduling-username", "content": "Asha Rao",
            "quote": "the username is Asha Rao", "login_origin": SITE}]))
        events = chat(daemon, "the username is Asha Rao", session="visa", timeout=5)
    finally:
        worker.release.set()
        thread.join(5)
    reply = [e for e in events if e["type"] == "reply"][-1]["text"]
    assert reply.endswith("Remembered: the username is Asha Rao\nSaved as your login name for usvisascheduling.com.")
    assert username(daemon, SITE) == "Asha Rao"

    shown: list[str] = []

    def sign_in(token: str) -> str:
        """The login step on the sign-in form: its hidden inputs, as browser_secret asks them (D12)."""
        ctx = run(daemon, lambda: _step_context(daemon, token))
        for name, value in (("password", "hidden-canary-1"), ("What is your favorite food?", None)):
            waiting = asyncio.run_coroutine_threadsafe(
                daemon.daemon.gateway.ask_secret(ctx, ATLAS, remember=name), daemon.loop)
            ap = asked(daemon)
            shown.append(ap.summary)
            daemon.call_soon(daemon.daemon.state.approvals.resolve, ap.id, value is not None, None, value)
            assert waiting.result(5) == value
        return done("Signed in.")
    spec = task("Log in to usvisascheduling.com and read the appointment page", sites=("usvisascheduling.com",))
    spec["plan"] = Plan(steps=[Step(id="login", kind="do", goal="Sign in on the login page")]).model_dump()
    script.sup.append(sup("task", "On it.", spec))
    script.workers["step.login"] = sign_in
    chat(daemon, "check my visa appointment on usvisascheduling.com", session="other")
    assert keychain == {f"{ATLAS} · password": "hidden-canary-1"}
    assert run(daemon, lambda: OwnerSecrets(daemon.daemon.home).records()[ATLAS]["sites"]) == ["usvisascheduling.com"]
    assert all(line.startswith("For atlasauth.b2clogin.com only (account: Asha Rao): ") for line in shown), shown
    step = next(ctx for _, key, ctx, _ in script.contexts if key == "step.login")
    assert "the username is Asha Rao" in step
    listed = call(daemon, grant(daemon, "q", Identity.OWNER, {"task:read"}, None), "secrets.list", place=ATLAS)
    assert listed["content"]["username"] == "Asha Rao"
    assert listed["content"]["entries"] == ["password"]
