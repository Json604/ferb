"""Calendar/WhatsApp audit: inspect recorded observations and recover unfinished coverage."""

import json

import pytest

from tests.conftest import FakeCap, chat, cond, done, install, outcome, sup, task, tool


def verdict(met, evidence="Observed the requested source."):
    return json.dumps({"results": [{"index": 0, "met": met, "evidence": evidence}]})


def dispatch(action, instructions="", question=""):
    return json.dumps({"action": action, "worker_type": "default",
                       "instructions": instructions, "question": question})


def test_unknown_coverage_returns_to_planner_and_can_be_completed(daemon):
    missing = "Only the last chat was visible; the other groups and five-day boundary need inspection."
    script = install(daemon, [sup("task", "Checking", task(conds=(cond("Review all official groups for five days"),)))],
                     {"worker": [done("One group checked."), done("All requested groups checked.")],
                      "worker.verify": [verdict(None, missing), verdict(True)],
                      "worker.dispatch": dispatch("retry", "Inspect the remaining groups and date boundary.")})
    result = outcome(chat(daemon, "Check official group messages for the last five days"))
    assert result["verified"] and result["answer"] == "All requested groups checked."
    recovery = next(c for _, key, c, _ in script.contexts if key == "worker.dispatch")
    assert missing in recovery and '"met": null' in recovery


@pytest.mark.parametrize("action,status", [("ask", "needs_user"), ("finish", "failed")])
def test_unknown_coverage_is_not_done_when_planner_cannot_complete_it(daemon, action, status):
    install(daemon, [sup("task", "Checking", task(conds=(cond("Review official groups"),)))],
            {"worker": done("A few chats checked."),
             "worker.verify": verdict(None, "Could not identify the remaining official groups."),
             "worker.dispatch": dispatch(action, "I could only inspect one group.", "Which community is official?")})
    result = outcome(chat(daemon, "Check official groups"))
    assert result["status"] == status and not result["verified"]
    assert ("Which community is official?" if action == "ask" else "Could not identify the remaining official groups.") in result["answer"]


@pytest.mark.parametrize("observed", [None, "owner@example.com", "different@example.com"])
def test_account_condition_is_judged_from_evidence_instead_of_alias_equality(daemon, observed):
    calls = []
    async def account(task_id, site):
        calls.append((task_id, site))
        return observed
    daemon.daemon.supervisor.checks.account_of = account
    condition = cond("Use owner account, or explain if unavailable", "account",
                     site="calendar.example.com", expected="owner")
    script = install(daemon, [sup("task", "Checking", task(conds=(condition,)))],
                     {"worker": done("I inspected the requested account or explained the limitation."),
                      "worker.verify": verdict(True, "Account label and stated scope match the condition.")})
    result = outcome(chat(daemon, "Check my owner calendar"))
    assert result["verified"] and result["verification"][0]["path"] == "verifier"
    context = next(c for _, key, c, _ in script.contexts if key == "worker.verify")
    assert "Use owner account, or explain if unavailable" in context
    assert not calls  # an arbitrary first-tab account is not identity evidence


def test_a_check_needing_a_click_is_unknown_and_a_worker_gathers_it(daemon):
    """W4: the checker can't navigate native views by clicking (code refuses it, V3); it reports the condition
    unknown with what it would need, and the follow-up worker opens the view and gathers the evidence."""
    navigate = FakeCap("mac.act", "navigate", {"mac:act"}, content={"view": "Official announcements"})
    daemon.daemon.registry.capabilities[navigate.name] = navigate
    need = "Opening the next group needs a click, which the checker can't make; the worker should open and read it."

    def checker(token):
        result = tool(daemon.sock, token, "mac.act", text="Open the next group for inspection")
        assert result["is_error"] and "read-only" in result["content"]["error"], result
        return verdict(None, need)

    def worker(token):
        result = tool(daemon.sock, token, "mac.act", text="Open the next group")
        assert not result["is_error"], result
        assert result["content"]["view"] == "Official announcements"
        return done("Opened and read the next group.")

    script = install(daemon, [sup("task", "Checking", task(conds=(cond("Read the group"),)))],
                     {"worker": [done("Read the group."), worker], "worker.verify": [checker, verdict(True)],
                      "worker.dispatch": dispatch("retry", "Open the next group and read it.")})
    result = outcome(chat(daemon, "Read official announcements"))
    assert result["verified"] and navigate.calls == 1                 # only the worker clicked
    recovery = next(c for _, key, c, _ in script.contexts if key == "worker.dispatch")
    assert need in recovery and '"met": null' in recovery
    offered = [set(t) for _, key, _, t in script.contexts if key == "worker.verify"]
    assert len(offered) == 2 and all("task.evidence" in t for t in offered)
    assert not any({"mac.act", "mac.pointer", "browser.act"} & t for t in offered)


