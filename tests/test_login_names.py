"""L36 §5: a username the owner gives is saved, from their own words only, as the site's login record's username.

Live 2026-10-24 06:31: "the username is …", said while correcting the running permit task, was saved nowhere; at 12:34 a
plan step of a new permit task typed the owner's email as the username."""
import asyncio
import json
import threading
from typing import Any

import pytest

from ferb.core.types import Identity, ToolContext
from ferb.identity.owner_secrets import OwnerSecrets
from ferb.orchestrator.plans import Plan, Step
from ferb.orchestrator.schemas import Remember
from tests.conftest import chat, done, install, outcome, sup, task, tool
from tests.mackit import gateway_call as call, grant_task as grant
from tests.support.hidden_input import asked, keychain_helper
from tests.support.live_steer import StalledWorker, control_turn
from tests.support.memory_items import journal, owner_fact, run

PERMIT = "https://www.bookslots.com"
SITE = "https://bookslots.com"
EVIL = "https://evil.example"
ATLAS = "https://appauth.b2clogin.com"          # the permit site's single sign-on form (I14)


def entry(content: str = "Asha Rao", origin: str | None = PERMIT, quote: str = "the username is Asha Rao") -> Remember:
    return Remember(kind="fact", subject="owner", key="bookslots-username", content=content, quote=quote,
                    login_origin=origin)


def remember(d: Any, items: list[Remember], own: str, sites: frozenset[str]) -> list[dict[str, str]]:
    src = journal(d, "chat.message", {"text": own, "untrusted_parts": 0})
    return list(d.run(d.daemon._remember(items, own, src, sites)))


def username(d: Any, place: str = PERMIT) -> str | None:
    return run(d, lambda: OwnerSecrets(d.daemon.home).username(place))


def refusals(d: Any) -> list[str]:
    return [str(e.data["why"]) for e in run(d, lambda: d.daemon._journal().of_kind("memory.login_refused"))]


def test_a_username_in_the_owners_words_for_the_tasks_site_is_saved(daemon: Any) -> None:
    lines = remember(daemon, [entry()], "the username is asha rao", frozenset({"bookslots.com"}))
    [item] = run(daemon, lambda: daemon.daemon.memory.items("active"))
    assert (item.origin, item.content) == ("owner_stated", "the username is asha rao")
    assert username(daemon) == "asha rao"                            # the owner's own spelling
    assert lines[0]["line"] == ("Memory: noted “the username is asha rao” (your words)\n"
                                "Saved as your login name for www.bookslots.com.")
    record = run(daemon, lambda: OwnerSecrets(daemon.daemon.home).records()[PERMIT])
    assert record["username_source"] == "memory:" + item.id
    assert refusals(daemon) == []


def test_a_site_the_owner_typed_names_it_too(daemon: Any) -> None:
    remember(daemon, [entry()], "the username is Asha Rao on bookslots.com", frozenset())
    assert username(daemon) == "Asha Rao"


@pytest.mark.parametrize("item, sites, why", [
    (entry(), frozenset(), "the site isn't one you named"),                              # no site named anywhere
    (entry(content="Asha R."), frozenset({"bookslots.com"}),                       # not the owner's words
     "the username isn't in your words"),
    (entry(origin="http://www.bookslots.com"), frozenset({"bookslots.com"}),  # not HTTPS
     "the site isn't an https:// address"),
    (entry(origin="app:com.apple.Notes"), frozenset({"bookslots.com"}),            # an app: ordinary memory
     "the site isn't an https:// address"),
    (entry(origin="https://evil.example"), frozenset({"bookslots.com"}),           # another site
     "the site isn't one you named"),
    (entry(origin="https://ais.bookslots.com"), frozenset({"bookslots.com"}),  # exact, "www." aside
     "the site isn't one you named"),
], ids=["no-site", "not-owner-words", "http", "app", "other-site", "subdomain"])
def test_no_username_is_saved_without_the_owners_words_and_a_named_https_site(daemon: Any, item: Remember,
                                                                             sites: frozenset[str], why: str) -> None:
    """The memory itself is still saved (or offered as FERB's wording); only the login record's username is refused,
    and why is journaled for the trace. A site the owner didn't name is said in one line of code's (final review I1);
    the other causes stay in the journal (ruling m-2)."""
    from ferb.identity.owner_secrets import site_host
    lines = remember(daemon, [item], "the username is Asha Rao", sites)
    assert username(daemon, PERMIT) is None and username(daemon, str(item.login_origin)) is None
    assert "login name" not in json.dumps(lines)
    assert refusals(daemon) == [why]
    told = [line.get("line", "") for line in lines]
    unnamed = f"Not saved as your username for {site_host(str(item.login_origin))}: you didn't name that site."
    assert ([t.endswith("\n" + unnamed) for t in told] == [True]) == (why == "the site isn't one you named")
    assert ("Not saved as your username" in json.dumps(lines)) == (why == "the site isn't one you named")


