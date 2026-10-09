"""`ferb trace` levels rendered by code from records (P26, RQ1–RQ4); golden outputs pin every line."""
from ferb.trace import compose, render
from tests.support.trace_builder import TraceBuilder, hf_cache_retry


def _ep(trace):
    return compose.compose(trace) | {"made_by": "code:episode/1", "built_at": "2026-10-07T11:07:00+05:30"}


def test_story_of_the_retry_case_is_golden() -> None:
    assert render.story(_ep(hf_cache_retry())) == [
        '#f29fee46 · "delete the hf cache completely" · done ✓ verified · 2m23s · 6 model calls',
        "1. 11:04:37 Planner (12s): task — delete ~/.cache/huggingface",
        "2. 11:04:49 Worker (46s): 2 tool calls · worker reported done: Deleted ~/.cache/huggingface (22 GB); it no longer exists.",
        "3. 11:05:35 Checker (8s): ⚠ checked nothing — 2 conditions unknown (Tool discovery was not attempted)",
        "4. 11:05:43 Planner (9s): retry — verify only, delete nothing more",
        "5. 11:05:52 Worker (37s): worker reported done: Re-verified: the folder is gone and nothing else was removed. — verified in step 6",
        "6. 11:06:29 Checker (31s): ✓ verified: fs.list shows no huggingface folder; ledger shows one deletion only",
        "Answer: Deleted ~/.cache/huggingface.",
        "⚠ 1 retry (1m17s spent after it) · ⚠ 1 check left conditions unknown",
        "Tokens: 700k in (83% cached) · 7.0k out · ≈ $0.84 API-equivalent (estimate; your subscription covers it)",
        "Desktop: 36 changes during the task, none near a FERB on-screen action",
    ]


def test_unknown_price_says_so() -> None:
    b = TraceBuilder()
    w = b.worker(5, "done", "ok")
    b.usage(w, 1000, 0, 10, None, billing="api", model="new-model-x")
    lines = render.story(_ep(b.finish()))
    assert "Tokens: 1.0k in (0% cached) · 10 out · cost unknown for new-model-x" in lines
    assert not any("$" in ln for ln in lines)


def test_missing_usage_records_are_disclosed() -> None:
    b = TraceBuilder()
    w = b.worker(5, "done", "ok")
    b.usage(w, 1000, 500, 10, 0.01, billing="api")
    b.planner(2, {"kind": "reply", "text": "hi"})
    lines = render.story(_ep(b.finish()))
    assert any(ln.endswith("· usage recorded for 1 of 2 model calls") for ln in lines)


def test_desktop_changes_near_screen_actions_get_detail_lines() -> None:
    b = TraceBuilder()
    b.worker(5, "done", "clicked")
    b.desktop(3, near=[("mac.act", ["frontmost_app", "cursor"])])
    lines = render.story(_ep(b.finish()))
    assert "Desktop: 3 changes during the task, 1 near a FERB on-screen action" in lines
    assert "  · 11:04:42 cursor, frontmost_app near mac.act" in lines


def test_a_front_app_change_no_ferb_action_was_near_says_so_with_ferbs_running_processes() -> None:
    b = TraceBuilder()
    b.worker(5, "done", "submitted")
    b.came_forward("Google Chrome", "com.google.Chrome",
                   actions=[{"tool": "shell.wait", "span_id": "w1", "on_screen": False}],
                   processes=[{"id": "p-1", "task": "f29fee46.worker",
                               "command": "codex -m gpt-6-astra --dangerously-bypass-approvals-and-sandbox"}])
    b.desktop(889)
    lines = render.story(_ep(b.finish()))
    assert ("Desktop: 889 changes during the task, none near a FERB on-screen action; 1 front-app or Space change "
            "with no FERB on-screen action in progress") in lines
    assert ("  · 11:04:42 Google Chrome came to the front — no FERB on-screen action in progress (none yet); "
            "running: codex -m gpt-6-astra --dangerously-bypass-approvals-and-san… (started by this task)") in lines


def test_another_tasks_call_and_a_pause_are_said_plainly() -> None:
    b = TraceBuilder()
    b.worker(5, "done", "waited")
    b.came_forward("Safari", "com.apple.Safari", processes=[],
                   actions=[{"tool": "mac.act", "span_id": "x1", "task_id": "f29fee46.worker", "on_screen": True},
                            {"tool": "browser.open", "span_id": "x2", "task_id": "other.worker", "on_screen": True}])
    b.paused(30)
    b.desktop(1)
    lines = render.story(_ep(b.finish()))
    assert lines[-3:] == ["Desktop: 1 changes during the task, 1 near a FERB on-screen action",
                          "  · 11:04:42 frontmost_app near mac.act, browser.open (another FERB task's call)",
                          "  · 11:04:42 desktop not observed (paused) until 11:05:12"]


def test_failed_status_and_verdict() -> None:
    b = TraceBuilder()
    b.worker(5, "failed", "couldn't open it")
    ep = _ep(b.finish(status="failed", verified=False, answer="I couldn't open it."))
    lines = render.story(ep, {"verdict": "fail", "note": "wrong file", "at": "2026-10-07T12:00:00+05:30"})
    assert " · failed ✗ · " in lines[0]
    assert "✗ 1 step failed" in lines
    assert lines[-1] == "Your verdict: fail (07 Oct 12:00) — wrong file"


def test_steps_view_has_one_line_per_tool_call() -> None:
    assert render.steps(hf_cache_retry()) == [
        "11:04:50  shell.run  command=du -sh ~/.cache/huggingface  (why: check size first)  → exit 0 · 1 line  [0.4s]",
        "11:04:52  shell.run  command=rm -rf ~/.cache/huggingface  (why: delete the exact path)  → exit 0 · 0 lines  [2.1s]",
    ]


def test_an_unfinished_story_shows_unknown_durations_and_does_not_crash() -> None:
    b = TraceBuilder()
    w = b.worker(5, "done", "deleted")
    trace = b.finish()
    w["ended_at"], w["output"] = None, None
    b.root["ended_at"] = None
    lines = render.story(_ep(trace))
    assert lines[0].count("?s") == 1 and " · ?s · " in lines[0]
    assert any("Worker (?s): ⏹ did not finish (interrupted)" in ln for ln in lines)
    assert "⏹ 1 step did not finish" in lines
