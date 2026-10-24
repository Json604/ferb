"""L33: where FERB is about to act, for FERB's on-screen cursor. Geometry only: never a label, value, URL or text.

A capability's `point()` must not block the event loop: synchronous Accessibility or Quartz work goes through
`asyncio.to_thread`, so the POINT_TIMEOUT_S bound holds."""
import math
from dataclasses import dataclass
from typing import Any, Literal

POINT_TIMEOUT_S = 0.5          # a point that takes longer is skipped; the action never waits for the cursor


@dataclass(frozen=True)
class Point:
    kind: Literal['click', 'type', 'scroll', 'press', 'drag']
    rect: tuple[float, float, float, float]          # x, y, w, h in global screen points, top-left origin
    display: int

    def __post_init__(self) -> None:
        if self.kind not in ('click', 'type', 'scroll', 'press', 'drag'):
            raise ValueError(f"bad point kind {self.kind!r}")
        if len(self.rect) != 4 or not all(isinstance(v, (int, float)) and math.isfinite(v) for v in self.rect):
            raise ValueError("a point's rect is four finite numbers")
        if self.rect[2] <= 0 or self.rect[3] <= 0:
            raise ValueError("a point's rect needs a positive width and height")

    def payload(self, task_id: str) -> dict[str, Any]:
        return {'task_id': task_id, 'kind': self.kind, 'rect': [float(v) for v in self.rect], 'display': self.display}


def display_of(rect: tuple[float, float, float, float]) -> int | None:
    """The display holding the rect's centre, or None when it is on no display."""
    import Quartz
    x, y, w, h = rect
    err, ids, count = Quartz.CGGetDisplaysWithPoint((x + w / 2, y + h / 2), 1, None, None)
    return int(ids[0]) if err == 0 and count else None
