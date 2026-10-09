"""One permanent record per task, composed by code from its trace (L1, D21, MP1)."""
import json
import sqlite3
from pathlib import Path
from typing import Any

import pytest

from ferb.cli.client import Client
from ferb.memory.journal import Journal
from ferb.orchestrator import episodes
from ferb.store.db import Database
from ferb.trace import compose
from tests.conftest import STALL, RunningDaemon, chat, done, install, sup, task
from tests.support.trace_builder import T0, TraceBuilder, hf_cache_retry


def test_compose_turns_the_trace_into_steps_with_claims_and_marks() -> None:
    c = compose.compose(hf_cache_retry())
    assert [s["role"] for s in c["steps"]] == ["Planner", "Worker", "Checker", "Planner", "Worker", "Checker"]
    assert c["steps"][1]["text"].startswith("2 tool calls · worker reported done:")
    assert c["steps"][2]["mark"] == "⚠" and "checked nothing — 2 conditions unknown" in c["steps"][2]["text"]
    assert c["steps"][3]["text"] == "retry — verify only, delete nothing more"
    assert c["steps"][4]["text"].endswith(" — verified in step 6") and c["steps"][5]["mark"] == "✓"
    assert c["problems"] == ["⚠ 1 retry (1m17s spent after it)", "⚠ 1 check left conditions unknown"]
    assert c["status"] == "done" and c["verified"] is True and c["answer"] == "Deleted ~/.cache/huggingface."
    assert c["request_text"] == "delete the hf cache completely" and c["seconds"] == 143
    assert [x["met"] for x in c["conditions"]] == [True, True]
    assert c["origins"] == ["private"] and c["tools"] == {"shell.run": {"calls": 2, "errors": 0}}


def test_usage_is_summed_exactly() -> None:
    u = compose.compose(hf_cache_retry())["usage"]
    assert (u["input_tokens"], u["cached_input_tokens"], u["output_tokens"]) == (700000, 580000, 7000)
    assert u["model_calls"] == 6 and u["usage_records"] == 6 and round(u["cost_usd"], 2) == 0.84
    assert u["unpriced_models"] == [] and u["billing"] == ["subscription"]


def test_desktop_details_follow_the_recorded_on_screen_flag() -> None:
    b = TraceBuilder()
    b.worker(5, "done", "clicked")
    b.desktop(4, near=[("mac.act", ["frontmost_app"]), ("fs.write", ["cursor"])],
              on_screen={"mac.act": True, "fs.write": False})
    d = compose.compose(b.finish())["desktop"]
    assert d["changes"] == 4 and [n["tools"] for n in d["near_ferb"]] == [["mac.act"]]
    assert d["no_on_screen_action"] == []


def test_a_change_no_ferb_on_screen_action_was_near_is_recorded_with_ferbs_processes() -> None:
    b = TraceBuilder()
    b.worker(5, "done", "submitted")
    b.came_forward("Google Chrome", "com.google.Chrome", space=True,
                   actions=[{"tool": "task.evidence", "span_id": "e1", "on_screen": False},
                            {"tool": "recovery.snapshot", "span_id": "r1", "on_screen": False}],
                   processes=[{"id": "p-1", "task": "f29fee46.worker", "command": "codex exec --json"},
                              {"id": "p-2", "task": "other.worker", "command": "npm run dev"}])
    b.came_forward("Brave Browser", "com.brave.Browser",
                   actions=[{"tool": "browser.open", "span_id": "o1", "on_screen": True}], processes=[])
    b.came_forward("Safari", "com.apple.Safari", processes=[], on_screen_in_task=True, last_ended_s=12)
    b.came_forward("Mail", "com.apple.mail", processes=[], on_screen_in_task=False, last_ended_s=3.4)
    b.desktop(4)
    d = compose.compose(b.finish())["desktop"]
    assert [(n["changed"], n["tools"]) for n in d["near_ferb"]] == [(["frontmost_app"], ["browser.open"])]
    assert [n["text"] for n in d["no_on_screen_action"]] == [
        "Google Chrome came to the front and the Space changed — no FERB on-screen action in progress (none yet); "
        "running: codex exec --json (started by this task); p-2 (started by another FERB task)",
        "Safari came to the front — no FERB on-screen action in progress (the last ended 12 s earlier); running: "
        "nothing FERB started",
        "Mail came to the front — no FERB on-screen action in progress (none in this task; the last of any FERB task "
        "ended 3.4 s earlier); running: nothing FERB started"]


