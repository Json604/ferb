"""Build step 13, offline: Jev in shadow mode against a fake Jev endpoint.
- Jev never approves: in shadow mode its answers are only logged next to FERB's decision; enabled, only a stricter
  answer acts (block, ask, exposed, redaction, dropped, stopped, a suggestion); it's never consulted for an action
  FERB already blocked, and a lower risk never lowers anything.
- Enabling: 200 answered samples at >= 95% agreement where FERB has real logic; elsewhere the owner decides.
- Trail data reaches Jev only after exclusions and redaction; durable trail batches have receipts and do not sample away events.
- Jev down / slow / rate-limited / wrong-shaped / without a key: FERB's own decision stands and nothing fails.
- One request per state (batched), model pinned to jev-1.13.0; the key never lands in config, the DB or logs.
"""

import asyncio
import json
import time
from typing import Any

import pytest
from pydantic import BaseModel

from ferb.core.types import ActionSpec, Identity, RiskLevel, Target, ToolContext, ToolResult
from ferb.decide.decider import Ask, JevClient, JevUnavailable
from ferb.decide.questions import QUESTIONS
from ferb.sense.trail import Observation
from tests.conftest import (FakeDistillRuntime, RunningDaemon, gateway_request as request,
                            gateway_setup as setup, trail_row)

KEY = "jev-test-key-7f3a9c"


class FakeJev:
    """Answers each question by its id (matched from the instructions); records every request."""

    def __init__(self) -> None:
        self.answers: dict[str, Any] = {}
        self.requests: list[dict[str, Any]] = []
        self.status: list[int] = []          # statuses to return before 200 (e.g. [429, 529])
        self.delay = 0.0
        self.fail: Exception | None = None
        self.garbage = False

    async def post(self, url: str, headers: dict[str, str], body: dict[str, Any], timeout: float) -> tuple[int, Any]:
        self.requests.append({"url": url, "headers": headers, "body": body})
        if self.delay:
            await asyncio.sleep(self.delay)
        if self.fail:
            raise self.fail
        if self.status:
            return self.status.pop(0), None
        if self.garbage:
            return 200, {"answers": {n: {"type": "choice", "choice": "banana"} for n in body["questions"]}}
        out = {}
        for n, q in body["questions"].items():
            qid = next(k for k, v in QUESTIONS.items() if v.instructions == q["instructions"])
            a = self.answers.get(qid)
            if a is None:
                continue
            out[n] = {"type": "noul", "noul": a} if q["type"] == "noul" else \
                {"type": "choice", "choice": a, "probabilities": {a: 0.9}, "confidence": 0.9}
        return 200, {"model": body["model"], "answers": out, "usage": {"input_tokens": 10, "output_tokens": 1}}


@pytest.fixture
def jd(daemon: RunningDaemon) -> Any:
    fake = FakeJev()
    daemon.daemon.config.set("jev", "on", True)
    daemon.daemon.jev_post = fake.post
    daemon.run(daemon.daemon._build_jev())
    daemon.daemon.jev.rng = lambda: 0.0                                    # always sampled, unless a test says so
    daemon.daemon.jev.enabled_timeout_s = 0.5
    return daemon, fake


def enable(d: RunningDaemon, *qs: str) -> None:
    d.daemon.config.set("jev", "enabled", list(qs))


class Shell:
    name, description, scopes = "fake.shell", "fake shell", {"shell"}
    identities, risk, reversible = {Identity.OWNER}, RiskLevel.MEDIUM, False

    class Input(BaseModel):
        cmd: str = "ls"

    def __init__(self, reads: tuple[tuple[str, str], ...] = (), content: dict[str, Any] | None = None) -> None:
        self.calls, self.reads, self.content = 0, reads, content or {"ok": True}

    def describe(self, args: BaseModel) -> ActionSpec:
        return ActionSpec("run_shell", Target(), content=getattr(args, "cmd"))

    async def run(self, args: BaseModel, ctx: ToolContext) -> ToolResult:
        self.calls += 1
        return ToolResult(self.content, read_sources=self.reads)


def shell_call(d: RunningDaemon, cap: Shell, cmd: str = "ls") -> dict[str, Any]:
    grant = getattr(d, "_shell_grant", None)
    if grant is None:                                                       # one task per test daemon
        grant, _ = setup(d, {"shell"}, request("run the build"), [])
        d._shell_grant = grant                                             # type: ignore[attr-defined]
    d.daemon.registry.capabilities[cap.name] = cap
    return d.run(d.daemon.gateway.call(grant, cap.name, {"cmd": cmd, "effect": "edit"}), timeout=20)  # type: ignore[no-any-return]


def rows(d: RunningDaemon, q: str | None = None) -> list[dict[str, Any]]:
    async def go() -> list[dict[str, Any]]:
        sql, args = "SELECT * FROM decisions", ()
        if q:
            sql, args = sql + " WHERE question = ?", (q,)
        return [dict(r) for r in d.daemon.db.conn.execute(sql + " ORDER BY seq", args)]  # type: ignore[union-attr]
    return d.run(go())  # type: ignore[no-any-return]


