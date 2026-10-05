"""Owns production and deterministic test identifiers. Must not infer intent, choose a target or claim semantic success from transport facts.

IdSource implementations."""

import uuid


class RandomIds:
    def new_id(self) -> str:
        return uuid.uuid4().hex


class SequentialIds:
    """Deterministic ids for tests and replays: <prefix>-000001, <prefix>-000002, ..."""

    def __init__(self, prefix: str = "id") -> None:
        self._prefix = prefix
        self._n = 0

    def new_id(self) -> str:
        self._n += 1
        return f"{self._prefix}-{self._n:06d}"