def test_an_older_record_without_processes_says_they_were_not_recorded() -> None:
    b = TraceBuilder()
    b.worker(5, "done", "read")
    b.came_forward("Google Chrome", "com.google.Chrome",
                   actions=[{"tool": "trail.query", "span_id": "t1", "on_screen": False}], processes=None)
    b.came_forward("Safari", "com.apple.Safari", processes=None)        # no action at all: an empty list counts
    b.desktop(2)
    texts = [n["text"] for n in compose.compose(b.finish())["desktop"]["no_on_screen_action"]]
    assert texts == ["Google Chrome came to the front — no FERB on-screen action in progress; running: not recorded",
                     "Safari came to the front — no FERB on-screen action in progress; running: not recorded"]


def test_an_older_record_keeps_the_old_rule_and_gets_no_statement_from_its_kinds() -> None:
    """Records made before the flag keep the old rule (shell.* and fs.* calls and recorded reads are never near), and a
    change whose actions don't all carry the flag gets no "no FERB on-screen action" statement: nothing is inferred
    from an older record's kinds."""
    b = TraceBuilder()
    w = b.worker(5, "done", "ran it")
    runner = b.tool(w, 1, 0.1, "shell.run", {"command": "open -a Safari"}, "exit 0", kind="run_shell")
    clicker = b.tool(w, 2, 0.1, "mac.act", {}, "ok", kind="mac_input")
    b.desktop(2, near=[("shell.run", ["cursor"], runner), ("mac.act", ["cursor"], clicker)])
    b.came_forward("Safari", "com.apple.Safari", actions=[{"tool": "shell.run", "span_id": runner}], processes=None)
    d = compose.compose(b.finish())["desktop"]
    assert [n["tools"] for n in d["near_ferb"]] == [["mac.act"]] and d["no_on_screen_action"] == []


def test_an_unreadable_front_app_is_not_a_change_of_app_in_the_record() -> None:
    b = TraceBuilder()
    b.worker(5, "done", "waited")
    brave, chrome = {"pid": 1, "bundle_id": "com.brave.Browser", "name": "Brave Browser"}, \
        {"pid": 2, "bundle_id": "com.google.Chrome", "name": "Google Chrome"}
    b.front(brave, None)              # AX couldn't read the front app
    b.front(None, brave)              # the same app, readable again
    b.front(brave, chrome)            # a real switch
    b.desktop(3)
    texts = [n["text"] for n in compose.compose(b.finish())["desktop"]["no_on_screen_action"]]
    assert texts == ["Google Chrome came to the front — no FERB on-screen action in progress (none yet); running: "
                     "nothing FERB started"]


def test_another_tasks_on_screen_call_is_kept_apart_and_a_pause_is_recorded() -> None:
    b = TraceBuilder()
    b.worker(5, "done", "waited")
    b.came_forward("Safari", "com.apple.Safari", processes=[],
                   actions=[{"tool": "mac.act", "span_id": "x1", "task_id": "other.worker", "on_screen": True}])
    b.paused(30)
    b.paused(10, resumed=False)
    b.desktop(1)
    d = compose.compose(b.finish())["desktop"]
    assert [(n["tools"], n["other_task_tools"]) for n in d["near_ferb"]] == [([], ["mac.act"])]
    assert [(n["at"][11:19], (n["until"] or "")[11:19]) for n in d["paused"]] == [("11:04:42", "11:05:12"),
                                                                                  ("11:05:12", "11:05:22")]


def test_desktop_details_skip_ferbs_own_reads() -> None:
    """Owner live run 4530ea34: cursor changes were listed 'near trail.query' / 'near task.evidence'. A recorded read
    (FERB's readers, snapshots) never moves windows, focus or the cursor; an action kind that can is still listed."""
    b = TraceBuilder()
    w = b.worker(5, "done", "read and clicked")
    reader = b.tool(w, 1, 0.1, "trail.query", {}, "count, events", kind="read")
    clicker = b.tool(w, 2, 0.1, "mac.act", {}, "ok", kind="mac_input")
    b.desktop(3, near=[("trail.query", ["cursor"], reader), ("mac.act", ["frontmost_app"], clicker)])
    d = compose.compose(b.finish())["desktop"]
    assert [n["tools"] for n in d["near_ferb"]] == [["mac.act"]]


