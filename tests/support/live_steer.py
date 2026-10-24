"""Shared helpers for tests that correct a task while it runs (L36): a worker that runs until released, and a planner
turn that controls a running task."""
import asyncio
import json
import threading
from typing import Any

import pytest

from ferb.core.types import RuntimeEvent
from tests.conftest import FakeRuntime, done, sup, task


class StalledWorker:
    """A worker that runs until released (or stopped), so a correction finds it live."""

    def __init__(self, monkeypatch: pytest.MonkeyPatch) -> None:
        self.started, self.release = threading.Event(), threading.Event()
        self.handles: list[Any] = []
        original = FakeRuntime.events

        async def events(rt: Any, run_: Any) -> Any:
            if run_.state['key'] != 'worker':
                async for event in original(rt, run_):
                    yield event
                return
            self.handles.append(run_)
            self.started.set()
            while not self.release.is_set() and not run_.state['cancel']:
                await asyncio.sleep(.01)
            yield RuntimeEvent('final', {'text': done('Booked the earliest slot.')})

        async def steer(rt: Any, run_: Any, message: str) -> None:
            return None
        monkeypatch.setattr(FakeRuntime, 'events', events)
        monkeypatch.setattr(FakeRuntime, 'continue_', steer)


def control_turn(goal: str, tree_id: str, action: str, **entries: Any) -> str:
    turn = json.loads(sup('task', 'Changing it.', task(goal)))
    turn['control'] = {'task_id': tree_id, 'action': action, 'standing_sends': 'unchanged', 'supersedes': []}
    return json.dumps(turn | entries)