@pytest.mark.parametrize("own, content", [
    ("the username is [redacted]", "[redacted]"),
    ("the username is [redacted]", "redacted"),              # review m-1: the word inside the marker
    ("the username is asha[redacted]", "asha"),              # review m-1: a span touching the marker
], ids=["marker", "inside", "touching"])
def test_a_username_the_redactor_hid_is_never_saved(daemon: Any, own: str, content: str) -> None:
    """Chat text is redacted at capture: a username shaped like a secret reaches the planner as "[redacted]"; FERB never
    saves the marker, the word inside it, or a span of the owner's words that touches it, as the account's name."""
    remember(daemon, [entry(content=content, quote=own)], own, frozenset({"bookslots.com"}))
    assert username(daemon) is None
    assert refusals(daemon) == ["the username was hidden as a secret"]


def test_a_failed_login_name_write_never_stops_the_turn(daemon: Any, monkeypatch: pytest.MonkeyPatch) -> None:
    """Review m-2: any error writing the username (a full disk) is journaled as a refusal; the memory and the turn's
    later entries are still saved."""
    def full(self: Any, data: Any) -> None:
        raise OSError(28, "No space left on device")
    monkeypatch.setattr(OwnerSecrets, "_save", full)
    dentist = Remember(kind="person", subject="owner", key="dentist", content="my dentist is Dr. Rao",
                       quote="my dentist is Dr. Rao")
    lines = remember(daemon, [entry(), dentist], "the username is Asha Rao\nmy dentist is Dr. Rao",
                     frozenset({"bookslots.com"}))
    assert [line["line"] for line in lines] == ["Memory: noted “the username is Asha Rao” (your words)",
                                                "Memory: noted “my dentist is Dr. Rao” (your words)"]
    [why] = refusals(daemon)
    assert why.startswith("OSError: ")
    monkeypatch.undo()
    assert username(daemon) is None


def test_forgetting_the_memory_clears_the_username(daemon: Any) -> None:
    """L36 §5: the login record's username goes with the memory it came from."""
    remember(daemon, [entry()], "the username is Asha Rao", frozenset({"bookslots.com"}))
    [item] = run(daemon, lambda: daemon.daemon.memory.items("active"))
    run(daemon, lambda: daemon.daemon._forget_memory(item.id))
    daemon.run(daemon.daemon._settle_forgets())
    assert username(daemon) is None


def test_forgetting_another_version_of_the_fact_clears_the_username(daemon: Any) -> None:
    """Review m-4: a forget takes every version of the key; the username from another version goes with it."""
    remember(daemon, [entry()], "the username is Asha Rao", frozenset({"bookslots.com"}))
    [first] = run(daemon, lambda: daemon.daemon.memory.items("active"))
    remember(daemon, [entry(content="Asha R Rao", quote="my username is Asha R Rao")], "my username is Asha R Rao",
             frozenset())                         # a new version: the record takes its spelling (review m-8)
    daemon.run(daemon.daemon._settle_forgets())
    [later] = run(daemon, lambda: daemon.daemon.memory.items("active"))
    assert later.content == "my username is Asha R Rao" and username(daemon) == "Asha R Rao"
    run(daemon, lambda: daemon.daemon._forget_memory(first.id))
    daemon.run(daemon.daemon._settle_forgets())
    assert username(daemon) is None


def _sources(d: Any) -> dict[str, Any]:
    return {place: record["username_source"]
            for place, record in run(d, lambda: OwnerSecrets(d.daemon.home).records()).items()}