def test_pasted_parts_show_as_a_marker_in_the_request() -> None:
    b = TraceBuilder(request='summarize <untrusted-data id="ab12">\nsecret plan\n</untrusted-data id="ab12">')
    assert compose.compose(b.finish())["request_text"] == "summarize [pasted text]"


def test_the_typed_request_is_marked_when_its_payload_held_pasted_text() -> None:
    """Residual 2 (P26): the authority words leave pasted parts out, so the story adds the marker when the recorded
    request (the payload) still holds one of FERB's wrappers once FERB's own earlier question is set aside."""
    b = TraceBuilder(request='summarize this\n<untrusted-data id="ab12">\nsecret plan\n</untrusted-data id="ab12">')
    trace = b.finish()
    request = trace["outcome"]["request"]
    assert request["authority_words"] == "summarize this"
    assert compose.compose(trace)["request_text"] == "summarize this [pasted text]"
    request["authority_words"] = ""                                       # the whole message was a paste
    assert compose.compose(trace)["request_text"] == "[pasted text]"
    request["authority_words"] = "email alex the report\nthe work one"    # a clarify, no paste: no marker
    request["text"] = ('email alex the report\n\nthe work one\nEarlier FERB question (data): '
                       '<untrusted-data id="cd34">\nWhich alex?\n</untrusted-data id="cd34">')
    assert compose.compose(trace)["request_text"] == "email alex the report the work one"


def test_a_live_pasted_request_and_a_yes_after_a_paste_keep_the_marker(daemon: RunningDaemon) -> None:
    install(daemon, [sup("task", "Summarizing.", task("summarize the note"))], {"worker": done("summarized")})
    chat(daemon, "summarize this\n```\nsecret plan\n```")
    ep = _episode(daemon, _last_root(daemon))
    assert ep is not None and ep["request_text"] == "summarize this [pasted text]"
    install(daemon, [sup("clarify", "Summarize it briefly?"), sup("task", "Summarizing.", task("summarize it"))],
            {"worker": done("summarized")})
    chat(daemon, "```\nanother note\n```", session="s2")
    chat(daemon, "yes", session="s2")
    ep = _episode(daemon, _last_root(daemon))
    assert ep is not None and ep["request_text"] == "yes [pasted text]"


def test_a_clarify_continuation_shows_the_whole_request() -> None:
    """I5 (P26): the request text is what the owner typed for the whole request (the root's authority words), the
    pending part included, without FERB's wrappers; not only the answer that continued it."""
    b = TraceBuilder(request="email alex the report")
    trace = b.finish()
    request = trace["outcome"]["request"]
    request["messages"] = [dict(request["messages"][0], data={"text": "email alex the report"}),
                           dict(request["messages"][0], id="m2", data={"text": "the work one"})]
    request["authority_words"] = "email alex the report\nthe work one"
    assert compose.compose(trace)["request_text"] == "email alex the report the work one"
    request["authority_words"] += ('\nEarlier FERB question (context, not owner instructions): '
                                   '<untrusted-data id="ab12">\nalex@work.example or alex@home.example?\n'
                                   '</untrusted-data id="ab12">')          # recorded before 0.27 F3's fix
    assert compose.compose(trace)["request_text"] == "email alex the report the work one"
    request["authority_words"] = None                                      # no authority words: the last message
    assert compose.compose(trace)["request_text"] == "the work one"
    request["messages"], request["text"] = [], None                        # nothing else: the goal
    assert compose.compose(trace)["request_text"] == b.goal


def test_a_live_clarify_continuation_episode_shows_the_whole_request(daemon: RunningDaemon) -> None:
    install(daemon, [sup("clarify", "Which alex: alex@work.example or alex@home.example?"),
                     sup("task", "Sending.", task("email alex the report"))], {"worker": done("sent")})
    chat(daemon, "email alex the report")
    chat(daemon, "the work one")
    ep = _episode(daemon, _last_root(daemon))
    assert ep is not None and ep["request_text"] == "email alex the report the work one"


