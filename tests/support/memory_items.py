"""Shared helpers for memory tests on a running test daemon (L36). The daemon's loop owns SQLite, so every read and
write goes through it."""
from typing import Any

from ferb.memory import summary
from tests.conftest import FakeDistillRuntime


def run(d: Any, fn: Any) -> Any:
    async def go() -> Any:
        return fn()
    return d.run(go())


def journal(d: Any, kind: str, data: dict[str, Any]) -> str:
    return str(run(d, lambda: d.daemon._journal().append(kind, data).id))


def owner_fact(d: Any, key: str, content: str, kind: str = "fact", subject: str = "owner") -> Any:
    """A memory item in the owner's own words, as `ferb memory add` saves it."""
    src = journal(d, "memory.added", {"content": content})
    return run(d, lambda: d.daemon.memory.propose(kind, subject, key, content, "owner_stated", [("journal", src)], 1.0))


def understood(d: Any, key: str, content: str, kind: str = "fact", subject: str = "owner") -> Any:
    """FERB's understanding of a fact, not yet holding up: found by search, never in the always-on profile (the owner's
    own words, about anything, join it at once: MEM-15)."""
    src = journal(d, "chat.message", {"text": content})
    return run(d, lambda: d.daemon.memory.propose(kind, subject, key, content, "inferred", [("journal", src)]))


def today(d: Any) -> str:
    return str(d.daemon.clock.now().astimezone().date().isoformat())


def said(d: Any, text: str, pasted: int | None = 0) -> str:
    """A chat message as intake journals it. `pasted` None: recorded before FERB counted pasted parts."""
    data: dict[str, Any] = {"session_id": "s1", "request_id": "r1", "text": text}
    if pasted is not None:
        data["untrusted_parts"] = pasted
    return journal(d, "chat.message", data)


def c_handle(d: Any, journal_id: str) -> str:
    """The C handle today's distill evidence gives that message."""
    ev = run(d, lambda: summary.build(d.daemon.db, today(d)))
    return next(h for h, refs in ev.handles.items() if h.startswith("C") and refs == [("journal", journal_id)])


def distill_item(handle: str, content: str, quote: str | None = None, **fields: Any) -> dict[str, Any]:
    """One distill answer item about the owner's dentist, citing `handle`, with a quote of that message or none."""
    return {"kind": "person", "subject": "owner", "key": "dentist", "content": content, "origin": "inferred",
            "confidence": 0.6, "handles": [handle],
            "quote": None if quote is None else {"handle": handle, "text": quote}, "retire": False} | fields


def distill_day(d: Any, *items: dict[str, Any]) -> dict[str, Any]:
    rt = FakeDistillRuntime({"narrative": "A day.", "items": list(items)})
    d.daemon.distiller.make_runtime = lambda *a, **k: rt
    return dict(d.run(d.daemon.distiller.run_day(today(d))))


def pending(d: Any, capability: str = "memory.confirm") -> list[Any]:
    return list(run(d, lambda: [q for q in d.daemon.state.approvals.pending() if q.capability == capability]))