def test_a_new_version_of_the_username_replaces_its_spelling_on_every_record(daemon: Any) -> None:
    """Task 7 review m-8: a later statement of the same fact (the planner reuses its key) supersedes the earlier one,
    and every record whose username came from an earlier version takes the new spelling from the new version, the
    site's other record ("www." aside) included; no record keeps a spelling the owner replaced."""
    remember(daemon, [entry()], "the username is Asha Rao", frozenset({"bookslots.com"}))
    lines = remember(daemon, [entry(content="asha.rao", origin=SITE, quote="the username is asha.rao now")],
                     "the username is asha.rao now", frozenset({"bookslots.com"}))
    daemon.run(daemon.daemon._settle_forgets())
    [later] = run(daemon, lambda: daemon.daemon.memory.items("active"))
    assert lines[0]["line"].endswith("Saved as your login name for bookslots.com.")
    assert (username(daemon, PERMIT), username(daemon, SITE)) == ("asha.rao", "asha.rao")
    assert _sources(daemon) == {PERMIT: "memory:" + later.id, SITE: "memory:" + later.id}
    [updated] = run(daemon, lambda: daemon.daemon._journal().of_kind("memory.login_updated"))
    assert updated.data == {"item": later.id, "places": [PERMIT]}


def test_a_new_version_for_a_site_not_named_now_still_updates_the_record_it_replaces(daemon: Any) -> None:
    """The record's site was one the owner typed when its username was saved; the new spelling is the owner's own words
    for the same fact, so it follows even when this message names no site (review m-8)."""
    remember(daemon, [entry()], "the username is Asha Rao", frozenset({"bookslots.com"}))
    lines = remember(daemon, [entry(content="Asha R Rao", quote="my username is Asha R Rao")],
                     "my username is Asha R Rao", frozenset())
    assert lines[0]["line"] == ("Memory: noted “my username is Asha R Rao” (your words) — replaces “the username is Asha Rao”\n"
                                "Updated your login name for www.bookslots.com.")
    assert username(daemon) == "Asha R Rao"
    assert refusals(daemon) == []                                         # the entry's own place was written


@pytest.mark.parametrize("change", ["no-login-entry", "hidden", "edit", "understanding"])
def test_a_new_version_that_names_no_usable_username_clears_it(daemon: Any, change: str) -> None:
    """Review m-8: a new version of the fact whose username FERB can't take from the owner's words (the planner gave no
    login entry, redaction hid it, an edit in the dashboard or `ferb memory edit`, whose words name no username, or
    FERB's understanding of what they just said: FERB's wording, never their spelling) clears the record's username
    instead of leaving the replaced spelling."""
    remember(daemon, [entry()], "the username is Asha Rao", frozenset({"bookslots.com"}))
    [first] = run(daemon, lambda: daemon.daemon.memory.items("active"))
    if change == "no-login-entry":
        remember(daemon, [entry(content="I changed the permit username", origin=None,
                                quote="I changed the permit username")], "I changed the permit username", frozenset())
    elif change == "hidden":
        remember(daemon, [entry(content="[redacted]", quote="the username is now")], "the username is now [redacted]",
                 frozenset({"bookslots.com"}))
    elif change == "edit":
        daemon.run(daemon.daemon._memory_edit(None, {"id": first.id, "content": "the username is asha.rao"}))
    else:
        src = journal(daemon, "chat.message", {"text": "I changed the permit username", "untrusted_parts": 0})
        run(daemon, lambda: daemon.daemon.memory.save(
            "fact", "owner", "bookslots-username", "The owner's permit username is now asha.rao", "inferred",
            [("journal", src)], 0.6, from_owner=True))
    daemon.run(daemon.daemon._settle_forgets())
    assert run(daemon, lambda: daemon.daemon.memory.get(first.id)).valid_to is not None
    assert username(daemon) is None
    [cleared] = run(daemon, lambda: daemon.daemon._journal().of_kind("memory.login_cleared"))
    assert cleared.data["places"] == [PERMIT]


def test_retiring_the_memory_clears_the_username(daemon: Any) -> None:
    """L36 §6 (Task 7 ledger): a retired item no longer holds; its username goes with it."""
    remember(daemon, [entry()], "the username is Asha Rao", frozenset({"bookslots.com"}))
    [item] = run(daemon, lambda: daemon.daemon.memory.items("active"))
    run(daemon, lambda: daemon.daemon.memory.retire(item.id, "the owner said so"))
    daemon.run(daemon.daemon._settle_forgets())
    assert username(daemon) is None
    assert run(daemon, lambda: daemon.daemon.memory.get(item.id)).status == "active"   # kept as history


