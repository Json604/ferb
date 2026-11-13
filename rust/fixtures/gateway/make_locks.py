"""Fixtures for the gateway's resource locks (ferb/act/locks.py `ResourceLocks`), above all `try_hold`, the live
preview's way in (DMN-403, DMN-406): scripted steps on the real class, recording what each `try_hold` answered and
who holds each key afterwards.

    cd <repo> && uv run python rust/fixtures/gateway/make_locks.py

Writes locks.json beside this file. Steps:
- `hold {id, keys, holder, reading?}`: the worker's call takes the locks (it gets them at once in every scenario).
- `hold_bg {id, keys, holder, reading?}`: a call queued behind a holder (its task runs until it waits).
- `release {id}`: that call ends (its locks go; a waiter is woken but has not run yet).
- `run`: the event loop turns until every runnable task waited again (the woken waiter takes its locks).
- `cancel {id}`: a queued call is cancelled, then `run`.
- `try {keys, holder, during_reads?}`: a bystander's look; it answers whether it may look and lets go at once.
- `try_keep {id, keys, holder, during_reads?}`: the same, kept until `release {id}`.
- `holders {keys}`: who holds each key.
Each step's result is recorded (`try`: the answer; `holders`: the map; errors: the ValueError's text). The cases are
tests/test_hangs_and_previews.py's and tests/test_task_preview.py's lock cases, plus edges read from the code (a lock
just handed to a waking worker, several keys with one busy, an unknown kind). This module starts no program.
Invented data only; the same bytes on every run.
"""

import asyncio
import json
import sys
from pathlib import Path
from typing import Any

REPO = Path(__file__).resolve().parents[3]
HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(REPO))

from ferb.act.locks import ResourceLocks  # noqa: E402

TAB = ["tab:b/1"]
SCENARIOS: list[dict[str, Any]] = [
    {"name": "looks_alongside_a_read", "from": "test_hangs_and_previews.py::test_the_preview_looks_alongside_a_read",
     "steps": [
         {"do": "hold", "id": "w", "keys": TAB, "holder": "worker", "reading": True},
         {"do": "try", "keys": TAB, "holder": "preview", "during_reads": True},
         {"do": "holders", "keys": TAB},
         {"do": "try", "keys": TAB, "holder": "preview"},
         {"do": "hold_bg", "id": "next", "keys": TAB, "holder": "next"},
         {"do": "try", "keys": TAB, "holder": "preview", "during_reads": True},
         {"do": "cancel", "id": "next"},
         {"do": "try", "keys": TAB, "holder": "preview", "during_reads": True},
         {"do": "release", "id": "w"},
         {"do": "hold", "id": "act", "keys": TAB, "holder": "worker"},
         {"do": "try", "keys": TAB, "holder": "preview", "during_reads": True},
         {"do": "release", "id": "act"},
         {"do": "try_keep", "id": "p", "keys": TAB, "holder": "preview", "during_reads": True},
         {"do": "holders", "keys": TAB},
         {"do": "release", "id": "p"},
         {"do": "holders", "keys": TAB},
     ]},
    {"name": "never_ahead_of_a_waking_worker", "from": "locks.py:26 _waited_for; DMN-403",
     "steps": [
         {"do": "hold", "id": "a", "keys": ["app:com.example.Notes"], "holder": "w1"},
         {"do": "hold_bg", "id": "b", "keys": ["app:com.example.Notes"], "holder": "w2"},
         {"do": "release", "id": "a"},
         {"do": "try", "keys": ["app:com.example.Notes"], "holder": "preview", "during_reads": True},
         {"do": "try", "keys": ["app:com.example.Notes"], "holder": "preview"},
         {"do": "run"},
         {"do": "holders", "keys": ["app:com.example.Notes"]},
         {"do": "try", "keys": ["app:com.example.Notes"], "holder": "preview", "during_reads": True},
         {"do": "release", "id": "b"},
         {"do": "try", "keys": ["app:com.example.Notes"], "holder": "preview"},
     ]},
    {"name": "a_read_with_a_waiter_is_busy", "from": "locks.py:59 try_hold during_reads",
     "steps": [
         {"do": "hold", "id": "r", "keys": TAB, "holder": "reader", "reading": True},
         {"do": "hold_bg", "id": "q", "keys": TAB, "holder": "actor"},
         {"do": "try", "keys": TAB, "holder": "preview", "during_reads": True},
         {"do": "release", "id": "r"},
         {"do": "run"},
         {"do": "try", "keys": TAB, "holder": "preview", "during_reads": True},
         {"do": "holders", "keys": TAB},
         {"do": "release", "id": "q"},
         {"do": "holders", "keys": TAB},
     ]},
    {"name": "several_keys_all_or_none", "from": "locks.py:60 every lock only if each is free",
     "steps": [
         {"do": "hold", "id": "f", "keys": ["file:/x/a.txt"], "holder": "worker"},
         {"do": "try_keep", "id": "p", "keys": ["tab:b/2", "file:/x/a.txt", "app:com.example.A"],
          "holder": "preview"},
         {"do": "holders", "keys": ["tab:b/2", "file:/x/a.txt", "app:com.example.A"]},
         {"do": "release", "id": "f"},
         {"do": "try_keep", "id": "p2", "keys": ["tab:b/2", "file:/x/a.txt", "app:com.example.A", "tab:b/2"],
          "holder": "preview"},
         {"do": "holders", "keys": ["tab:b/2", "file:/x/a.txt", "app:com.example.A"]},
         {"do": "try", "keys": ["file:/x/a.txt"], "holder": "other"},
         {"do": "release", "id": "p2"},
         {"do": "holders", "keys": ["tab:b/2", "file:/x/a.txt", "app:com.example.A"]},
     ]},
    {"name": "a_read_is_looked_at_and_the_rest_taken", "from": "locks.py:75 a read holds it: not taken",
     "steps": [
         {"do": "hold", "id": "r", "keys": ["tab:b/3"], "holder": "reader", "reading": True},
         {"do": "try_keep", "id": "p", "keys": ["tab:b/3", "app:com.example.B"], "holder": "preview",
          "during_reads": True},
         {"do": "holders", "keys": ["tab:b/3", "app:com.example.B"]},
         {"do": "release", "id": "r"},
         {"do": "holders", "keys": ["tab:b/3", "app:com.example.B"]},
         {"do": "try", "keys": ["tab:b/3"], "holder": "other"},
         {"do": "release", "id": "p"},
         {"do": "try", "keys": ["app:com.example.B"], "holder": "other"},
     ]},
    {"name": "a_bystander_holds_and_the_worker_waits", "from": "test_task_preview.py never queues; abandoned frees",
     "steps": [
         {"do": "try_keep", "id": "p", "keys": ["app:com.example.Notes"], "holder": "preview:t"},
         {"do": "hold_bg", "id": "w", "keys": ["app:com.example.Notes"], "holder": "worker"},
         {"do": "holders", "keys": ["app:com.example.Notes"]},
         {"do": "release", "id": "p"},
         {"do": "run"},
         {"do": "holders", "keys": ["app:com.example.Notes"]},
         {"do": "release", "id": "w"},
         {"do": "try", "keys": ["app:com.example.Notes"], "holder": "preview:t"},
     ]},
    {"name": "unknown_kind", "from": "locks.py:22",
     "steps": [
         {"do": "try", "keys": ["disk:x"], "holder": "preview"},
         {"do": "try", "keys": ["tab:ok", "screen:main"], "holder": "preview", "during_reads": True},
     ]},
]