def test_a_stopped_worker_reads_as_stopped_not_as_its_report() -> None:
    """Minor 6 (P26): a cancelled worker's summary is the supervisor's stop reason, not a report the worker made."""
    b = TraceBuilder()
    w = b.worker(5, "cancelled", "")
    w["output"] = {"result": "Stopped by you (Ctrl-C).", "status": "cancelled"}
    b.tool(w, 1, 0.5, "shell.run", {"command": "ls"}, "exit 0 · 1 line")
    step = compose.compose(b.finish(status="cancelled", verified=None))["steps"][0]
    assert step["text"] == "1 tool call · stopped: Stopped by you (Ctrl-C)."


def test_episode_keeps_its_own_tool_origins_apart_from_inherited_ones(tmp_path: Path) -> None:
    """Minor 4 (S3, MP2): origins is the full union; tool_origins only the task's own tool calls (I1 rule), so F5's
    sink rule can tell the task's own reads from inherited context."""
    b = TraceBuilder()
    b.root["origin"] = "external:paste,owner"
    b.source("recalled memory", "a note from an email", "external:email:someone")
    w = b.worker(5, "done", "read")
    b.tool(w, 1, 0.5, "browser.read", {"url": "https://a.com"}, "text", origin="external:web:a.com", kind="read")
    b.tool(w, 2, 0.5, "browser.read", {"url": "https://b.com"}, "text", origin=None, kind="read")
    c = compose.compose(b.finish())
    assert c["tool_origins"] == ["external:unlabeled", "external:web:a.com"]
    assert set(c["origins"]) == {"external:paste", "owner", "external:email:someone", "external:unlabeled",
                                 "external:web:a.com"}
    db = Database(tmp_path / "ferb.db")
    db.migrate()
    episodes.write(db, c | {"made_by": episodes.MADE_BY, "built_at": T0.isoformat()})
    got = episodes.get(db, c["task_id"])
    assert got is not None and got["tool_origins"] == c["tool_origins"] and got["origins"] == sorted(c["origins"])


def _failed_turn(kind: str) -> dict[str, Any]:
    """One trace whose `kind` model span ended with an error, though its recorded text parses as a decision."""
    b = TraceBuilder()
    if kind == "model.turn":
        n = b.planner(3, {"kind": "task", "text": "Deleting it.", "task": {"goal": b.goal}})
    elif kind == "model.dispatch":
        b.worker(5, "failed", "could not delete")
        n = b.dispatch(2, "retry", "try again")
    elif kind == "model.question":
        b.worker(5, "done", "deleted")
        n = b.question(2, {"ask_owner": False, "answer": "the hf one", "question": ""})
    else:
        b.worker(5, "done", "deleted")
        n = b.reconcile(2, "done", "All resources released.")
    n["error"] = "ValidationError: 1 validation error for the decision"
    return b.finish()


@pytest.mark.parametrize("kind", ["model.turn", "model.dispatch", "model.question", "model.resources"])
def test_a_failed_model_turn_reads_as_a_failure_not_a_decision(kind: str) -> None:
    c = compose.compose(_failed_turn(kind))
    step = next(s for s in c["steps"] if s["role"] == "Planner")
    assert step["mark"] == "✗" and step["text"] == "failed: ValidationError: 1 validation error for the decision"
    failed = 2 if kind == "model.dispatch" else 1               # the dispatch case's worker also reported failed
    assert c["problems"] == [f"✗ {failed} step{'s' if failed > 1 else ''} failed"]   # a failed retry is no retry


def test_the_reconciler_is_a_planner_step_naming_what_it_decided() -> None:
    b = TraceBuilder()
    b.worker(5, "done", "Opened the page and saved it.")
    b.checker(3, [True, True], ["saved file exists", "nothing else changed"])
    b.reconcile(4, "failed", "The tab closed during cleanup, so the page is no longer open.")
    c = compose.compose(b.finish(status="failed", verified=False))
    assert [s["role"] for s in c["steps"]] == ["Worker", "Checker", "Planner"]
    assert c["steps"][2] == {"at": c["steps"][2]["at"], "role": "Planner", "seconds": 4.0, "mark": "",
                             "text": "after cleanup: failed — The tab closed during cleanup, so the page is no "
                                     "longer open."}
    assert c["usage"]["model_calls"] == 3