def test_a_username_whose_memory_has_ended_is_never_offered(daemon: Any, monkeypatch: pytest.MonkeyPatch) -> None:
    """Review fix round 1: a record's username follows its memory item when it is read (`secrets.list`, the
    hidden-input question): once the item is past the end date the owner gave it, it names no account. Nothing sweeps
    the record, which still holds the name."""
    from datetime import datetime

    from ferb.clock.when import local_zone
    from ferb.orchestrator.schemas import Until
    zone = local_zone()
    daemon.call_soon(daemon.daemon.clock.set_wall, datetime(2026, 10, 24, 10, 0, tzinfo=zone))
    keychain_helper(monkeypatch)
    dated = Remember(kind="fact", subject="owner", key="bookslots-username", content="Asha Rao",
                     quote="the username is Asha Rao", login_origin=PERMIT,
                     until=Until(quote="until the 25th", date="10-25"))
    remember(daemon, [dated], "the username is Asha Rao until the 25th", frozenset({"bookslots.com"}))

    def offered(tid: str) -> tuple[Any, str]:
        owner = grant(daemon, tid, Identity.OWNER, {"task:read"}, "log in to bookslots.com")
        listed = call(daemon, owner, "secrets.list", place=PERMIT)["content"]["username"]
        ctx = ToolContext(tid, f"w-{tid}", f"req-{tid}", Identity.OWNER, frozenset({"task:read"}), "dev", None, "/tmp",
                          (), ())
        waiting = asyncio.run_coroutine_threadsafe(daemon.daemon.gateway.ask_secret(ctx, PERMIT), daemon.loop)
        ap = asked(daemon)
        daemon.call_soon(daemon.daemon.state.approvals.resolve, ap.id, False)
        assert waiting.result(5) is None
        return listed, ap.summary
    listed, summary = offered("q1")
    assert listed == "Asha Rao" and "(account: Asha Rao)" in summary
    daemon.call_soon(daemon.daemon.clock.set_wall, datetime(2026, 10, 26, 0, 1, tzinfo=zone))
    listed, summary = offered("q2")
    assert listed is None and "Asha Rao" not in summary
    assert username(daemon) == "Asha Rao"                                 # read at use: the record is unchanged


def test_a_forget_that_cannot_clear_the_username_says_so(daemon: Any, monkeypatch: pytest.MonkeyPatch) -> None:
    """Review I-3: any error clearing it (a full disk) is journaled with its type, never swallowed."""
    remember(daemon, [entry()], "the username is Asha Rao", frozenset({"bookslots.com"}))
    [item] = run(daemon, lambda: daemon.daemon.memory.items("active"))

    def full(self: Any, data: Any) -> None:
        raise OSError(28, "No space left on device")
    monkeypatch.setattr(OwnerSecrets, "_save", full)
    run(daemon, lambda: daemon.daemon._forget_memory(item.id))
    daemon.run(daemon.daemon._settle_forgets())
    monkeypatch.undo()
    [failed] = run(daemon, lambda: daemon.daemon._journal().of_kind("memory.login_clear_failed"))
    assert failed.data["error"].startswith("OSError: ") and failed.data["items"] == [item.id]
    assert username(daemon) == "Asha Rao"                                 # left, and said


def test_a_retract_during_a_username_write_waits_for_it_and_clears_it(daemon: Any,
                                                                     monkeypatch: pytest.MonkeyPatch) -> None:
    """Review m-3: username writes are serialized (one daemon lock), so a retract that starts while the write is under
    way clears it after the write, never before (when it would find nothing to clear and the write would then leave
    the username of a retracted memory)."""
    src = journal(daemon, "chat.message", {"text": "the username is Asha Rao", "untrusted_parts": 0})
    real, entered, gate = OwnerSecrets.set_username, threading.Event(), threading.Event()

    def held_write(self: Any, *args: Any) -> None:                  # the write, stopped before the handle file's lock
        entered.set()
        gate.wait(10)
        real(self, *args)
    monkeypatch.setattr(OwnerSecrets, "set_username", held_write)
    try:
        writing = asyncio.run_coroutine_threadsafe(daemon.daemon._remember(
            [entry()], "the username is Asha Rao", src, frozenset({"bookslots.com"})), daemon.loop)
        assert entered.wait(5)
        [item] = run(daemon, lambda: daemon.daemon.memory.items("active"))
        retracting = asyncio.run_coroutine_threadsafe(daemon.daemon._memory_retract(None, {"id": item.id}),
                                                      daemon.loop)
        assert daemon.run(_ping(), timeout=2) == "pong"
        with pytest.raises(TimeoutError):
            retracting.result(0.5)                                  # its clear waits for the write
    finally:
        gate.set()
    writing.result(5)
    retracting.result(5)
    assert username(daemon) is None


