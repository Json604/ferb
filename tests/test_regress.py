from pathlib import Path
from typing import Any

from ferb.eval.regress import RegressTask, load_set, run_set


def test_load_set_parses_tasks(tmp_path: Path) -> None:
    p = tmp_path / "s.toml"
    p.write_text('[[task]]\nid="t1"\nprompt="say hi"\nsurface="disposable"\nexpect="a greeting"\n')
    tasks = load_set(p)
    assert len(tasks) == 1 and tasks[0].id == "t1" and tasks[0].surface == "disposable"
    assert tasks[0].expect == "a greeting"


def test_smoke_set_loads() -> None:
    import ferb
    from ferb.eval.regress import DEFAULT_SET
    assert DEFAULT_SET.is_file() and Path(ferb.__file__).parent in DEFAULT_SET.parents
    tasks = load_set(DEFAULT_SET)
    assert len(tasks) == 2 and all(t.surface == "disposable" and t.expect for t in tasks)


class _FakeClient:
    def __init__(self, scripts: dict[str, list[dict[str, Any]]] | None = None) -> None:
        self.scripts = scripts or {}
        self.calls: list[tuple[str, dict[str, Any]]] = []
        self.sent: list[dict[str, Any]] = []
        self.answer_fails = False
        self.closed = False

    def call(self, method: str, params: dict[str, Any] | None = None) -> dict[str, Any]:
        self.calls.append((method, params or {}))
        if method == "chat.answer" and self.answer_fails:
            raise RuntimeError("approval expired")
        if method == "chat.attach":
            return {"ok": True, "session_id": "s1"}
        return {"ok": True, "state": "declined"}

    def stream(self, method: str, params: dict[str, Any]) -> Any:
        self.sent.append(params)
        key = params["text"]
        if key == "boom":
            raise RuntimeError("submit failed")
        if key in self.scripts:
            yield from self.scripts[key]
            return
        yield {"type": "plan", "task_id": "x-" + key}
        yield {"type": "outcome", "task_id": "x-" + key, "status": "done", "summary": "ok:" + key}


    def close(self) -> None:
        self.closed = True


def _t(i: str, prompt: str) -> RegressTask:
    return RegressTask(i, prompt, "disposable", "")


def test_run_set_records_a_row_per_task_and_survives_errors() -> None:
    c = _FakeClient()
    rows = run_set(c, [_t("a", "hi"), _t("b", "boom"), _t("c", "yo")], out=None, record=lambda r: None,
                   connect=lambda: c)
    assert [r["status"] for r in rows] == ["done", "errored", "done"]
    assert rows[0]["task_id"] == "x-hi" and rows[1]["task_id"] is None
    assert rows[0]["asked"] == []
    assert [c_[0] for c_ in c.calls].count("chat.attach") == 1
    assert {p["session_id"] for p in c.sent} == {"s1"}


def test_run_set_declines_owner_question_and_records_it() -> None:
    c = _FakeClient({"q": [
        {"type": "plan", "task_id": "t-q"},
        {"type": "event", "topic": "approval.requested", "data": {"approval_id": "ap1", "reason": "Send it?"}},
        {"type": "outcome", "task_id": "t-q", "status": "failed", "summary": "declined"},
    ]})
    rows = run_set(c, [_t("q", "q")], out=None, record=lambda r: None)
    assert ("chat.answer", {"approval_id": "ap1", "text": ""}) in c.calls
    assert rows[0]["asked"] == ["Send it?"] and rows[0]["status"] == "failed"


def test_run_set_reply_clarify_and_error_events() -> None:
    c = _FakeClient({
        "r": [{"type": "reply", "text": "hello"}],
        "k": [{"type": "clarify", "text": "which one?"}],
        "e": [{"type": "error", "text": "bad"}],
    })
    rows = run_set(c, [_t("r", "r"), _t("k", "k"), _t("e", "e")], out=None, record=lambda r: None)
    assert [(r["status"], r["outcome_summary"], r["task_id"]) for r in rows] == [
        ("reply", "hello", None), ("clarify", "which one?", None), ("errored", "bad", None)]


def test_run_set_calls_record_with_rows() -> None:
    got: list[Any] = []
    rows = run_set(_FakeClient(), [_t("a", "hi")], out=None, record=got.append)
    assert got == [rows]


def test_cli_regress_prints_verdict_lines_and_writes_rows(tmp_path: Path) -> None:
    import io
    import json

    from ferb.cli.admin import regress
    p = tmp_path / "s.toml"
    p.write_text('[[task]]\nid="t1"\nprompt="hi"\nexpect="a greeting"\n')
    out = io.StringIO()
    assert regress(_FakeClient(), lambda: _FakeClient(), p, tmp_path, out) == 0
    text = out.getvalue()
    assert "ferb verdict x-hi pass|fail" in text and "a greeting" in text
    saved = list((tmp_path / "regression").glob("*.json"))
    assert len(saved) == 1 and ":" not in saved[0].name and json.loads(saved[0].read_text())[0]["task_id"] == "x-hi"


def test_failed_decline_is_recorded_and_stream_is_drained() -> None:
    c = _FakeClient({"q": [
        {"type": "plan", "task_id": "t-q"},
        {"type": "event", "topic": "approval.requested", "data": {"approval_id": "ap1", "reason": "Send?"}},
        {"type": "outcome", "task_id": "t-q", "status": "done", "summary": "fine"},
    ]})
    c.answer_fails = True
    rows = run_set(c, [_t("q", "q"), _t("n", "next")], out=None, record=lambda r: None)
    assert rows[0]["status"] == "done" and rows[0]["asked"][0] == "Send?"
    assert rows[0]["asked"][1].startswith("decline failed: RuntimeError")
    assert rows[1]["task_id"] == "x-next" and rows[1]["status"] == "done"


def test_broken_stream_without_factory_marks_remaining_tasks_errored() -> None:
    rows = run_set(_FakeClient(), [_t("b", "boom"), _t("n", "next")], out=None, record=lambda r: None)
    assert [r["status"] for r in rows] == ["errored", "errored"]
    assert "not run" in rows[1]["outcome_summary"] and rows[1]["task_id"] is None


def test_broken_stream_with_factory_uses_a_fresh_client() -> None:
    old, fresh = _FakeClient(), _FakeClient()
    rows = run_set(old, [_t("b", "boom"), _t("n", "next")], out=None, record=lambda r: None, connect=lambda: fresh)
    assert old.closed and rows[1]["task_id"] == "x-next" and fresh.sent[0]["session_id"] == "s1"


def test_cli_missing_set_is_a_clean_error(tmp_path: Path, monkeypatch: Any, capsys: Any) -> None:
    from ferb import cli
    monkeypatch.setattr(cli, "Client", lambda *a, **k: _FakeClient())
    assert cli.main(["regress", str(tmp_path / "nope.toml")]) != 0
    assert f"ferb: no such regression set: {tmp_path / 'nope.toml'}" in capsys.readouterr().err