def test_spans_still_open_read_as_not_finished() -> None:
    b = TraceBuilder()
    p = b.planner(3, {"kind": "task", "text": "Deleting it.", "task": {"goal": b.goal}})
    w = b.worker(5, "done", "deleted")
    c_ = b.checker(3, [True, True], ["gone", "only one deletion"])
    trace = b.finish()
    for n in (p, w):
        n["ended_at"], n["output"] = None, None
    b.root["ended_at"] = None
    c = compose.compose(trace)
    assert [(s["role"], s["mark"], s["text"], s["seconds"]) for s in c["steps"][:2]] == [
        ("Planner", "⏹", "did not finish (interrupted)", None),
        ("Worker", "⏹", "did not finish (interrupted)", None)]
    assert c["steps"][2]["mark"] == "✓" and c_["ended_at"]           # an open worker is not "verified in step 3"
    assert c["problems"] == ["⏹ 2 steps did not finish"]
    assert c["ended_at"] is None and c["seconds"] is None            # the last timestamp is a lower bound, not the end


def test_unpriced_usage_is_named_and_never_counted_as_free() -> None:
    b = TraceBuilder()
    w = b.worker(5, "done", "deleted")
    b.usage(w, 1000, 0, 10, None, model="new-model-x")
    u = compose.compose(b.finish())["usage"]
    assert u["unpriced_models"] == ["new-model-x"] and u["cost_usd"] is None
    b2 = TraceBuilder()
    w2 = b2.worker(5, "done", "deleted")
    b2.usage(w2, 1000, 0, 10, 0.25)
    b2.usage(w2, 1000, 0, 10, None, model="new-model-x")
    u2 = compose.compose(b2.finish())["usage"]
    assert u2["cost_usd"] == 0.25 and u2["unpriced_models"] == ["new-model-x"] and u2["usage_records"] == 2


def test_a_task_whose_request_or_context_held_outside_content_is_marked(tmp_path: Path) -> None:
    """S3: the request's origins (root task span) and what the context held (context.source) count, not only tool
    results."""
    b = TraceBuilder()
    b.root["origin"] = "external:paste,owner"
    b.source("recalled memory", "a note from an email", "external:email:someone")
    b.worker(5, "done", "summarized")
    c = compose.compose(b.finish())
    assert {"external:paste", "owner", "external:email:someone"} <= set(c["origins"])
    db = Database(tmp_path / "ferb.db")
    db.migrate()
    ep = c | {"made_by": episodes.MADE_BY, "built_at": T0.isoformat()}
    episodes.write(db, ep)
    episodes.write(db, ep)                                  # idempotent: rebuilding replaces the row
    row = db.conn.execute("SELECT origins, external_read FROM episodes WHERE task_id = ?", (c["task_id"],)).fetchone()
    assert row["external_read"] == 1 and "external:paste" in row["origins"].split(",")
    got = episodes.get(db, c["task_id"])
    assert got is not None and got["external_read"] is True and got["steps"] == c["steps"]
    assert got["verified"] is True and got["usage"] == c["usage"]


def test_a_span_recorded_without_an_origin_counts_as_outside_content(tmp_path: Path) -> None:
    """Residual 1 (S3, MP2): every span from before F3 has no origin, and those from before F0 not even an action
    kind. A tool call or context source that holds output but no origin is unlabeled outside content, whatever its
    kind, by the one rule task.evidence and task_origins use; a span with no output adds nothing."""
    b = TraceBuilder()
    w = b.worker(5, "done", "looked")
    b.tool(w, 1, 0.5, "browser.snapshot", {"tab": "t1"}, "", origin=None, kind=None, output={"tree": "page text"})
    b.tool(w, 2, 0.5, "shell.read", {"id": "p1"}, "", origin=None, kind=None, output={"output": "file text"})
    c = compose.compose(b.finish())
    assert c["origins"] == c["tool_origins"] == ["external:unlabeled"]
    db = Database(tmp_path / "ferb.db")
    db.migrate()
    episodes.write(db, c | {"made_by": episodes.MADE_BY, "built_at": T0.isoformat()})
    got = episodes.get(db, c["task_id"])
    assert got is not None and got["external_read"] is True
    b2 = TraceBuilder()
    b2.tool(b2.worker(5, "done", "clicked"), 1, 0.5, "browser.act", {"element": "e1"}, "", origin=None, kind="click",
            output={"ok": True})
    assert compose.compose(b2.finish())["origins"] == ["external:unlabeled"]       # whatever its kind
    b3 = TraceBuilder()
    b3.source("recalled memory", "a note", None)
    b3.tool(b3.worker(5, "failed", "lost"), 1, 0.5, "probe.send", {"text": "x"}, "", origin=None, kind="send",
            error="ConnectionError: lost the acknowledgement")                   # an error path: no output
    c3 = compose.compose(b3.finish())
    assert c3["origins"] == ["external:unlabeled"] and c3["tool_origins"] == []   # the context source only