def test_retracting_the_memory_clears_the_username(daemon: Any) -> None:
    remember(daemon, [entry()], "the username is Asha Rao", frozenset({"bookslots.com"}))
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
        remember(daemon, [entry()], "the username is Asha Rao", frozenset({"bookslots.com"}))
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
                                           frozenset({"bookslots.com"}))
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


FORM_SITES = ["bookslots.com", "appauth.b2clogin.com"]


@pytest.mark.parametrize("place, words, sites, linked", [
    (ATLAS, "check my permit appointment on bookslots.com", FORM_SITES, True),
    (ATLAS, "check my permit appointment", FORM_SITES, False),                    # the site isn't in the owner's words
    (ATLAS, "sign in at appauth.b2clogin.com for bookslots.com", FORM_SITES, False),   # the form is typed
    (EVIL, "check my permit appointment on bookslots.com", ["bookslots.com"], True),   # review I-2
    (ATLAS, "check my permit appointment on bookslots.com and mail it from gmail.com",       # review I-2: two
     [*FORM_SITES, "gmail.com"], False),
], ids=["typed", "not-typed", "form-typed", "other-form-named-source", "two-typed-sites"])
def test_the_first_sign_in_on_a_form_names_the_account_through_the_tasks_typed_sites(
        daemon: Any, monkeypatch: pytest.MonkeyPatch, place: str, words: str, sites: list[str], linked: bool) -> None:
    """Coordinator ruling on concern 1: before any entry is saved at the sign-in form, the hidden-input question and
    secrets.list read the username through the current task's owner-typed site (`Gateway._question_sites`: the same
    provenance as a stored link, and only when the task has exactly one such site, review I-2). A username read
    through a link names its site ("bookslots.com account: …"), so on any other page the owner sees whose
    account it is. They only read: no link is written until an entry is saved."""
    from ferb.daemon.gateway import _site
    from ferb.policy.targets import Request, Targets
    keychain_helper(monkeypatch)
    source = "memory:" + owner_fact(daemon, "bookslots-username", "the username is Asha Rao").id
    run(daemon, lambda: OwnerSecrets(daemon.daemon.home).set_username(SITE, "Asha Rao", source))
    owner = grant(daemon, "q", Identity.OWNER, {"task:read"}, words)
    run(daemon, lambda: daemon.daemon.state.requests.record(Request("req-q", words, Targets.of(sites=sites))))
    ctx = ToolContext("q", "w-q", "req-q", Identity.OWNER, frozenset({"task:read"}), "dev", None, "/tmp", (), ())
    waiting = asyncio.run_coroutine_threadsafe(daemon.daemon.gateway.ask_secret(ctx, place, remember="password"),
                                               daemon.loop)
    ap = asked(daemon)
    try:
        shown = " (bookslots.com account: Asha Rao)" if linked else ""
        assert ap.summary == f"For {_site(place)} only{shown}: “password” · saved in your Mac's Keychain"
        assert ("Account: Asha Rao (saved for bookslots.com)." in ap.reason) == linked
    finally:
        daemon.call_soon(daemon.daemon.state.approvals.resolve, ap.id, False)
        assert waiting.result(5) is None
    listed = call(daemon, owner, "secrets.list", place=place)["content"]
    assert (listed["username"], listed.get("username_site")) == (("Asha Rao", "bookslots.com") if linked
                                                                 else (None, None))
    assert ("linked" in listed) == linked
    assert run(daemon, lambda: OwnerSecrets(daemon.daemon.home).records()) == {SITE: {
        "username": "Asha Rao", "username_source": source, "sites": [], "entries": {}, "kinds": {}}}   # nothing written


