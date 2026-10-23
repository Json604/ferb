"""P1/D10 (perf plan 2026-10-23, Task 3): the perception batcher decides exactly as the module did before the cost work
(tests/support/batches_reference.py, verbatim), and a wake's work follows the new events, not the trail or the unsent
set.

- Seeded streams drive the reference and the current batcher side by side, each on its own database with the same fake
  clock times, ids and fake Jev: every flush reason, batch, Jev request, publish and receipt matches, through sizes
  around the budget, every source, notification bursts, interrupts, excerpts, Jev unavailable then back, rows deleted
  or rewritten, and restarts, some with a batch in flight.
- Work per wake (trail rows read, events encoded, characters counted) is the same with 200 or 3000 unsent events.
- The exact counts this relies on: a growing text's slices (GrowingCount), and the o200k piece boundary after ", "
  that the notification trimming's lower bound (uncut_count) is built on."""
import asyncio
import json
import random
import re
import zlib
from itertools import accumulate
from pathlib import Path
from types import SimpleNamespace
from typing import Any, Callable

import pytest

import ferb.sense.batches as current
import tests.support.batches_reference as reference
from ferb.clock.clock import FakeClock
from ferb.core import context
from ferb.core.context import TOKEN_SLICE, GrowingCount, comma_bounds, count_tokens, uncut_count
from ferb.core.ids import SequentialIds
from ferb.sanitize.redact import Redactor
from ferb.sense.exclusions import Exclusions
from ferb.sense.notifications import NotificationPolicy
from ferb.sense.trail import Observation, Trail
from ferb.store.db import Database

tokenizer = pytest.mark.skipif(context._encoding() is None, reason="the o200k tokenizer isn't cached here")

WORDS = ["def", "self", "return", "pending", "Code", "FERB", "x", "1234", "56", "é", "naïve", "→", "日本語", "😀", "it's",
         "don'T", "a,", ",", ", ", " ,", "/", "//", "\\", '"', "'", "{", "}]", "[", "...", "?!", "\t", "  ", " ",
         " ", "́", "Ünïcode", "HTTP", "kartikey.xyz/path?q=1", "-", "+", "::", "<|endoftext|>"]
APPS = [("com.microsoft.VSCode", "Code"), ("com.google.Chrome", "Google Chrome"), ("com.apple.Notes", "Notes"),
        ("net.whatsapp.WhatsApp", "WhatsApp")]


def words(rng: random.Random, n: int) -> str:
    return "".join(rng.choice(WORDS) + rng.choice(["", " ", " ", ", ", "\n"]) for _ in range(n))


# ---------- the exact counts ----------

def test_count_tokens_adds_up_its_slices() -> None:
    rng = random.Random(1)
    text = words(rng, 9000)
    assert len(text) > 3 * TOKEN_SLICE
    assert count_tokens(text) == sum(count_tokens(text[i:i + TOKEN_SLICE]) for i in range(0, len(text), TOKEN_SLICE))


def test_a_growing_count_is_count_tokens_of_the_text_so_far() -> None:
    rng = random.Random(2)
    for count in (count_tokens, len):
        size, text = GrowingCount(count), ""
        for _ in range(150):
            more = words(rng, rng.choice([0, 1, 3, 40, 900, 2500]))
            suffix = words(rng, rng.choice([0, 1, 30]))
            assert size.with_suffix(suffix) == count(text + suffix)
            size.append(more)
            text += more
        assert size.with_suffix() == count(text) and len(text) > 5 * TOKEN_SLICE


def test_the_proven_pattern_is_the_one_counted_with() -> None:
    """uncut_count's proof (ferb/core/context.py) is about exactly this pattern: change it, redo the proof."""
    assert context._PATTERN == "|".join([
        r"[^\r\n\p{L}\p{N}]?[\p{Lu}\p{Lt}\p{Lm}\p{Lo}\p{M}]*[\p{Ll}\p{Lm}\p{Lo}\p{M}]+(?i:'s|'t|'re|'ve|'m|'ll|'d)?",
        r"[^\r\n\p{L}\p{N}]?[\p{Lu}\p{Lt}\p{Lm}\p{Lo}\p{M}]+[\p{Ll}\p{Lm}\p{Lo}\p{M}]*(?i:'s|'t|'re|'ve|'m|'ll|'d)?",
        r"\p{N}{1,3}", r" ?[^\s\p{L}\p{N}]+[\r\n/]*", r"\s*[\r\n]+", r"\s+(?!\S)", r"\s+"])


@tokenizer
def test_o200k_never_merges_across_a_comma_and_its_space() -> None:
    """Cut right after any ", " comma, the parts count alone to the whole (adversarial text around the cuts)."""
    rng = random.Random(3)
    tight = ["'", "s", "S", "1", "2", " ", "  ", "\n", "\r\n", ",", ", ", ",,", "/", "\\", '"', "}", "]", "{", "é",
             "́", " ", " ", "　", "😀", "日", "'ll", "x", "<|endoftext|>", "!", "\t"]
    for _ in range(20000):
        text = "".join(rng.choice(tight) for _ in range(rng.randint(1, 120)))
        bounds = comma_bounds(text)
        assert all(text[b - 1] == "," and text[b] == " " for b in bounds)
        cuts = sorted(rng.sample(bounds, rng.randint(0, len(bounds)))) if bounds else []
        parts = [text[a:b] for a, b in zip([0] + cuts, cuts + [len(text)])]
        assert count_tokens(text) == sum(count_tokens(p) for p in parts), (text, cuts)