def test_a_task_that_read_only_private_data_is_not_marked(tmp_path: Path) -> None:
    c = compose.compose(hf_cache_retry())
    c["usage"]["unpriced_models"] = ["modèle-ü"]
    db = Database(tmp_path / "ferb.db")
    db.migrate()
    episodes.write(db, c | {"made_by": episodes.MADE_BY, "built_at": T0.isoformat()})
    got = episodes.get(db, c["task_id"])
    assert got is not None and got["external_read"] is False and got["origins"] == ["private"]
    raw = db.conn.execute("SELECT usage_json FROM episodes WHERE task_id = ?", (c["task_id"],)).fetchone()[0]
    assert "modèle-ü" in raw                                # stored as written, like the other JSON columns


def test_episode_is_written_when_a_task_finishes(daemon: RunningDaemon) -> None:
    install(daemon, [sup("task", "Doing it.", task("say hi"))], {"worker": done("said hi")})
    chat(daemon, "say hi")
    ep = _episode(daemon, _last_root(daemon))
    assert ep is not None and ep["request_text"] == "say hi" and ep["status"] == "done"
    assert ep["made_by"] == episodes.MADE_BY and json.loads(ep["steps_json"])
    usage = json.loads(ep["usage_json"])
    assert usage["usage_records"] >= 3                      # the intake planner's usage counts too


def test_the_intake_planners_usage_is_in_the_tree(daemon: RunningDaemon) -> None:
    install(daemon, [sup("task", "Doing it.", task("say hi"))], {"worker": done("said hi")})
    chat(daemon, "say hi")
    root = _last_root(daemon)

    async def go():
        tree = daemon.daemon.spans.tree_json(root)
        planner = next(n for n in tree if n["kind"] == "model.turn")
        return [c["kind"] for c in planner["children"]]
    assert "model.usage" in daemon.run(go())


def test_a_stopped_task_gets_an_episode(daemon: RunningDaemon) -> None:
    install(daemon, [sup("task", "Working.", task("long job"))], {"worker": STALL})
    c = Client(daemon.sock, timeout=30)
    try:
        for ev in c.stream("chat.send", {"session_id": "s", "text": "long job"}):
            if ev["type"] == "plan":
                c2 = Client(daemon.sock)
                c2.call("task.stop", {"task_ids": [ev["task_id"]]})
                c2.close()
            if ev["type"] in ("outcome", "reply", "clarify"):
                break
    finally:
        c.close()
    root = _last_root(daemon)
    ep = _episode(daemon, root)

    async def status():
        return daemon.daemon.db.conn.execute("SELECT status FROM tasks WHERE id = ?", (root,)).fetchone()["status"]
    assert ep is not None and ep["status"] == daemon.run(status()) and ep["status"] != "done"
    # The worker span records its own status, so a stopped worker reads as stopped, not as a guess from its error.
    assert any(s["role"] == "Worker" and "stopped: " in s["text"] for s in json.loads(ep["steps_json"]))


def _seed_ghost(home: Path) -> None:
    """A daemon that stops while task 'ghost' runs: its root, worker and checker spans are still open."""
    d = RunningDaemon(home)
    try:
        async def seed():
            now = d.daemon.clock.now().isoformat()
            d.daemon.db.conn.execute(
                "INSERT INTO tasks (id, tree_id, request_id, goal, identity, role, scopes_json, status, created_at, "
                "updated_at) VALUES ('ghost', 'ghost', 'r', 'tidy the desktop', 'owner', 'worker', '[]', 'running', ?, ?)",
                (now, now))
            root = d.daemon.spans.start("task", task_id="ghost", input={"request": "tidy the desktop"})
            d.daemon.spans.start("worker", task_id="ghost.worker", parent_id=root.id,
                                 input={"goal": "tidy the desktop", "role": "worker", "scopes": []})
            d.daemon.spans.start("worker", task_id="ghost.worker.verify", parent_id=root.id,
                                 input={"goal": "verify", "role": "verifier", "scopes": []})
            d.daemon.db.conn.commit()
        d.run(seed())
    finally:
        d.stop()