def test_a_login_saved_in_a_task_with_two_typed_sites_links_neither(daemon: Any,
                                                                    monkeypatch: pytest.MonkeyPatch) -> None:
    """Re-review N-1: a save stores links only under the one-typed-site rule too. A password saved at Google's
    sign-in form in a task the owner typed bookslots.com and gmail.com for lists neither site, so later tasks
    never show the permit username at Google."""
    from ferb.policy.targets import Request, Targets
    google = "https://accounts.google.com"
    keychain = keychain_helper(monkeypatch)
    source = "memory:" + owner_fact(daemon, "bookslots-username", "the username is Asha Rao").id
    run(daemon, lambda: OwnerSecrets(daemon.daemon.home).set_username(SITE, "Asha Rao", source))
    words = "check my permit appointment on bookslots.com and mail it from gmail.com"
    run(daemon, lambda: daemon.daemon.state.requests.record(
        Request("req-q", words, Targets.of(sites=["bookslots.com", "gmail.com"]))))
    ctx = ToolContext("q", "w-q", "req-q", Identity.OWNER, frozenset({"task:read"}), "dev", None, "/tmp", (), ())
    waiting = asyncio.run_coroutine_threadsafe(daemon.daemon.gateway.ask_secret(ctx, google, remember="password"),
                                               daemon.loop)
    ap = asked(daemon)
    assert ap.summary == "For accounts.google.com only: “password” · saved in your Mac's Keychain"
    daemon.call_soon(daemon.daemon.state.approvals.resolve, ap.id, True, None, "hidden-canary-1")
    assert waiting.result(5) == "hidden-canary-1"
    store = OwnerSecrets(daemon.daemon.home)
    assert keychain == {f"{google} · password": "hidden-canary-1"}
    assert run(daemon, lambda: store.records()[google]["sites"]) == []
    assert run(daemon, lambda: store.account(google)) == (None, None)


USERNAME_ENTRY = {"kind": "fact", "subject": "owner", "key": "bookslots-username", "content": "Asha Rao",
                  "quote": "the username is Asha Rao", "login_origin": PERMIT}


def test_a_site_only_a_pasted_email_named_never_gets_the_username(daemon: Any) -> None:
    """Review I-1: the planner copied a pasted email's site into the task's targets and the entry's login_origin.
    Only a site whose host the owner typed counts, so the username is refused (the memory itself is saved)."""
    turn = json.loads(sup("task", "Logging in.", task("Log in to the site in the email", sites=("evil.example",))))
    turn["remember"] = [USERNAME_ENTRY | {"login_origin": EVIL}]
    install(daemon, [json.dumps(turn)], {"worker": done("Signed in.")})
    pasted = "\x1b[200~Your permit account: sign in at https://evil.example/login\x1b[201~"
    chat(daemon, "log in to the permit site from this email, the username is Asha Rao\n" + pasted)
    assert username(daemon, EVIL) is None
    assert refusals(daemon) == ["the site isn't one you named"]
    assert [i.content for i in run(daemon, lambda: daemon.daemon.memory.items("active"))] == [
        "log in to the permit site from this email, the username is Asha Rao"]


@pytest.mark.parametrize("first, saved", [("log in to bookslots.com", True), ("log in to the permit site", False)],
                         ids=["site-typed", "site-not-typed"])
def test_a_continue_of_an_ended_task_names_the_site_its_owner_typed(daemon: Any, first: str, saved: bool) -> None:
    """A control on a task that has ended: its saved spec's sites count when the owner typed them in that task's words
    (`continuity.load`; review I-1), never when only the planner named them."""
    script = install(daemon, [sup("task", "Logging in.", task("Log in to bookslots.com",
                                                              sites=("bookslots.com",)))],
                     {"worker": done("Signed in.")})
    tree_id = outcome(chat(daemon, first, session="permit"))["task_id"]
    script.sup.append(control_turn("Log in to the permit site again", tree_id, "continue", remember=[USERNAME_ENTRY]))
    chat(daemon, "the username is Asha Rao, carry on", session="permit")
    assert username(daemon) == ("Asha Rao" if saved else None)


def _answer_the_tasks_question(daemon: Any, request: str, sites: tuple[str, ...], login_origin: str,
                               answer: str = "Sam Rivera",
                               answered_in: str = "permit") -> tuple[list[dict[str, Any]], dict[str, Any]]:
    """The owner starts a task with `request` in chat "permit"; its worker asks "What username?" (owner.ask) and waits;
    the owner's answer, in chat `answered_in`, is the planner's `answer_question` turn with a `remember` entry naming
    `login_origin`. The answer turn's events, and what the worker got."""
    import time
    got: dict[str, Any] = {}

    def worker(token: str) -> str:
        got["answer"] = tool(daemon.sock, token, "owner.ask", question="What username?")["content"]
        return done("Signed in.")
    script = install(daemon, [sup("task", "Logging in.", task("Log in to the permit site", sites=sites))],
                     {"worker": worker})
    first = threading.Thread(target=lambda: chat(daemon, request, session="permit"))
    first.start()
    try:
        end = time.monotonic() + 5
        while not (open_ := [q for q in daemon.daemon.state.approvals.pending() if q.capability == "owner.ask"]):
            assert time.monotonic() < end, "the worker never asked"
            time.sleep(0.02)
        script.sup.append(json.dumps({"kind": "reply", "text": "", "task": None,
                                      "answer_question": {"approval_id": open_[0].id, "answer": answer},
                                      "remember": [USERNAME_ENTRY | {"content": answer, "quote": answer,
                                                                     "login_origin": login_origin}]}))
        events = chat(daemon, answer, session=answered_in)
    finally:
        first.join(10)
    return events, got


