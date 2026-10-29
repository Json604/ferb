"""On-demand task-owned preview captures. Pixels are never journaled or saved here."""
from __future__ import annotations

from collections.abc import Awaitable, Callable

Capture = Callable[[], Awaitable[bytes]]


class TaskPreviews:
    def __init__(self) -> None:
        self._targets: dict[str, Capture] = {}
        self._scopes: dict[str, str] = {}
        self._identities: dict[str, str] = {}
        self._pending: dict[str, object] = {}

    def resolving(self, task: str) -> object:
        token = object()
        self._pending[task] = token
        return token

    def is_current(self, task: str, token: object) -> bool:
        return self._pending.get(task) is token

    def bind(self, task: str, capture: Capture, scope: str = "", *, identity: str = "") -> None:
        if identity and self._identities.get(task) == identity and self._scopes.get(task) == scope:
            return  # repeated actions on one surface must not discard its in-flight frame
        self._identities[task] = identity
        self._targets[task] = capture
        self._scopes[task] = scope

    def retain_scope(self, task: str, scope: str) -> None:
        """An unresolved target in another app must not show the previous app."""
        if self._scopes.get(task) != scope:
            self.remove({task})

    def remove(self, tasks: set[str]) -> None:
        for task in self._targets.keys() | self._pending.keys():
            if any(task == root or task.startswith(root + '.') for root in tasks):
                self._targets.pop(task, None)
                self._scopes.pop(task, None)
                self._identities.pop(task, None)
                self._pending.pop(task, None)

    async def frame(self, task: str) -> bytes | None:
        capture = self._targets.get(task)
        if capture is None:
            return None
        frame = await capture()
        # A target switch or task completion during capture must discard these pixels.
        return frame if self._targets.get(task) is capture else None
