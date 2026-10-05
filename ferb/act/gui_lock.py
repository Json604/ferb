"""Own serialization of global mouse and keyboard actions. Must not hold the lock across model calls.

One GUI actor holds the lock for its observe -> act -> verify operation."""

import asyncio
from contextlib import asynccontextmanager
from typing import AsyncIterator


class GuiLock:
    def __init__(self) -> None:
        self._lock = asyncio.Lock()
        self.holder: str | None = None

    @asynccontextmanager
    async def hold(self, holder: str) -> AsyncIterator[None]:
        async with self._lock:
            self.holder = holder
            try:
                yield
            finally:
                self.holder = None