def test_checker_can_search_and_page_actual_worker_results_after_view_is_gone(daemon):
    source = "Account: owner@example.com\n" + "earlier rows\n" * 3000 + "Announcement Of SST Clubs — Sep 30"
    snapshot = FakeCap("fake.snapshot", "read", set(), content={"text": source})
    daemon.daemon.registry.capabilities[snapshot.name] = snapshot

    def worker(token):
        result = tool(daemon.sock, token, snapshot.name)
        assert not result["is_error"]
        return done("The named group was visible; its view has since changed.")

    def checker(token):
        listed = tool(daemon.sock, token, "task.evidence", query="Announcement Of SST Clubs")
        assert not listed["is_error"], listed
        records = listed["content"]["records"]
        assert len(records) == 1 and records[0]["tool"] == snapshot.name
        span_id = records[0]["id"]
        parts, offset = [], 0
        while True:
            page = tool(daemon.sock, token, "task.evidence", span_id=span_id, offset=offset, max_chars=4000)
            assert not page["is_error"], page
            parts.append(page["content"]["text"])
            offset = page["content"]["next_offset"]
            if offset is None:
                break
        record = json.loads("".join(parts))
        assert record["output"]["text"] == source
        assert record["input"]["tool"] == snapshot.name and record["ended_at"]
        assert record["task_id"].endswith(".worker")
        return verdict(True, "Recorded source observation includes the group and account.")

    script = install(daemon, [sup("task", "Checking", task(conds=(cond("The named group was read"),)))],
                     {"worker": worker, "worker.verify": checker})
    result = outcome(chat(daemon, "Find the SST announcement group"))
    assert result["verified"]
    context = next(c for _, key, c, _ in script.contexts if key == "worker.verify")
    assert "task_evidence" in context


def test_evidence_pagination_redaction_and_task_isolation(daemon):
    from ferb.core.types import Identity, ToolContext

    async def inspect():
        d = daemon.daemon
        d._add_secret_value("sensitive-test-token")
        for index in range(3):
            s = d.spans.start("tool.call", task_id="this-task", input={"tool": "fake.read", "args": {"page": index}})
            if index < 2:
                d.spans.finish(s, output={"text": "visible name sensitive-test-token", "page": index},
                               error="disconnected" if index else None)
        hidden = d.spans.start("tool.call", task_id="this-task-unrelated", input={"tool": "fake.read"})
        d.spans.finish(hidden, output={"text": "PRIVATE OTHER TASK"})
        nested = d.spans.start("tool.call", task_id="this-task", input={"tool": "task.evidence"})
        d.spans.finish(nested, output={"text": "recursive evidence"})
        model = d.spans.start("model.turn", task_id="this-task", input={})
        d.spans.finish(model, output={"text": "MODEL INTERNALS"})
        cap = d.registry.capabilities["task.evidence"]
        ctx = ToolContext("this-task", "test", "request", Identity.OWNER, frozenset({"task:read"}),
                          "test-device", None, str(d.home / "workspace"), (), ())
        page = await cap.run(cap.Input(limit=2), ctx)
        assert len(page.content["records"]) == 2 and page.content["next_after"] is not None
        second = await cap.run(cap.Input(after=page.content["next_after"], limit=2), ctx)
        assert len(second.content["records"]) == 1 and second.content["next_after"] is None
        assert second.content["records"][0]["ended_at"] is None
        first = await cap.run(cap.Input(span_id=page.content["records"][0]["id"]), ctx)
        assert "sensitive-test-token" not in first.content["text"]
        assert "visible name" in first.content["text"]
        assert page.content["records"][1]["error"] == "disconnected"
        other = await cap.run(cap.Input(span_id=hidden.id), ctx)
        assert other.is_error and "PRIVATE OTHER TASK" not in str(other.content)
        search = await cap.run(cap.Input(query="VISIBLE NAME"), ctx)
        assert len(search.content["records"]) == 2
        assert all(x["tool"] == "fake.read" for x in search.content["records"])
    daemon.run(inspect())


def test_success_sounding_finish_cannot_hide_unknown_condition(daemon):
    install(daemon, [sup("task", "Checking", task(conds=(cond("All five days covered"),)))],
            {"worker": done("Recent messages found."), "worker.verify": verdict(None, "Only one day read."),
             "worker.dispatch": dispatch("finish", "I checked your messages.")})
    result = outcome(chat(daemon, "Check five days"))
    assert result["status"] == "failed" and not result["verified"]
    assert "All five days covered" in result["answer"]
