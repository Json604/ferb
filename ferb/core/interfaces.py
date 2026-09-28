"""Protocols at real boundaries (ARCHITECTURE.md section 5).

Each protocol is added when its first implementation lands, with section 5's signature exactly.
"""

from datetime import datetime
from typing import Protocol


class Clock(Protocol):
    def now(self) -> datetime: ...              # timezone-aware wall time: timestamps, schedules
    def monotonic(self) -> float: ...           # elapsed time + timeouts; unaffected by clock changes
    async def sleep_until(self, t: datetime) -> None: ...


class IdSource(Protocol):
    """Injected ID generation (rule 7) so any run can be replayed with deterministic ids."""
    def new_id(self) -> str: ...