def test_a_username_given_as_the_answer_to_the_tasks_question_is_saved_for_the_site_its_owner_typed(
        daemon: Any) -> None:
    """Final review I1 (the reviewer's probe): the owner started "log in to bookslots.com", the task asked "What
    username?", and the answer "Sam Rivera" was remembered but never put on the login record: the turn that
    answers a question carries no task and no control, so `_turn_sites` had no site. The answered question's task
    counts (L36 §5: "the task's recorded owner words"), when it is this chat's."""
    events, got = _answer_the_tasks_question(daemon, "log in to bookslots.com", ("bookslots.com",), SITE)
    assert got["answer"] == {"answer": "Sam Rivera"}
    assert username(daemon, SITE) == "Sam Rivera"
    said = "\n".join(str(e.get("text", "")) for e in events)
    assert "Memory: noted “Sam Rivera” (your words)\nSaved as your login name for bookslots.com." in said
    assert refusals(daemon) == []


def test_an_answer_from_another_chat_says_the_site_wasnt_named_in_this_chat(daemon: Any) -> None:
    """Scoped re-review nit (probe P2): a question of a task started in chat "permit", which named bookslots.com,
    answered from chat "other": the username isn't saved (the answered task counts only in its own chat, review m-5),
    and the line says why accurately: the site was named, but not in this chat."""
    events, got = _answer_the_tasks_question(daemon, "log in to bookslots.com", ("bookslots.com",), SITE,
                                             answered_in="other")
    assert got["answer"] == {"answer": "Sam Rivera"} and username(daemon, SITE) is None
    assert refusals(daemon) == ["the site isn't one you named"]
    said = "\n".join(str(e.get("text", "")) for e in events)
    assert ("Memory: noted “Sam Rivera” (your words)\nNot saved as your username for bookslots.com: you didn't name that "
            "site in this chat." in said)


def test_an_answer_naming_a_site_only_outside_content_gave_is_refused_and_said(daemon: Any) -> None:
    """Final review I1: a site only a pasted email named (the planner copied it into the task and the entry) is still
    refused for the username; the fact is remembered, and the owner reads why in one line of code's on the answer."""
    pasted = "\x1b[200~Your permit account: sign in at https://evil.example/login\x1b[201~"
    events, _ = _answer_the_tasks_question(daemon, "log in to the permit site from this email\n" + pasted,
                                           ("evil.example",), EVIL)
    assert username(daemon, EVIL) is None
    assert refusals(daemon) == ["the site isn't one you named"]
    said = "\n".join(str(e.get("text", "")) for e in events)
    assert ("Memory: noted “Sam Rivera” (your words)\nNot saved as your username for evil.example: you didn't name that site."
            in said)
    assert [i.content for i in run(daemon, lambda: daemon.daemon.memory.items("active"))] == ["Sam Rivera"]


def test_a_control_from_another_chat_names_no_site(daemon: Any, monkeypatch: pytest.MonkeyPatch) -> None:
    """Review m-5: the task a control names counts only when it belongs to this chat (`continuity.load` by session),
    live or not, even when this turn's words name one of its sites. (A chat turn never gets that far: an unknown task
    id is repaired, then refused; this pins `_turn_sites` itself.) Review I-1: of the task's sites, only those the
    owner typed count."""
    from ferb.orchestrator.schemas import SupervisorOutcome
    from ferb.orchestrator.supervisor import ChatSession, _Turn
    worker = StalledWorker(monkeypatch)
    install(daemon, [sup("task", "Logging in.", task("Log in to bookslots.com",
                                                     sites=("bookslots.com", "evil.example")))])
    thread = threading.Thread(target=lambda: chat(daemon, "log in to bookslots.com", session="permit"))
    thread.start()
    try:
        assert worker.started.wait(5)
        tree_id = worker.handles[0].task_id.rsplit(".", 1)[0]
        out = SupervisorOutcome.model_validate(json.loads(control_turn("Log in", tree_id, "continue")))
        sites = {chat_id: run(daemon, lambda: daemon.daemon.supervisor._turn_sites(
            out, _Turn("r", ChatSession(chat_id), typed="the username for bookslots.com is Asha Rao")))
            for chat_id in ("permit", "other")}
    finally:
        worker.release.set()
        thread.join(5)
    assert sites == {"permit": frozenset({"bookslots.com"}), "other": frozenset()}


