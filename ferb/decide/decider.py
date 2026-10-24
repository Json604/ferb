"""Owns optional Jev typed-decision transport, shadowing and recorded fallback. Must not expand enrichment into task authorization or silently replace the baseline.

Jev behind FERB's own decisions.

- Callers provide an observation or memory proposal and the baseline they would otherwise keep.
- `JevClient` calls Jev (POST /v1/systemone, model pinned to jev-1.13.0: calibration differs between versions), with
  the key from the Keychain; questions about the same state go in one request; 429 / 529 retry with backoff.
- `ShadowDecider` asks both. A question in shadow mode never waits: a sample of its calls goes to a background queue
  and Jev's answer is only logged next to the baseline. A question the owner enabled waits at most `enabled_timeout_s`,
  and only a stricter answer (questions.py) comes back to the caller. A live question (Q4: `trail.attention`) is
  recorded as 'live', and its caller marks what the answer did (`mark_effect`). Jev down, slow, wrong-shaped, or
  without a key: the baseline stands, the reason is logged, and nothing fails.

What reaches Jev is built by callers from data that already passed exclusions and redaction, and is redacted once
more here. Nothing sent is stored; decisions keep a reference to what was decided."""

import asyncio
import json
import random
import time
from dataclasses import dataclass
from typing import Any, Callable

from ferb.core.interfaces import Clock, IdSource
from ferb.core.context import count_tokens, token_method
from ferb.decide.questions import QUESTIONS, Question, base_id
from ferb.store.db import Database

JEV_URL = "https://api.typesafe.ai/v1/systemone"
JEV_KEYS_URL = "https://console.typesafe.ai/keys"     # where anyone gets a key (Typesafe's docs, quickstart)
JEV_MODEL = "jev-1.13.0"
KEY_ACCOUNT = "jev/api_key"                      # Keychain: service "ferb", this account
JEV_CONTEXT_TOKENS = 32000             # D10/P1 in docs/0.27_PLAN.md; includes state, questions and framing


class JevUnavailable(Exception):
    pass


@dataclass
class Ask:
    key: str                                     # question id, or "id#n" for one of several in a batch
    baseline: str
    ref: str | None = None
    criteria: dict[str, str] | None = None       # per-call typed choice options

    @property
    def question(self) -> Question:
        return QUESTIONS[base_id(self.key)]


class JevClient:
    def __init__(self, key: Callable[[], Any], post: Callable[..., Any] | None = None, url: str = JEV_URL,
                 model: str = JEV_MODEL, retries: int = 4, backoff_s: float = 0.5,
                 sleep: Callable[[float], Any] = asyncio.sleep) -> None:
        if model == "jev-latest":
            raise ValueError("pin a Jev version: calibration differs between versions")
        self.key, self.url, self.model, self.retries, self.backoff_s, self.sleep = key, url, model, retries, backoff_s, sleep
        self._post = post

    async def _http(self, headers: dict[str, str], body: dict[str, Any], timeout: float) -> tuple[int, Any]:
        if self._post is not None:
            return await self._post(self.url, headers, body, timeout)   # type: ignore[no-any-return]
        import httpx
        async with httpx.AsyncClient(timeout=timeout) as c:
            r = await c.post(self.url, headers=headers, json=body)
            try:
                return r.status_code, r.json()
            except ValueError:
                return r.status_code, None

    async def ask(self, state: Any, questions: dict[str, dict[str, Any]], timeout: float = 20.0) -> dict[str, Any]:
        key = await self.key()
        if not key:
            raise JevUnavailable("no Jev API key in the Keychain (`ferb setup --jev`)")
        headers = {"Authorization": f"Bearer {key}", "Content-Type": "application/json"}
        body = {"model": self.model, "state": state, "questions": questions}
        delay = self.backoff_s
        for attempt in range(self.retries + 1):
            try:
                status, data = await self._http(headers, body, timeout)
            except Exception as e:                                   # network, TLS, timeout: unavailable
                raise JevUnavailable(f"{type(e).__name__}") from None
            if status in (429, 529) and attempt < self.retries:
                await self.sleep(delay)
                delay *= 2
                continue
            if status != 200 or not isinstance(data, dict) or not isinstance(data.get("answers"), dict):
                raise JevUnavailable(f"HTTP {status}")
            return data
        raise JevUnavailable("rate limited")