def settle(d: RunningDaemon) -> None:
    d.run(d.daemon.jev.idle(), timeout=10)


# ---------- never approves; shadow only logs ----------


# ---------- enabling ----------

def test_attention_remains_shadow_and_sensitive_classifier_is_retired():
    assert "trail.sensitive" not in QUESTIONS
    assert {"memory.worth_keeping", "trail.relevance", "trail.attention", "notification.attention"} <= set(QUESTIONS)
    assert QUESTIONS["notification.attention"].shadow_only


def test_trail_reaches_jev_only_after_exclusions_redaction_and_batching(jd: Any) -> None:
    d, fake = jd
    async def rec(o: Observation) -> Any:
        return d.daemon.trail.record(o)
    d.run(rec(Observation("app_focus", {"app": "Passwords", "bundle_id": "com.apple.Passwords"})))
    d.run(rec(Observation("window", {"app": "Terminal", "bundle_id": "com.apple.Terminal",
                                     "title": "export T=sk-ant-abcdefghijklmnopqrstuvwxyz0123"})))
    assert not fake.requests
    d.run(d.daemon.perception.batches.flush("test"))
    sent = json.dumps([r["body"]["state"] for r in fake.requests])
    assert "Passwords" not in sent and "sk-ant-abcdefghijklmnop" not in sent and "Terminal" in sent
    fake.requests.clear()
    d.run(d.daemon.perception.batches.flush("test"))
    assert not fake.requests


def test_notification_shadow_cannot_be_enabled_through_config(jd: Any) -> None:
    d, fake = jd
    enable(d, "notification.attention")
    fake.answers = {"notification.attention": 0.95}
    assert d.run(d.daemon.jev.ask({"notification": "self message"}, [Ask("notification.attention", "no", "test")])) == {}
    settle(d)
    row = rows(d, "notification.attention")[-1]
    assert row["jev"] == "yes" and row["mode"] == "shadow" and row["effect"] is None


# ---------- Jev unavailable: FERB's own decision, nothing fails ----------

@pytest.mark.parametrize("trouble", ["down", "slow", "garbage", "http500", "no_key"])
def test_unavailable_jev_never_fails_a_task(jd: Any, trouble: str) -> None:
    d, fake = jd
    enable(d, "memory.worth_keeping")
    if trouble == "down":
        fake.fail = ConnectionError("refused")
    elif trouble == "slow":
        fake.delay = 3.0
    elif trouble == "garbage":
        fake.garbage = True
    elif trouble == "http500":
        fake.status = [500]
    else:
        async def none() -> None:
            return None
        d.daemon.jev.client.key = none
    t0 = time.monotonic()
    assert d.run(d.daemon.jev.ask({'title': 'a normal page'}, [Ask('memory.worth_keeping', 'yes', 'test')])) == {}
    assert time.monotonic() - t0 < 2.5
    assert rows(d) and all(x['error'] and x['jev'] is None for x in rows(d))


def test_rate_limits_retry_with_backoff() -> None:
    fake, waits = FakeJev(), []
    fake.answers = {"memory.worth_keeping": 0.1}
    fake.status = [429, 529]

    async def key() -> str:
        return KEY

    async def sleep(s: float) -> None:
        waits.append(s)
    c = JevClient(key, post=fake.post, sleep=sleep)
    q = QUESTIONS["memory.worth_keeping"].api()
    out = asyncio.run(c.ask({"x": 1}, {"q0": q}))
    assert out["answers"]["q0"]["noul"] == 0.1 and waits == [0.5, 1.0]
    assert fake.requests[0]["headers"]["Authorization"] == f"Bearer {KEY}"
    fake.status = [429] * 10
    with pytest.raises(JevUnavailable):
        asyncio.run(c.ask({"x": 1}, {"q0": q}))
    with pytest.raises(ValueError, match="pin"):
        JevClient(key, model="jev-latest")


def test_shadow_calls_never_slow_a_task(jd: Any) -> None:
    d, fake = jd
    fake.delay = 2.0
    t0 = time.monotonic()
    r = shell_call(d, Shell())
    assert not r["is_error"] and time.monotonic() - t0 < 0.5


def test_the_key_stays_out_of_config_database_and_logs(jd: Any) -> None:
    d, fake = jd
    d.daemon.jev.client.key = _const(KEY)
    fake.answers = {"memory.worth_keeping": 0.1}
    d.run(d.daemon.jev.ask({"title": "normal page"}, [Ask("memory.worth_keeping", "yes", "test")]))
    settle(d)

    async def ckpt() -> None:
        d.daemon.db.conn.execute("PRAGMA wal_checkpoint(FULL)")           # type: ignore[union-attr]
    d.run(ckpt())
    for p in d.daemon.home.rglob("*"):
        if p.is_file() and not p.is_socket():
            assert KEY.encode() not in p.read_bytes(), p