@tokenizer
def test_the_lower_bound_counts_only_uncut_atoms_and_never_exceeds_the_count() -> None:
    rng = random.Random(4)
    for _ in range(60):
        units = [words(rng, rng.choice([1, 5, 30, 300, 2500])) for _ in range(rng.randint(3, 40))]
        text = "[" + ", ".join(units) + "]"
        ends = list(accumulate((len(u) + 2 for u in units), initial=0))   # atom i = " " + unit + "," (shifted)
        own = [count_tokens(" " + u + ",") if len(u) + 2 <= TOKEN_SLICE else 0 for u in units]
        totals = list(accumulate(own, initial=0))
        lo, hi = 1, len(units) - 1                                         # the first and last aren't delimited
        bound = uncut_count(ends, totals, lo, hi, 0)
        uncut = sum(own[i] for i in range(lo, hi)
                    if ends[i] // TOKEN_SLICE == (ends[i + 1] - 1) // TOKEN_SLICE)
        assert bound == uncut <= count_tokens(text)


# ---------- batch_events and the notification trimming ----------

def _event(rng: random.Random, i: int) -> dict[str, Any]:
    bundle, app = rng.choice(APPS)
    size = rng.choice([1, 5, 20, 60, 400] + [2000] * (rng.random() < 0.2) + [12000] * (rng.random() < 0.02))
    data: dict[str, Any] = {"bundle_id": bundle, "app": app, "window_id": str(rng.randint(1, 3)), "value": words(rng, size)}
    if rng.random() < 0.3:
        data = {"browser": rng.choice(["chrome", "brave", "safari"]), "tab_id": str(rng.randint(1, 4)),
                "title": words(rng, size)}
    return {"id": f"e{i}", "at": "2026-10-23T10:00:00+05:30", "source": rng.choice(["accessibility", "tab", "window"]),
            "actor": "unknown", "data": data}


def test_batch_events_matches_the_reference_at_every_budget() -> None:
    rng = random.Random(5)
    for trial in range(25):
        events = [_event(rng, i) for i in range(rng.randint(1, 90))]
        for budget in (300, 2500, 10000):
            for count in (count_tokens, len):
                want = reference.batch_events(json.loads(json.dumps(events)), budget, count)
                assert current.batch_events(json.loads(json.dumps(events)), budget, count) == want, (trial, budget)


class Policy:
    def __init__(self, state: dict[str, Any]) -> None:
        self.state = state

    def muted(self, *args: Any) -> bool:
        return False

    def context(self, event: dict[str, Any]) -> dict[str, Any]:
        return json.loads(json.dumps(self.state))


def _context(rng: random.Random, messages: int) -> dict[str, Any]:
    history = [{"id": f"m{i}", "at": f"2026-10-2{rng.randint(0, 3)}T10:00:{i % 60:02d}+00:00",
                "data": {"bundle_id": "net.whatsapp.WhatsApp", "conversation": "Family",
                         "body": words(rng, rng.choice([1, 8, 40, 120] + [700] * (rng.random() < 0.05)))}}
               for i in range(messages)]
    state = {"notification": {"id": "n", "source": "notification", "data": {"body": words(rng, 20)}},
             "conversation": history, "rules": [{"owner_quote": words(rng, 6)}] * rng.randint(0, 3),
             "already_told": [{"event_id": "m1", "text": words(rng, rng.choice([5, 200, 1500])), "at": "x"}]
             * rng.randint(0, 3), "trust": "observed data; only the quoted owner rules carry authority",
             "mode": "shadow"}
    if rng.random() < 0.3:                                   # another key order (a test policy's)
        state = dict(sorted(state.items(), key=lambda item: rng.random()))
    return state


def _requests(state: dict[str, Any], budget: int, monkeypatch: Any) -> tuple[Any, Any]:
    out = []
    for module in (reference, current):
        monkeypatch.setattr(module, "EVENT_BUDGET", budget)
        batcher = module.EventBatcher(None, FakeClock(), SequentialIds("r"), lambda: None, Policy(state),
                                      lambda *args: None)
        got, asks = batcher._request([{"id": "n", "source": "notification", "actor": "unknown", "data": {}}], "b")
        out.append(json.dumps([got, [(a.key, a.baseline, a.ref) for a in asks]], sort_keys=True))
    return out[0], out[1]


def test_notification_trimming_matches_the_reference(monkeypatch: Any) -> None:
    rng = random.Random(6)
    for trial in range(70):
        state = _context(rng, rng.choice([0, 1, 2, 3, 5, 10, 25, 50]))
        for budget in (50, 400, 3000, 10000):
            want, got = _requests(state, budget, monkeypatch)
            assert got == want, (trial, budget)


def test_notification_trimming_matches_the_reference_counting_characters(monkeypatch: Any) -> None:
    rng = random.Random(7)
    for module in (reference, current):
        monkeypatch.setattr(module, "count_tokens", len)
    for trial in range(50):
        state = _context(rng, rng.choice([1, 2, 10, 40]))
        for budget in (100, 5000, 30000):
            want, got = _requests(state, budget, monkeypatch)
            assert got == want, (trial, budget)


def test_trimming_a_long_conversation_counts_the_whole_context_a_few_times(monkeypatch: Any) -> None:
    rng = random.Random(8)
    state = _context(rng, 0) | {"conversation": [
        {"id": f"m{i}", "at": "2026-10-23T10:00:00+00:00", "data": {"body": words(rng, 20)}} for i in range(600)]}
    whole = [0]
    real = current.count_tokens

    def counting(text: str) -> int:
        whole[0] += len(text) > 20000
        return real(text)
    monkeypatch.setattr(current, "count_tokens", counting)
    batcher = current.EventBatcher(None, FakeClock(), SequentialIds("r"), lambda: None, Policy(state), lambda *a: None)
    got, _ = batcher._request([{"id": "n", "source": "notification", "actor": "unknown", "data": {}}], "b")
    assert got["omitted_oldest_messages"]["count"] > 300
    assert whole[0] <= 25                                 # the reference counts it whole once per message left out
    monkeypatch.setattr(current, "count_tokens", real)
    assert _requests(state, current.EVENT_BUDGET, monkeypatch)[0] == _requests(state, current.EVENT_BUDGET,
                                                                                 monkeypatch)[1]


def _left_out(state: dict[str, Any], k: int) -> dict[str, Any]:
    """The context after the reference loop left out k messages."""
    out, omitted = json.loads(json.dumps(state)), []
    for _ in range(k):
        omitted.append("trail:" + out["conversation"].pop(0)["id"])
        out[current.OMITTED] = {"count": len(omitted), "source_refs": list(omitted)}
    return out


def test_the_trimming_bound_never_exceeds_the_count_and_its_layout_is_the_text() -> None:
    rng = random.Random(9)
    gaps = []
    for trial in range(40):
        state = _context(rng, rng.choice([2, 3, 6, 20, 60]))
        if trial % 8 == 0:                                         # many slices: short messages, long history
            state["conversation"] = [{"id": f"s{i}", "at": "x", "data": {"body": words(rng, rng.randint(0, 12))}}
                                     for i in range(220)]
        layout = current._Layout.of(state, current._text(state))
        assert layout is not None
        texts = [current._text(item) for item in state["conversation"]]
        for k in range(1, len(texts)):
            layout.leave_out()
            text = current._text(_left_out(state, k))
            assert text == (layout.head + ", ".join(texts[k:]) + layout.mid + ", " + json.dumps(current.OMITTED)
                            + ': {"count": ' + str(k) + ', "source_refs": [' + ", ".join(
                                json.dumps(ref) for ref in layout.refs) + "]}}")
            for ends, totals, lo, hi, shift in layout.runs():        # each atom counted is the text between two
                for i in range(lo, hi):                              # ", " cuts, and its count is its own
                    a, b = shift + ends[i], shift + ends[i + 1]
                    assert text[a - 1:a + 1] == ", " and text[b - 1:b + 1] == ", ", (trial, k, i)
                    assert totals[i + 1] - totals[i] == (count_tokens(text[a:b]) if b - a <= TOKEN_SLICE else 0)
            bound, whole = layout.bound(), count_tokens(text)
            assert 0 <= bound <= whole, (trial, k)
            gaps.append(whole - bound)
    assert sorted(gaps)[len(gaps) // 2] < 400                      # tight enough to skip most counts


def test_the_trimming_loop_matches_the_reference_with_the_tightest_bound(monkeypatch: Any) -> None:
    """With the exact count as its bound (sound, and as tight as can be), the loop still leaves out what the reference
    does, at budgets on the boundary."""
    def exact(self: Any) -> int:
        return count_tokens(current._text(_left_out(self.state, len(self.refs))))
    real = current._Layout.of

    def of(state: dict[str, Any], whole: str) -> Any:
        layout = real(state, whole)
        if layout is not None:
            layout.state = json.loads(json.dumps(state))
        return layout
    monkeypatch.setattr(current._Layout, "of", staticmethod(of))
    monkeypatch.setattr(current._Layout, "bound", exact)
    rng = random.Random(12)
    for trial in range(20):
        state = _context(rng, rng.choice([3, 8, 20]))
        counts = [count_tokens(current._text(_left_out(state, k))) for k in range(len(state["conversation"]))]
        for k in rng.sample(range(len(counts)), min(4, len(counts))):
            for budget in (counts[k] - 1, counts[k], counts[k] + 1):
                want, got = _requests(state, budget, monkeypatch)
                assert got == want, (trial, k, budget)


def test_notification_trimming_matches_the_reference_at_budgets_on_the_boundary(monkeypatch: Any) -> None:
    rng = random.Random(10)
    for trial in range(25):
        state = _context(rng, rng.choice([3, 8, 30]))
        counts = [count_tokens(current._text(_left_out(state, k))) for k in range(len(state["conversation"]))]
        for k in rng.sample(range(len(counts)), min(4, len(counts))):
            for budget in (counts[k] - 1, counts[k], counts[k] + 1):
                want, got = _requests(state, budget, monkeypatch)
                assert got == want, (trial, k, budget)


# ---------- the loop, side by side with the reference ----------

class Jev:
    """Answers from the request itself, so two worlds with the same ids answer alike; may hang once on a request (a
    batch in flight when the process stops)."""

    def __init__(self, world: "World") -> None:
        self.world = world

    async def answers(self, state: Any, asks: Any) -> dict[str, str]:
        text = json.dumps([state, [(a.key, a.baseline, a.ref) for a in asks]], sort_keys=True, default=str)
        self.world.log.append(("jev", text))
        h = zlib.crc32(text.encode())
        if h % 41 == 0 and text not in self.world.hung:
            self.world.hung.add(text)
            self.world.hanging = True
            await asyncio.Event().wait()
        if h % 11 == 0:
            raise RuntimeError("Jev failed")
        if h % 7 == 0:
            return {}
        return {asks[0].key: "yes" if h % 3 == 0 else "no"}

    def mark_effect(self, key: str, ref: str | None, effect: str) -> None:
        self.world.log.append(("effect", key, ref, effect))


class World:
    """One database, trail and batcher (reference or current), its loop running."""

    def __init__(self, path: Path, module: Any, age: float) -> None:
        self.db = Database(path)
        self.db.migrate()
        self.clock, self.ids, self.module, self.age = FakeClock(), SequentialIds("w"), module, age
        self.trail = Trail(self.db, self.clock, self.ids, Redactor(), Exclusions, lambda *a: None, lambda: "dev",
                           lambda: False)
        self.policy = NotificationPolicy(self.db, self.clock, self.ids, Redactor())
        self.log: list[Any] = []
        self.hung: set[str] = set()
        self.on, self.hanging, self.waiting = True, False, False
        self.tick = asyncio.Event()
        self.jev = Jev(self)
        self.start()

    def start(self) -> None:
        self.batcher = self.module.EventBatcher(
            self.db, self.clock, self.ids, lambda: self.jev if self.on else None, self.policy,
            lambda topic, data: self.log.append((topic, json.dumps(data, sort_keys=True, default=str))),
            oldest_seconds=lambda: self.age)
        self.trail.tag = self.batcher.notify
        if self.module is current:
            real = self.batcher._sleep

            async def sleep(stop: asyncio.Event, deadline: Any) -> None:
                self.waiting = True
                try:
                    await real(stop, deadline)
                finally:
                    self.waiting = False
            self.batcher._sleep = sleep
        self.stop = asyncio.Event()
        self.task = asyncio.create_task(self.batcher.run(self.stop))

    def evaluate(self) -> None:
        """The next moment the loop looks again: the reference's 1 s timer, the current loop's next wake."""
        (self.tick if self.module is reference else self.batcher.wake).set()

    async def restart(self, cancel: bool, again: bool = True) -> None:
        if cancel:
            self.task.cancel()
        else:
            self.stop.set()
            self.tick.set()
        try:
            await self.task
        except asyncio.CancelledError:
            pass
        self.tick.clear()
        self.hanging, self.waiting = False, False
        if again:
            self.start()

    def tables(self) -> Any:
        return ([tuple(r) for r in self.db.conn.execute("SELECT * FROM perception_batches ORDER BY rowid")],
                [tuple(r) for r in self.db.conn.execute("SELECT * FROM perception_receipts ORDER BY event_id")])


def _reference_timer(world: World) -> Any:
    """The reference loop's `asyncio`, its 1 s wait_for timing out when the harness ticks instead of on a wall clock."""
    async def wait_for(waiting: Any, timeout: float) -> None:
        assert timeout == 1.0
        tick, woke = asyncio.ensure_future(world.tick.wait()), asyncio.ensure_future(waiting)
        world.waiting = True
        try:
            done, _ = await asyncio.wait({tick, woke}, return_when=asyncio.FIRST_COMPLETED)
        finally:
            world.waiting = False
            tick.cancel()
            woke.cancel()
        if tick in done:
            world.tick.clear()
        if woke not in done:
            raise asyncio.TimeoutError
    return SimpleNamespace(Event=asyncio.Event, Lock=asyncio.Lock, wait_for=wait_for, TimeoutError=asyncio.TimeoutError,
                           CancelledError=asyncio.CancelledError)


async def _settle(worlds: list[World]) -> None:
    calm = 0
    for _ in range(50000):
        await asyncio.sleep(0)
        for w in worlds:
            if w.task.done() and not w.task.cancelled() and w.task.exception() is not None:
                raise w.task.exception()                                    # type: ignore[misc]
        quiet = all((w.waiting or w.hanging) and not w.batcher.wake.is_set() and not w.tick.is_set() for w in worlds)
        calm = calm + 1 if quiet else 0
        if calm >= 30:
            return
    raise AssertionError(("the loops never settled", [(w.module.__name__, w.waiting, w.hanging, w.batcher.wake.is_set(),
                                                         w.tick.is_set(), w.task.done()) for w in worlds]))


SOURCES = ["accessibility"] * 6 + ["window", "app", "clipboard", "typed_text", "tab", "file", "idle", "notification",
                                   "incoming_call"]


def _observation(rng: random.Random, n: int, burst: str | None, big: float, quiet: bool) -> Observation:
    """One observation of any source; `big`: how often one is over the budget alone (an excerpt), `quiet`: interrupts
    rarely come between bursts, so the budget and age decide."""
    bundle, app = rng.choice(APPS)
    size = rng.choice([0, 2, 6, 20, 60, 150] + [600] * (rng.random() < 0.1) + [11000] * (rng.random() < big))
    if burst and quiet:
        size = rng.choice([150, 600, 1500])
    source = "notification" if burst else rng.choice(SOURCES[:-2] if quiet and rng.random() < 0.95 else SOURCES)
    text = words(rng, size)
    if source == "notification":
        return Observation("notification", {"external_id": f"x{n}", "bundle_id": burst or bundle,
                                            "conversation": rng.choice(["Family", "self"]), "title": app, "body": text},
                           {"actor": rng.choice(["unknown"] * 5 + ["ferb"])})
    if source == "tab":
        return Observation("tab", {"browser": rng.choice(["chrome", "brave", "safari"]), "tab_id": str(rng.randint(1, 3)),
                                   "url": "https://kartikey.xyz/" + str(n), "title": text})
    if source in ("file", "idle"):
        return Observation(source, {"path": "/tmp/ferb-test/" + str(n), "seconds": n % 7})
    return Observation(source, {"bundle_id": bundle, "app": app, "pid": 7, "window_id": str(rng.randint(1, 3)),
                                "notification": rng.choice(["AXValueChanged", "AXFocusedUIElementChanged",
                                                            "AXUIElementDestroyed"]),
                                "role": "AXTextArea", "title": text}, {"actor": "unknown"})


def _stream(seed: int, steps: int, budget: int, age: float, tmp_path: Path, monkeypatch: Any, big: float = 0.01,
            quiet: bool = False) -> None:
    for module in (reference, current):
        monkeypatch.setattr(module, "EVENT_BUDGET", budget)

    async def go() -> None:
        old, new = World(tmp_path / f"old{seed}.db", reference, age), World(tmp_path / f"new{seed}.db", current, age)
        monkeypatch.setattr(reference, "asyncio", _reference_timer(old))
        worlds, rng, n = [old, new], random.Random(seed), 0
        if rng.random() < 0.5:
            for w in worlds:
                w.policy.add("net.whatsapp.WhatsApp", "self", None, "mute this chat", "please mute this chat")
        for step in range(steps):
            op = rng.choices(["record", "burst", "advance", "jev", "restart", "delete", "rewrite"],
                             [50, 4, 30, 3, 3, 3, 3])[0]
            if op in ("record", "burst"):
                burst = rng.choice([a for a, _ in APPS]) if op == "burst" else None
                for _ in range(rng.choice([1, 1, 2, 5, 20] + [60] * quiet) if op == "record" else rng.randint(3, 12)):
                    seed_n, n = n, n + 1
                    for w in worlds:
                        w.trail.record(_observation(random.Random(seed * 100003 + seed_n), seed_n, burst, big, quiet))
            elif op == "advance":
                seconds = rng.choice([0.2, 1.0, 5.0, 20.0, age - 0.1, age, age + 1, 3 * age])
                for w in worlds:
                    w.clock.advance(seconds)
                    w.evaluate() if w.module is reference else None
            elif op == "jev":
                for w in worlds:
                    w.on = not w.on
                    w.batcher.wake.set()                                    # the daemon's _wire_jev
            elif op == "restart":
                for w in worlds:
                    await w.restart(cancel=False)
            else:
                ids = [r[0] for r in old.db.conn.execute("SELECT id FROM trail ORDER BY seq DESC LIMIT 30")]
                if ids:
                    victim, kept = rng.choice(ids), json.dumps({"content_expired": True, "kept": words(rng, 3)})
                    for w in worlds:
                        if op == "delete":
                            w.db.conn.execute("DELETE FROM trail WHERE id=?", (victim,))
                        else:
                            w.db.conn.execute("UPDATE trail SET data_json=? WHERE id=?", (kept, victim))
                        w.evaluate()
            await _settle(worlds)
            while old.hanging or new.hanging:                               # a batch in flight when it stops
                assert old.hanging == new.hanging, (seed, step)
                for w in worlds:
                    await w.restart(cancel=True)
                await _settle(worlds)
            assert new.log == old.log, (seed, step, op)
            assert new.tables() == old.tables(), (seed, step, op)
        for w in worlds:
            await w.restart(cancel=True, again=False)
        assert sum(entry[0] == "perception.batch" for entry in old.log) > 3
        for w in worlds:
            w.db.close()
    asyncio.run(go())


@pytest.mark.parametrize("seed", range(6))
def test_the_loop_decides_as_the_reference_on_seeded_streams(seed: int, tmp_path: Path, monkeypatch: Any) -> None:
    _stream(seed, steps=160, budget=[300, 900, 2500][seed % 3], age=60.0, tmp_path=tmp_path, monkeypatch=monkeypatch)


@pytest.mark.parametrize("seed", (101, 102))
def test_the_loop_decides_as_the_reference_at_the_real_budget(seed: int, tmp_path: Path, monkeypatch: Any) -> None:
    _stream(seed, steps=200, budget=10000, age=30.0, tmp_path=tmp_path, monkeypatch=monkeypatch, big=0.03, quiet=True)


# ---------- the cost of a wake ----------

class Counted:
    """The batcher's database connection, counting what statements on the trail hand back."""

    def __init__(self, conn: Any) -> None:
        self.conn, self.rows = conn, 0

    def __getattr__(self, name: str) -> Any:
        return getattr(self.conn, name)

    def execute(self, sql: str, params: Any = ()) -> Any:
        cursor = self.conn.execute(sql, params)
        return _Rows(cursor, self) if re.search(r"FROM trail\b", sql) else cursor


class _Rows:
    def __init__(self, cursor: Any, counted: Counted) -> None:
        self.cursor, self.counted = cursor, counted

    def __iter__(self) -> Any:
        for row in self.cursor:
            self.counted.rows += 1
            yield row

    def fetchone(self) -> Any:
        row = self.cursor.fetchone()
        self.counted.rows += row is not None
        return row

    def fetchall(self) -> list[Any]:
        rows = self.cursor.fetchall()
        self.counted.rows += len(rows)
        return rows


def _store(tmp_path: Path, unsent: int, history: int = 12000) -> Any:
    db = Database(tmp_path / f"cost{unsent}.db")
    db.migrate()
    clock = FakeClock()
    at = clock.now().isoformat()
    db.conn.execute("INSERT INTO perception_batches VALUES('old',?,'age','done','[]',NULL)", (at,))
    rows = [(f"t{i}", at, "2026-01-01", "dev", "unknown", "accessibility",
             json.dumps({"bundle_id": "com.microsoft.VSCode", "window_id": "1", "title": f"line {i} of batches.py"}))
            for i in range(history + unsent)]
    db.conn.executemany("INSERT INTO trail (id, at, day, device_id, actor, source, data_json) VALUES (?,?,?,?,?,?,?)",
                        rows)
    db.conn.executemany("INSERT INTO perception_receipts VALUES(?, 'old')", [(f"t{i}",) for i in range(history)])
    return db, clock


def _wake_costs(tmp_path: Path, unsent: int, monkeypatch: Any) -> list[tuple[int, int, int]]:
    """(trail rows read, events encoded, characters counted) for: a wake with nothing new, one with one new event."""
    db, clock = _store(tmp_path, unsent)
    monkeypatch.setattr(current, "EVENT_BUDGET", 10 ** 9)          # never a flush: only the decision's cost
    work = {"encoded": 0, "counted": 0}
    real_text, real_count = current._text, current.count_tokens

    def text(value: Any) -> str:
        work["encoded"] += isinstance(value, dict) and "source" in value
        return real_text(value)

    def count(text: str) -> int:
        work["counted"] += len(text)
        return real_count(text)
    monkeypatch.setattr(current, "_text", text)
    monkeypatch.setattr(current, "count_tokens", count)
    trail = Trail(db, clock, SequentialIds("c"), Redactor(), Exclusions, lambda *a: None, lambda: "dev", lambda: False)
    conn = Counted(db.conn)
    db.conn = conn
    calls: list[tuple[int, int, int]] = []

    async def go() -> None:
        batcher = current.EventBatcher(db, clock, SequentialIds("b"), lambda: object(),
                                       NotificationPolicy(db, clock, SequentialIds("n"), Redactor()), lambda *a: None,
                                       oldest_seconds=lambda: 10.0 ** 6)
        trail.tag = batcher.notify
        stop = asyncio.Event()
        task = asyncio.create_task(batcher.run(stop))
        for _ in range(200):
            await asyncio.sleep(0)
        for wake in (lambda: batcher.wake.set(), lambda: trail.record(Observation(
                "accessibility", {"bundle_id": "com.microsoft.VSCode", "window_id": "1", "title": "one more line"},
                {"actor": "unknown"}))):
            conn.rows, work["encoded"], work["counted"] = 0, 0, 0
            wake()
            for _ in range(200):
                await asyncio.sleep(0)
            calls.append((conn.rows, work["encoded"], work["counted"]))
        stop.set()
        await task
    asyncio.run(go())
    db.close()
    return calls


def test_a_wake_costs_the_same_with_200_or_3000_unsent_events(tmp_path: Path, monkeypatch: Any) -> None:
    for idle, new in (_wake_costs(tmp_path, 200, monkeypatch), _wake_costs(tmp_path, 3000, monkeypatch)):
        assert idle == (0, 0, 0)                                   # nothing new: nothing read, encoded or counted
        rows, encoded, counted = new                               # one new event: its row, its JSON, at most the
        assert (rows, encoded) == (1, 1) and counted <= TOKEN_SLICE + 300      # last slice and the event counted


def test_the_reference_reads_every_unsent_row_on_every_wake(tmp_path: Path) -> None:
    db, clock = _store(tmp_path, 3000)
    conn = Counted(db.conn)
    db.conn = conn
    batcher = reference.EventBatcher(db, clock, SequentialIds("b"), lambda: object(),
                                     NotificationPolicy(db, clock, SequentialIds("n"), Redactor()), lambda *a: None)
    assert len(batcher.pending()) == 3000 and conn.rows == 3000
    db.close()


def test_the_loop_has_no_fixed_timer_and_flushes_at_the_age_deadline(tmp_path: Path) -> None:
    db = Database(tmp_path / "ferb.db")
    db.migrate()
    clock, ids = FakeClock(), SequentialIds("d")
    trail = Trail(db, clock, ids, Redactor(), Exclusions, lambda *a: None, lambda: "dev", lambda: False)
    published: list[str] = []

    class Fake:
        async def answers(self, state: Any, asks: Any) -> dict[str, str]:
            return {asks[0].key: "no"}
    batcher = current.EventBatcher(db, clock, ids, lambda: Fake(), NotificationPolicy(db, clock, ids, Redactor()),
                                   lambda topic, data: published.append(data.get("reason", "")),
                                   oldest_seconds=lambda: 60.0)
    trail.tag = batcher.notify
    looks = [0]
    real = batcher._sync

    def sync() -> Any:
        looks[0] += 1
        return real()
    batcher._sync = sync                                           # type: ignore[method-assign]

    async def go() -> None:
        stop = asyncio.Event()
        task = asyncio.create_task(batcher.run(stop))
        trail.record(Observation("window", {"bundle_id": "com.microsoft.VSCode", "title": "FERB"}))
        await asyncio.sleep(1.3)                                   # real time: a 1 s timer would have looked again
        assert looks[0] <= 2 and published == []
        clock.advance(59.9)
        await asyncio.sleep(0.05)
        assert published == []
        clock.advance(0.1)                                         # the oldest event is 60 s old
        await asyncio.sleep(0.05)
        assert published == ["age"]
        stop.set()
        await asyncio.wait_for(task, 0.5)                          # stop is seen at once, not after a timer
    asyncio.run(go())
    db.close()


# ---------- the unsent rows stay in step with the trail ----------

def test_the_unsent_rows_and_their_count_are_always_pending_and_its_count(tmp_path: Path) -> None:
    """After any mix of new rows, flushes (some cut off), deletes, rewrites and restarts."""
    db = Database(tmp_path / "ferb.db")
    db.migrate()
    clock, ids = FakeClock(), SequentialIds("i")
    trail = Trail(db, clock, ids, Redactor(), Exclusions, lambda *a: None, lambda: "dev", lambda: False)
    rng, calls = random.Random(11), [0]

    class Fake:
        async def answers(self, state: Any, asks: Any) -> dict[str, str]:
            calls[0] += 1
            if calls[0] % 5 == 0:
                await asyncio.Event().wait()
            return {}
    batcher = current.EventBatcher(db, clock, ids, lambda: Fake(), NotificationPolicy(db, clock, ids, Redactor()),
                                   lambda *a: None)

    async def cut_off() -> None:
        job = asyncio.create_task(batcher.flush("age"))
        for _ in range(500):
            await asyncio.sleep(0)
        job.cancel()
        try:
            await job
        except asyncio.CancelledError:
            pass
    for n in range(300):
        op = rng.choices(["record", "flush", "delete", "rewrite", "restart"], [70, 8, 8, 8, 6])[0]
        if op == "record":
            trail.record(_observation(random.Random(n), n, None, 0.02, rng.random() < 0.5))
        elif op == "flush":
            asyncio.run(cut_off())
        elif op == "restart":
            batcher = current.EventBatcher(db, clock, ids, lambda: Fake(), NotificationPolicy(db, clock, ids, Redactor()),
                                           lambda *a: None)
        else:
            row = db.conn.execute("SELECT id FROM trail ORDER BY seq DESC LIMIT 1 OFFSET ?", (rng.randint(0, 9),)).fetchone()
            if row and op == "delete":
                db.conn.execute("DELETE FROM trail WHERE id=?", (row[0],))
            elif row:
                db.conn.execute("UPDATE trail SET data_json=? WHERE id=?", (json.dumps({"kept": words(rng, 2)}), row[0]))
        held, want = batcher._sync(), batcher.pending()
        assert held.events() == want
        assert held.tokens() == count_tokens(current._text(want)) if want else held.rows == {}
        assert held.interrupts == sum(e["source"] in current.INTERRUPTS for e in want)
        assert not want or held.first_at() == want[0]["at"]
    db.close()


def test_rows_another_batcher_sent_before_this_one_read_them_stay_sent(tmp_path: Path) -> None:
    db = Database(tmp_path / "ferb.db")
    db.migrate()
    clock, ids = FakeClock(), SequentialIds("a")
    trail = Trail(db, clock, ids, Redactor(), Exclusions, lambda *a: None, lambda: "dev", lambda: False)

    class Fake:
        async def answers(self, state: Any, asks: Any) -> dict[str, str]:
            return {}
    one, other = (current.EventBatcher(db, clock, ids, lambda: Fake(), NotificationPolicy(db, clock, ids, Redactor()),
                                       lambda *a: None) for _ in range(2))
    assert other._sync().rows == {}
    trail.record(Observation("window", {"bundle_id": "com.apple.Notes", "title": "sent by the other"}))
    asyncio.run(one.flush("age"))
    assert other._sync().rows == {} and other.pending() == []
    db.close()


@pytest.mark.parametrize("over", (-1, 0, 1))
def test_a_flush_for_tokens_is_at_exactly_the_budget(over: int, tmp_path: Path, monkeypatch: Any) -> None:
    """The unsent list's count against EVENT_BUDGET at the boundary, beside the reference (count >= budget)."""
    async def go() -> None:
        old, new = World(tmp_path / "old.db", reference, 1e6), World(tmp_path / "new.db", current, 1e6)
        monkeypatch.setattr(reference, "asyncio", _reference_timer(old))
        worlds = [old, new]
        for w in worlds:
            w.on = False
            for n in range(40):
                w.trail.record(_observation(random.Random(n), n, None, 0.0, True))
        await _settle(worlds)
        exact = count_tokens(reference._text(old.batcher.pending()))
        for module in (reference, current):
            monkeypatch.setattr(module, "EVENT_BUDGET", exact + over)
        for w in worlds:
            w.on = True
            w.batcher.wake.set()
        await _settle(worlds)
        assert new.log == old.log and new.tables() == old.tables()
        assert bool(old.tables()[0]) == (over <= 0)                 # a "tokens" flush exactly when count >= budget
        for w in worlds:
            await w.restart(cancel=True, again=False)
            w.db.close()
    asyncio.run(go())


def test_deleting_or_rewriting_a_trail_row_counts_a_trail_change(tmp_path: Path) -> None:
    db = Database(tmp_path / "ferb.db")
    db.migrate()
    trail = Trail(db, FakeClock(), SequentialIds("t"), Redactor(), Exclusions, lambda *a: None, lambda: "dev",
                  lambda: False)
    changes = lambda: db.conn.execute("SELECT n FROM trail_changes").fetchone()[0]        # noqa: E731
    first = trail.record(Observation("clipboard", {"text": "copied"}))
    second = trail.record(Observation("clipboard", {"text": "copied again"}))
    assert changes() == 0                                          # a new row comes after the last seq read
    db.conn.execute("UPDATE trail SET relevance='low' WHERE id=?", (first.id,))
    assert changes() == 0                                          # relevance never reaches Jev
    db.conn.execute("UPDATE trail SET data_json=? WHERE id=?", (json.dumps({"content_expired": True}), first.id))
    db.conn.execute("DELETE FROM trail WHERE id=?", (second.id,))
    assert changes() == 2
    db.close()


def test_an_open_write_is_read_whole_and_a_rolled_back_row_never_hides_the_next(tmp_path: Path) -> None:
    db = Database(tmp_path / "ferb.db")
    db.migrate()
    clock, ids = FakeClock(), SequentialIds("r")
    trail = Trail(db, clock, ids, Redactor(), Exclusions, lambda *a: None, lambda: "dev", lambda: False)
    batcher = current.EventBatcher(db, clock, ids, lambda: object(), NotificationPolicy(db, clock, ids, Redactor()),
                                   lambda *a: None)
    kept = trail.record(Observation("window", {"bundle_id": "com.apple.Notes", "title": "kept"}))
    assert [e["id"] for e in batcher._sync().events()] == [kept.id]
    db.conn.execute("BEGIN")
    undone = trail.record(Observation("window", {"bundle_id": "com.apple.Notes", "title": "rolled back"}))
    seq = lambda ident: db.conn.execute("SELECT seq FROM trail WHERE id=?", (ident,)).fetchone()[0]   # noqa: E731
    undone_seq = seq(undone.id)
    assert [e["id"] for e in batcher._sync().events()] == [kept.id, undone.id] == [e["id"] for e in batcher.pending()]
    db.conn.execute("ROLLBACK")
    later = trail.record(Observation("window", {"bundle_id": "com.apple.Notes", "title": "takes its seq"}))
    assert seq(later.id) == undone_seq                             # SQLite hands the rolled-back seq out again
    assert [e["id"] for e in batcher._sync().events()] == [kept.id, later.id] == [e["id"] for e in batcher.pending()]
    db.close()


def test_a_flush_sees_rows_deleted_or_rewritten_since_they_were_read(tmp_path: Path) -> None:
    db = Database(tmp_path / "ferb.db")
    db.migrate()
    clock, ids = FakeClock(), SequentialIds("f")
    trail = Trail(db, clock, ids, Redactor(), Exclusions, lambda *a: None, lambda: "dev", lambda: False)
    seen: list[Any] = []

    class Fake:
        async def answers(self, state: Any, asks: Any) -> dict[str, str]:
            seen.append(state["events"])
            return {}
    batcher = current.EventBatcher(db, clock, ids, lambda: Fake(), NotificationPolicy(db, clock, ids, Redactor()),
                                   lambda *a: None)
    rows = [trail.record(Observation("clipboard", {"bundle_id": "com.apple.Notes", "text": f"copy {i}"}))
            for i in range(3)]
    assert len(batcher._sync().rows) == 3
    db.conn.execute("DELETE FROM trail WHERE id=?", (rows[0].id,))
    db.conn.execute("UPDATE trail SET data_json=? WHERE id=?",
                    (json.dumps({"bundle_id": "com.apple.Notes", "content_expired": True}), rows[1].id))
    want = batcher.pending()
    asyncio.run(batcher.flush("age"))
    assert seen == [want] and [e["data"] for e in want] == [{"bundle_id": "com.apple.Notes", "content_expired": True},
                                                          {"bundle_id": "com.apple.Notes", "text": "copy 2"}]
    assert batcher._sync().rows == {} and batcher.pending() == []
    db.close()


def test_a_flush_cancelled_midway_reads_the_unsent_rows_again(tmp_path: Path) -> None:
    db = Database(tmp_path / "ferb.db")
    db.migrate()
    clock, ids = FakeClock(), SequentialIds("c")
    trail = Trail(db, clock, ids, Redactor(), Exclusions, lambda *a: None, lambda: "dev", lambda: False)
    calls = [0]

    class Fake:
        async def answers(self, state: Any, asks: Any) -> dict[str, str]:
            calls[0] += 1
            if calls[0] > 1:
                await asyncio.Event().wait()
            return {}
    batcher = current.EventBatcher(db, clock, ids, lambda: Fake(), NotificationPolicy(db, clock, ids, Redactor()),
                                   lambda *a: None)
    for app in ("com.apple.Notes", "com.microsoft.VSCode", "com.google.Chrome"):
        trail.record(Observation("window", {"bundle_id": app, "title": "x"}))

    async def go() -> None:
        job = asyncio.create_task(batcher.flush("age"))
        while calls[0] < 2:
            await asyncio.sleep(0)
        job.cancel()
        with pytest.raises(asyncio.CancelledError):
            await job
    asyncio.run(go())
    assert [e["id"] for e in batcher._sync().events()] == [e["id"] for e in batcher.pending()]
    assert len(batcher.pending()) == 1                             # the app never sent; the in-flight one has receipts
    db.close()


def test_restart_releases_in_flight_receipts_and_reads_them_again(tmp_path: Path) -> None:
    db = Database(tmp_path / "ferb.db")
    db.migrate()
    clock, ids = FakeClock(), SequentialIds("s")
    trail = Trail(db, clock, ids, Redactor(), Exclusions, lambda *a: None, lambda: "dev", lambda: False)
    batcher = current.EventBatcher(db, clock, ids, lambda: object(), NotificationPolicy(db, clock, ids, Redactor()),
                                   lambda *a: None)
    event = trail.record(Observation("window", {"bundle_id": "com.microsoft.VSCode", "title": "FERB"}))
    assert len(batcher._sync().rows) == 1
    db.conn.execute("INSERT INTO perception_batches VALUES('batch',?,'age','pending',?,NULL)",
                    (clock.now().isoformat(), json.dumps([event.id])))
    db.conn.execute("INSERT INTO perception_receipts VALUES(?,'batch')", (event.id,))
    stop = asyncio.Event()
    stop.set()
    asyncio.run(batcher.run(stop))
    assert [e["id"] for e in batcher._sync().events()] == [event.id]
    db.close()