def _mode(a: Ask, on: set[str]) -> str:
    """How a decision is recorded: 'live' for a question the plan of record made live (Q4), 'enabled' for one the
    owner switched on, else 'shadow'."""
    return "live" if a.question.live else "enabled" if base_id(a.key) in on else "shadow"


@dataclass
class _Job:
    state: Any
    asks: list[Ask]
    mode: str = "shadow"


@dataclass
class ShadowDecider:
    db: Database
    clock: Clock
    ids: IdSource
    client: JevClient | None
    redact: Callable[[Any], Any]
    enabled: Callable[[], set[str]] = set                           # question ids the owner switched on
    rng: Callable[[], float] = random.random
    enabled_timeout_s: float = 2.0
    queue_max: int = 200
    dropped: int = 0
    _queue: asyncio.Queue[_Job] | None = None
    _worker: asyncio.Task[None] | None = None

    async def ask(self, state: Any, asks: list[Ask]) -> dict[str, str]:
        """Record every ask; return {key: Jev's answer} only for enabled questions where Jev is stricter."""
        on = {name for name in self.enabled() if name in QUESTIONS and not QUESTIONS[name].shadow_only}
        live = [a for a in asks if base_id(a.key) in on]
        shadow = [a for a in asks if base_id(a.key) not in on]
        sampled = [a for a in shadow if self.rng() < a.question.shadow_sample]
        for a in shadow:
            if a not in sampled:
                self._record(a, None, None, None, "not sampled", "shadow")
        if not live:
            if sampled:
                self._enqueue(_Job(state, sampled))
            return {}
        answers = await self._call(state, live + sampled, self.enabled_timeout_s, on)
        out = {}
        for a in live:
            v = answers.get(a.key)
            if v is not None and a.question.is_stricter(v, a.baseline):
                out[a.key] = v
        return out

    async def advice(self, state: Any, asks: list[Ask]) -> dict[str, str]:
        """Recorded non-enforcing advice; only the orchestrator may interpret it."""
        answers = await self._call(state, asks, self.enabled_timeout_s, set())
        return answers

    async def answers(self, state: Any, asks: list[Ask]) -> dict[str, str]:
        """A durable caller (perception batches) awaits its recorded decisions. Each is recorded in its question's
        mode: a shadow question's answer enables nothing downstream; a live question's caller acts on it and marks the
        effect (`mark_effect`)."""
        return await self._call(state, asks, 30.0, set())

    def mark_effect(self, key: str, ref: str | None, effect: str) -> None:
        self.db.conn.execute(
            "UPDATE decisions SET effect = ? WHERE seq = (SELECT MAX(seq) FROM decisions WHERE question = ? AND "
            "(ref IS ? OR ref = ?))", (effect, base_id(key), ref, ref))

    # ---------- calling Jev ----------

    async def _call(self, state: Any, asks: list[Ask], timeout: float, on: set[str]) -> dict[str, str]:
        names = {f"q{i}": a for i, a in enumerate(asks)}
        questions = {n: (a.question.api() | ({"criteria": a.criteria} if a.criteria else {})) for n, a in names.items()}
        started = time.monotonic()
        try:
            wire = self._state(state, questions)
            if self.client is None:
                raise JevUnavailable("Jev isn't set up")
            data = await asyncio.wait_for(self.client.ask(wire, questions, timeout), timeout)
        except (JevUnavailable, asyncio.TimeoutError) as e:
            why = "timeout" if isinstance(e, asyncio.TimeoutError) else str(e)
            for a in asks:
                self._record(a, None, None, None, why, _mode(a, on))
            return {}
        ms = int((time.monotonic() - started) * 1000)
        out = {}
        for n, a in names.items():
            ans = data["answers"].get(n)
            mode = _mode(a, on)
            try:
                if not isinstance(ans, dict):
                    raise ValueError("no answer")
                if a.criteria and ans.get("choice") not in a.criteria:
                    raise ValueError("unknown choice")
                v, conf = (str(ans["choice"]), ans.get("confidence")) if a.criteria else a.question.value(ans)
            except (KeyError, ValueError, TypeError) as e:
                self._record(a, None, None, ms, f"bad answer: {e}", mode)
                continue
            out[a.key] = v
            self._record(a, v, conf, ms, None, mode)
        return out

    def _state(self, state: Any, questions: dict[str, Any] | None = None) -> Any:
        s = self.redact(state)                                    # once more, whatever the caller did
        text = json.dumps({"state": s, "questions": questions or {}}, ensure_ascii=False, default=str)
        # Keep the documented context ceiling; the normal 10k event budget supplies ample framing headroom.
        # This is explicitly an estimate of the provider's tokenizer, never an exact Jev token claim.
        if count_tokens(text) >= JEV_CONTEXT_TOKENS:
            raise JevUnavailable(f"request reaches Jev's {JEV_CONTEXT_TOKENS} token context ({token_method()}); no partial state was sent")
        return s

    def _record(self, a: Ask, jev: str | None, conf: Any, ms: int | None, error: str | None, mode: str) -> None:
        self.db.conn.execute(
            "INSERT INTO decisions (id, at, question, ref, baseline, jev, confidence, latency_ms, error, mode) "
            "VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            (self.ids.new_id(), self.clock.now().isoformat(), base_id(a.key), a.ref, a.baseline, jev,
             float(conf) if isinstance(conf, (int, float)) else None, ms, error, mode))

    # ---------- the shadow queue: never in the way ----------

    def _enqueue(self, job: _Job) -> None:
        if self._queue is None:
            self._queue = asyncio.Queue()
            self._worker = asyncio.get_running_loop().create_task(self._drain())
        while self._queue.qsize() >= self.queue_max:
            old = self._queue.get_nowait()
            self.dropped += 1
            for a in old.asks:
                self._record(a, None, None, None, "dropped (shadow queue full)", "shadow")
        self._queue.put_nowait(job)

    async def _drain(self) -> None:
        assert self._queue is not None
        while True:
            job = await self._queue.get()
            try:
                await self._call(job.state, job.asks, 30.0, set())
            except Exception as e:                                # never takes anything down
                for a in job.asks:
                    self._record(a, None, None, None, f"{type(e).__name__}", "shadow")
            finally:
                self._queue.task_done()

    async def idle(self) -> None:
        """Tests: wait until the shadow queue is empty."""
        if self._queue is not None:
            await self._queue.join()

    def close(self) -> None:
        if self._worker is not None:
            self._worker.cancel()

    # ---------- observation statistics ----------

    def stats(self) -> list[dict[str, Any]]:
        out = []
        for q in QUESTIONS.values():
            r = self.db.conn.execute(
                "SELECT COUNT(*) AS asked, SUM(jev IS NOT NULL) AS answered, "
                "SUM(jev IS NOT NULL AND jev != baseline) AS differ, SUM(effect IS NOT NULL) AS acted, "
                "AVG(latency_ms) AS ms FROM decisions WHERE question = ? AND (error IS NULL OR error != 'not sampled')",
                (q.id,)).fetchone()
            answered = int(r["answered"] or 0)
            out.append({"question": q.id, "asked": int(r["asked"] or 0), "answered": answered,
                        "flag_rate": (int(r["differ"] or 0) / answered) if answered else None,
                        "acted": int(r["acted"] or 0), "avg_ms": round(r["ms"]) if r["ms"] else None,
                        "enabled": q.id in self.enabled() and not q.shadow_only, "effect": q.effect,
                        "mode": "live" if q.live else "enabled" if q.id in self.enabled() and not q.shadow_only
                        else "shadow"})
        return out

    def flagged_refs(self, question: str, n: int = 5) -> list[str]:
        return [r["ref"] for r in self.db.conn.execute(
            "SELECT ref FROM decisions WHERE question = ? AND jev IS NOT NULL AND jev != baseline AND ref IS NOT NULL "
            "ORDER BY seq DESC LIMIT ?", (question, n))]