def _const(v: str) -> Any:
    async def f() -> str:
        return v
    return f


# ---------- the other questions ----------

def test_memory_proposals_jev_rejects_are_dropped_when_enabled(jd: Any) -> None:
    d, fake = jd
    enable(d, "memory.worth_keeping")
    fake.answers = {"memory.worth_keeping": 0.1}
    day = "2026-09-10"
    trail_row(d, day)
    ev_handle = "E1"
    rt = FakeDistillRuntime({"narrative": "n", "items": [
        {"kind": "preference", "subject": "owner", "key": "github", "content": "uses GitHub", "origin": "observed",
         "confidence": 0.6, "handles": [ev_handle], "quote": None, "retire": False}]})
    d.daemon.distiller.make_runtime = lambda *a, **k: rt
    r = d.run(d.daemon.distiller.run_day(day), timeout=20)
    assert r["proposed"] == 0 and "not worth keeping" in r["refused"][0]


def test_jev_guard_counts_questions_and_does_not_reintroduce_character_cap(jd: Any, monkeypatch: pytest.MonkeyPatch) -> None:
    import ferb.decide.decider as module
    d, _ = jd
    measured = []
    def count(text):
        measured.append(text)
        return 1000
    monkeypatch.setattr(module, 'count_tokens', count)
    state = {'text': 'x' * 50000}
    assert d.daemon.jev._state(state, {'q': {'instructions': 'required rules'}}) == state
    assert 'required rules' in measured[-1]
    monkeypatch.setattr(module, 'count_tokens', lambda text: 32000)
    with pytest.raises(JevUnavailable, match='no partial state'):
        d.daemon.jev._state(state)


@pytest.mark.parametrize("status", [401, 402, 403])
def test_a_refused_key_is_not_asked_again_until_it_changes(status: int) -> None:
    """Live 2026-10-25 12:39 to 2026-10-27: Jev answered HTTP 402 (the owner's free credits ran out) to 10,366 asks, about
    four a minute, each sending a redacted activity batch. A key refused for payment or authorization isn't asked again
    by this client: `ferb setup --jev` (a new key) or a restart builds a new one. Owner 2026-10-27: perception stays
    paused; a Jev-free approach comes later."""
    fake = FakeJev()
    fake.answers = {"memory.worth_keeping": 0.1}
    fake.status = [status]

    async def key() -> str:
        return KEY
    c = JevClient(key, post=fake.post)
    q = QUESTIONS["memory.worth_keeping"].api()
    with pytest.raises(JevUnavailable, match=f"HTTP {status}"):
        asyncio.run(c.ask({"x": 1}, {"q0": q}))
    with pytest.raises(JevUnavailable, match="not asked again"):
        asyncio.run(c.ask({"x": 1}, {"q0": q}))
    assert len(fake.requests) == 1                                  # the second ask never left the Mac


# ---------- the one switch (notes/memory.md decision 4) ----------

def test_jev_is_off_by_default_and_nothing_runs(daemon: RunningDaemon) -> None:
    """Switched off (the default): even with a fake endpoint ready, no decider is built, the batcher sends nothing,
    no decisions or receipts are written and the destructive shadow check asks nothing."""
    fake = FakeJev()
    d = daemon.daemon
    d.jev_post = fake.post
    daemon.run(d._build_jev())
    assert d.jev is None and d._jev_active() is None

    async def rec(o: Observation) -> Any:
        return d.trail.record(o)
    daemon.run(rec(Observation("window", {"app": "Terminal", "bundle_id": "com.apple.Terminal", "title": "x"})))
    daemon.run(d.perception.batches.flush("test"))
    daemon.run(d._destructive_shadow({"kind": "pay", "task_id": "t"}, "span:1"))
    assert not fake.requests
    assert _count(daemon, "decisions") == _count(daemon, "perception_receipts") == 0
    assert _count(daemon, "perception_batches") == 0


def _count(d: RunningDaemon, table: str) -> int:
    async def go() -> int:
        return int(d.daemon.db.conn.execute(f"SELECT COUNT(*) FROM {table}").fetchone()[0])  # type: ignore[union-attr]
    return d.run(go())  # type: ignore[no-any-return]


def test_a_refused_key_stops_the_batcher_and_shadow_checks(jd: Any) -> None:
    """Live 2026-10-25..27: the batcher kept sending (and writing failed decisions) after Jev refused the key, because
    it checked only that a decider existed."""
    d, fake = jd
    d.daemon.jev.client.refused = "HTTP 402"
    assert d.daemon._jev_active() is None

    async def rec(o: Observation) -> Any:
        return d.daemon.trail.record(o)
    d.run(rec(Observation("window", {"app": "Terminal", "bundle_id": "com.apple.Terminal", "title": "y"})))
    d.run(d.daemon.perception.batches.flush("test"))
    d.run(d.daemon._destructive_shadow({"kind": "pay", "task_id": "t"}, "span:2"))
    assert not fake.requests
    assert _count(d, "decisions") == 0