def _step_context(d: Any, token: str) -> ToolContext:
    """The ToolContext the gateway gives a run's tool call: its task, worker, request and grant."""
    grant_ = d.daemon.tokens.validate(token)
    rec = d.daemon.tasks.get(grant_.task_id)
    return ToolContext(grant_.task_id, grant_.worker_id, rec.task.request_id, grant_.identity, grant_.scopes, "dev",
                       None, "/tmp", (), ())


def test_a_username_said_in_a_live_correction_reaches_the_next_tasks_login(daemon: Any,
                                                                          monkeypatch: pytest.MonkeyPatch) -> None:
    """The spec's username case, live 2026-10-24: a steer of the running permit task saves the fact and the site's
    username (the owner typed the site when starting the task: review I-1). A later task the owner typed
    bookslots.com for signs in on the single sign-on form at
    appauth.b2clogin.com: its first hidden input already names the account (through the task's typed sites), the
    entry saved there makes the form's record list the site, so the next one names it too and secrets.list for the
    form returns the username after the task; the login step got the fact in its memory."""
    keychain = keychain_helper(monkeypatch)
    for i in range(6):
        owner_fact(daemon, f"hose_{i}", f"the garden hose note {i}")
    worker = StalledWorker(monkeypatch)
    script = install(daemon, [sup("task", "Logging in.", task("Log in to bookslots.com",
                                                              sites=("bookslots.com",)))])
    first: list[dict[str, Any]] = []
    thread = threading.Thread(target=lambda: first.extend(chat(daemon, "log in to bookslots.com",
                                                               session="permit")))
    thread.start()
    try:
        assert worker.started.wait(5)
        tree_id = worker.handles[0].task_id.rsplit(".", 1)[0]
        # the planner restates the task without its sites: the sites the task named before the steer still count
        script.sup.append(control_turn("Log in to bookslots.com as Asha Rao", tree_id, "continue", remember=[{
            "kind": "fact", "subject": "owner", "key": "bookslots-username", "content": "Asha Rao",
            "quote": "the username is Asha Rao", "login_origin": SITE}]))
        events = chat(daemon, "the username is Asha Rao", session="permit", timeout=5)
    finally:
        worker.release.set()
        thread.join(5)
    reply = [e for e in events if e["type"] == "reply"][-1]["text"]
    assert reply.endswith("Memory: noted “the username is Asha Rao” (your words)\nSaved as your login name for bookslots.com.")
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
    spec = task("Log in to bookslots.com and read the appointment page", sites=("bookslots.com",))
    spec["plan"] = Plan(steps=[Step(id="login", kind="do", goal="Sign in on the login page")]).model_dump()
    script.sup.append(sup("task", "On it.", spec))
    script.workers["step.login"] = sign_in
    chat(daemon, "check my permit appointment on bookslots.com", session="other")
    assert keychain == {f"{ATLAS} · password": "hidden-canary-1"}
    assert run(daemon, lambda: OwnerSecrets(daemon.daemon.home).records()[ATLAS]["sites"]) == ["bookslots.com"]
    assert len(shown) == 2
    assert all(line.startswith("For appauth.b2clogin.com only (bookslots.com account: Asha Rao): ")
               for line in shown), shown
    step = next(ctx for _, key, ctx, _ in script.contexts if key == "step.login")
    system = next(req.system for _, key, req in script.requests if key == "step.login")
    assert "the username is Asha Rao" in step + system        # the owner's words join the core profile (MEM-15)
    listed = call(daemon, grant(daemon, "q", Identity.OWNER, {"task:read"}, None), "secrets.list", place=ATLAS)
    assert listed["content"]["username"] == "Asha Rao"
    assert listed["content"]["username_site"] == "bookslots.com"
    assert listed["content"]["entries"] == ["password"]
