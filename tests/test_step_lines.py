"""Tools get a one-line summary next to their output; the same step line feeds ferb logs and ferb trace --steps (P26)."""
import json

from ferb.cli.logs import format_event
from ferb.trace import lines
from tests.conftest import FakeCap, RunningDaemon, gateway_request, gateway_setup, gateway_tool_call


def test_summaries_for_common_tools() -> None:
    s = lines.summarize
    assert s("fs.list", {}, {"path": "/x", "entries": [{}, {}, {}], "total": 3}, False) == "3 entries"
    assert s("fs.list", {}, {"path": "/x", "entries": [{}, {}, {}], "total": 10}, False) == "3 of 10 entries"
    assert s("fs.read", {}, {"size": 5, "text": "abcde", "offset": 0, "next_offset": None}, False) == "read 5 bytes"
    assert s("fs.read", {}, {"size": 10, "text": "abc", "offset": 0, "next_offset": 3}, False) == \
        "read 3 of 10 bytes from offset 0"
    assert s("fs.write", {}, {"path": "/x/y.txt", "size": 12, "bytes": 12}, False) == "wrote /x/y.txt (12 bytes)"
    assert s("fs.edit", {}, {"path": "/x/y.txt", "replaced": 1}, False) == "edited /x/y.txt"
    assert s("fs.trash", {}, {"path": "/x/y.txt", "trashed_to": "/T/y.txt"}, False) == "moved to Trash: /x/y.txt"
    assert s("shell.run", {}, {"exit_code": 0, "running": False, "output": "a\nb\nc"}, False) == "exit 0 · 3 lines"
    assert s("shell.run", {}, {"exit_code": 1, "running": False, "output": "oops"}, True) == "exit 1 · 1 line"
    assert s("shell.run", {}, {"id": "p1", "running": True, "exit_code": None, "output": ""}, False) == \
        "still running (p1)"
    assert s("owner.ask", {}, {"answer": "the one at work"}, False) == 'you answered: "the one at work"'
    assert s("owner.ask", {}, {"answer": "work Alex", "answered_by": "orchestrator"}, False) == \
        'the orchestrator answered: "work Alex"'
    assert s("owner.ask", {}, {"declined": True}, False) == "no answer"


def test_errors_and_unknown_tools_get_a_generic_summary() -> None:
    assert lines.summarize("fs.read", {}, {"error": "no such file: /nope"}, True) == "error: no such file: /nope"
    assert lines.summarize("some.tool", {}, {"a": 1, "b": [1, 2]}, False) == "a, b"
    assert lines.summarize("some.tool", {}, {}, False) == "done"
    assert len(lines.summarize("some.tool", {}, {"error": "x" * 500}, True)) <= 160


def test_step_line_shows_time_tool_args_why_summary_and_duration() -> None:
    line = lines.step_line("2026-10-08T11:04:49+05:30", "shell.run", {"command": "du -sh ~/.cache/huggingface"},
                           "check size first", "exit 0 · 1 line", 0.4, None)
    assert line == "11:04:49  shell.run  command=du -sh ~/.cache/huggingface  (why: check size first)  → exit 0 · 1 line  [0.4s]"
    err = lines.step_line("2026-10-08T11:04:49+05:30", "fs.read", {"path": "/nope"}, "", "error: no such file", 0.0,
                          "no such file")
    assert err == "11:04:49  fs.read  path=/nope  ✗ no such file  [0.0s]"
    assert lines.duration(143) == "2m23s" and lines.duration(8) == "8s" and lines.duration(None) == "?s"


def test_gateway_stores_summaries_and_logs_show_step_lines(daemon: RunningDaemon) -> None:
    cap = FakeCap("probe.read", "read", {"probe"}, reads=(("file", "/x"),), content={"entries": [1, 2]})
    grant, _ = gateway_setup(daemon, {"probe"}, gateway_request("look"), [cap])
    gateway_tool_call(daemon, grant, "probe.read", text="hello")

    async def go():
        return daemon.daemon.db.conn.execute("SELECT summary FROM spans WHERE kind = 'tool.call'").fetchone()["summary"]
    assert daemon.run(go()) == "entries"
    ev = {"at": "2026-10-08T11:04:49+05:30", "class": "telemetry", "topic": "tool.finished",
          "data": {"tool": "fs.list", "task_id": "t", "summary": "3 entries", "seconds": 0.1, "error": None,
                   "args": {"path": "/x"}, "why": ""}}
    assert format_event(ev) == "11:04:49  fs.list  path=/x  → 3 entries  [0.1s]"


def test_error_paths_get_a_summary_and_still_record_no_origin(daemon: RunningDaemon) -> None:
    cap = FakeCap("probe.send", "send", {"probe"}, boom=True)
    grant, _ = gateway_setup(daemon, {"probe"}, gateway_request("look"), [cap])
    seen: list[dict] = []
    real = daemon.daemon.gateway.publish
    daemon.daemon.gateway.publish = lambda topic, data: (seen.append({"topic": topic, **data}), real(topic, data))[1]
    out = gateway_tool_call(daemon, grant, "probe.send", text="my token sk-ant-api03-abcdefghijklmnopqrstuvwx")
    assert out["is_error"] and "Origin:" not in out["footer"]

    async def go():
        return dict(daemon.daemon.db.conn.execute(
            "SELECT summary, origin, output_json FROM spans WHERE kind = 'tool.call'").fetchone())
    row = daemon.run(go())
    assert row["summary"].startswith("error: ConnectionError") and row["origin"] is None and row["output_json"] is None
    done = [e for e in seen if e["topic"] == "tool.finished"]
    assert len(done) == 1 and done[0]["tool"] == "probe.send" and done[0]["error"].startswith("ConnectionError")
    assert "abcdefghijklmnopqrstuvwx" not in json.dumps(done[0])


def test_a_secret_straddling_the_summary_cut_never_leaks(daemon: RunningDaemon) -> None:
    secret = "sk-ant-api03-ABCDEFGHIJKLMNOPQRSTUVWX"

    class Leaky(FakeCap):
        async def run(self, args, ctx):
            raise ConnectionError("x" * 124 + " " + secret)    # "error: ConnectionError: " + 125 chars: it starts near 149
    grant, _ = gateway_setup(daemon, {"probe"}, gateway_request("look"), [Leaky("probe.send", "send", {"probe"})])
    seen: list[dict] = []
    real = daemon.daemon.gateway.publish
    daemon.daemon.gateway.publish = lambda topic, data: (seen.append({"topic": topic, **data}), real(topic, data))[1]
    gateway_tool_call(daemon, grant, "probe.send")

    async def go():
        return daemon.daemon.db.conn.execute("SELECT summary FROM spans WHERE kind = 'tool.call'").fetchone()[0]
    summary = daemon.run(go())
    event = next(e for e in seen if e["topic"] == "tool.finished")
    for text in (summary, json.dumps(event)):
        assert "sk-ant" not in text and "ABCDEFGH" not in text
