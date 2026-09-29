"""GUI lock (section 7 step 7): one GUI actor at a time. Taken for real mouse/keyboard input only, held across one
action's observe -> act -> verify, never across model calls."""

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