async def _run(sc: dict[str, Any]) -> list[Any]:
    locks = ResourceLocks()
    held: dict[str, Any] = {}       # id -> its async context manager, entered
    queued: dict[str, asyncio.Task[Any]] = {}
    out: list[Any] = []

    async def settle() -> None:
        for _ in range(10):
            await asyncio.sleep(0)

    for st in sc["steps"]:
        do = st["do"]
        result: Any = None
        if do == "hold":
            cm = locks.hold(st["keys"], st["holder"], reading=st.get("reading", False))
            await cm.__aenter__()
            held[st["id"]] = cm
        elif do == "hold_bg":
            cm = locks.hold(st["keys"], st["holder"], reading=st.get("reading", False))
            held[st["id"]] = cm
            queued[st["id"]] = asyncio.create_task(cm.__aenter__())
            await settle()
        elif do == "release":
            cm = held.pop(st["id"])
            await cm.__aexit__(None, None, None)
        elif do == "run":
            await settle()
        elif do == "cancel":
            queued.pop(st["id"]).cancel()
            held.pop(st["id"])
            await settle()
        elif do in ("try", "try_keep"):
            cm = locks.try_hold(st["keys"], st["holder"], during_reads=st.get("during_reads", False))
            try:
                result = await cm.__aenter__()
            except ValueError as e:
                result = {"error": str(e)}
            else:
                if do == "try":
                    await cm.__aexit__(None, None, None)
                else:
                    held[st["id"]] = cm
        elif do == "holders":
            result = {k: locks.holder(k) for k in st["keys"]}
        else:
            raise SystemExit(f"unknown step {do}")
        out.append(result)
    return out


def main() -> None:
    scenarios = []
    for sc in SCENARIOS:
        results = asyncio.run(_run(sc))
        scenarios.append({**sc, "results": results})
    (HERE / "locks.json").write_text(json.dumps({"made_by": "rust/fixtures/gateway/make_locks.py",
                                                 "scenarios": scenarios}, indent=1) + "\n")


if __name__ == "__main__":
    main()
