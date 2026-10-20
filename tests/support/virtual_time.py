"""Virtual time for asyncio tests: how long a coroutine would take with slow fakes, without sleeping.

`run_virtual(coro)` runs it on a loop whose clock jumps to the next timer whenever nothing is ready, so
`asyncio.sleep(4)` in a fake stands for a 4 s wait and costs nothing. `charged_to_thread(costs)` replaces
`asyncio.to_thread` with a fake that runs the function at once and then charges its virtual cost by name (e.g. an AX
walk's worst-case ~2 s re-read), so blocking work done in threads counts as time that other tasks can overlap. No
real threads run, so nothing else can wake the loop: waiting with no timer left is a deadlock and raises."""

import asyncio
import selectors
from typing import Any, Awaitable, Callable, Coroutine, TypeVar

T = TypeVar("T")


class _JumpSelector(selectors.DefaultSelector):            # type: ignore[misc,valid-type]
    loop: "VirtualLoop | None" = None

    def select(self, timeout: float | None = None) -> Any:
        events = super().select(0)
        if events or timeout == 0:
            return events
        if timeout is None:
            raise RuntimeError("virtual loop: nothing is ready and no timer is due (a deadlock)")
        assert self.loop is not None
        self.loop.now += timeout                             # jump to the next timer
        return events


class VirtualLoop(asyncio.SelectorEventLoop):
    def __init__(self) -> None:
        selector = _JumpSelector()
        super().__init__(selector)
        selector.loop = self
        self.now = 0.0

    def time(self) -> float:
        return self.now


def run_virtual(coro: Coroutine[Any, Any, T]) -> tuple[T, float]:
    """The coroutine's result and the virtual seconds it took."""
    loop = VirtualLoop()
    try:
        return loop.run_until_complete(coro), loop.now
    finally:
        loop.close()


def charged_to_thread(costs: dict[str, float]) -> Callable[..., Awaitable[Any]]:
    """A stand-in for asyncio.to_thread: runs the function now, then waits its cost (by function name) in virtual time."""
    async def to_thread(func: Callable[..., Any], *args: Any, **kwargs: Any) -> Any:
        result = func(*args, **kwargs)
        cost = costs.get(getattr(func, "__name__", ""), 0.0)
        if cost:
            await asyncio.sleep(cost)
        return result
    return to_thread