def test_a_task_interrupted_by_a_restart_gets_an_episode(home: Path) -> None:
    _seed_ghost(home)
    d2 = RunningDaemon(home)
    try:
        ep = _episode(d2, "ghost")
        assert ep is not None and ep["status"] == "interrupted" and ep["request_text"] == "tidy the desktop"
        steps = json.loads(ep["steps_json"])
        assert [(s["role"], s["mark"], s["text"], s["seconds"]) for s in steps] == [
            ("Worker", "⏹", "did not finish (interrupted)", None),
            ("Checker", "⏹", "did not finish (interrupted)", None)]
        assert json.loads(ep["problems_json"]) == ["⏹ 2 steps did not finish"]
        assert ep["ended_at"] is None and ep["seconds"] is None
    finally:
        d2.stop()


def test_a_failing_episode_fallback_never_stops_start_up(home: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    _seed_ghost(home)
    real_append = Journal.append

    def append(self: Journal, kind: str, data: Any = None, origin: Any = None) -> Any:
        if kind == "episode.failed":
            raise sqlite3.OperationalError("disk I/O error")
        return real_append(self, kind, data, origin)
    monkeypatch.setattr(episodes, "build", _broken)
    monkeypatch.setattr(Journal, "append", append)
    d2 = RunningDaemon(home)                                # start-up goes on: no exception here
    try:
        assert _episode(d2, "ghost") is None and Client(d2.sock).call("ping", {}) is not None
    finally:
        d2.stop()


def _broken(*a: Any, **k: Any) -> dict[str, Any]:
    raise RuntimeError("composer broke")


def test_an_episode_that_cannot_be_built_never_fails_the_task(daemon: RunningDaemon,
                                                              monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setattr(episodes, "build", _broken)
    install(daemon, [sup("task", "Doing it.", task("say hi"))], {"worker": done("said hi")})
    evs = chat(daemon, "say hi")
    assert [e for e in evs if e["type"] == "outcome"][-1]["status"] == "done"
    root = _last_root(daemon)

    async def failed():
        rows = daemon.daemon.db.conn.execute("SELECT data_json FROM journal WHERE kind = 'episode.failed'").fetchall()
        return [json.loads(r["data_json"]) for r in rows]
    assert daemon.run(failed()) == [{"task_id": root, "error": "RuntimeError: composer broke"}]
    assert _episode(daemon, root) is None


def test_a_failing_episode_fallback_never_skips_the_tasks_cleanup(daemon: RunningDaemon,
                                                                  monkeypatch: pytest.MonkeyPatch) -> None:
    journal = daemon.daemon.supervisor.journal
    real_append = journal.append

    def append(kind: str, data: Any = None, origin: Any = None) -> Any:
        if kind == "episode.failed":
            raise sqlite3.OperationalError("disk I/O error")
        return real_append(kind, data, origin)
    monkeypatch.setattr(episodes, "build", _broken)
    monkeypatch.setattr(journal, "append", append)
    install(daemon, [sup("task", "Doing it.", task("say hi"))], {"worker": done("said hi")})
    finished: list[dict[str, Any]] = []
    publish = daemon.daemon.supervisor.publish
    monkeypatch.setattr(daemon.daemon.supervisor, "publish",
                        lambda kind, data: (finished.append(data) if kind == "task.finished" else None,
                                            publish(kind, data)))
    evs = chat(daemon, "say hi")
    root = _last_root(daemon)
    assert [e for e in evs if e["type"] == "outcome"][-1]["status"] == "done"
    assert {"task_id": root, "status": "done"} in finished and daemon.daemon.supervisor._tree_of == {}


def _last_root(daemon: RunningDaemon) -> str:
    async def go():
        return daemon.daemon.db.conn.execute(
            "SELECT id FROM tasks WHERE parent_id IS NULL ORDER BY created_at DESC, rowid DESC").fetchone()["id"]
    return daemon.run(go())


def _episode(daemon: RunningDaemon, task_id: str) -> dict | None:
    async def go():
        row = daemon.daemon.db.conn.execute("SELECT * FROM episodes WHERE task_id = ?", (task_id,)).fetchone()
        return dict(row) if row else None
    return daemon.run(go())
