"""Optional window-addressed SkyLight mouse transport. No activation, focus changes or global posting.

Private ABI: missing symbols disable this route. Availability means we can attempt delivery, not that an app
accepts it. Routing fields follow Cua's MIT-licensed implementation; see THIRD_PARTY_NOTICES.md.
Deliberately posts once: posting through both transports could duplicate a click or a wheel tick.
"""

import ctypes
from functools import lru_cache
from typing import Any


class WindowRoute:
    def __init__(self, library: Any) -> None:
        self.library = library

    def post(self, event: Any, pid: int, window: int, point: tuple[float, float]) -> None:
        """Keep the event's global screen point; stamp a separate window-local point for native hit testing."""
        from ferb.sense.watchers.owner_input import FERB_EVENT_MARK
        ptr = event.__c_void_p__()
        for field, value in ((40, pid), (51, window), (91, window), (92, window), (42, FERB_EVENT_MARK)):
            self.library.SLEventSetIntegerValueField(ptr, field, value)
        self.library.SLEventSetWindowLocation(ptr, *point)
        self.library.SLEventPostToPid(pid, ptr)


@lru_cache(maxsize=1)
def load_route() -> WindowRoute | None:
    try:
        lib = ctypes.CDLL('/System/Library/PrivateFrameworks/SkyLight.framework/SkyLight')
        signatures = {
            'SLEventPostToPid': [ctypes.c_int32, ctypes.c_void_p],
            'SLEventSetIntegerValueField': [ctypes.c_void_p, ctypes.c_uint32, ctypes.c_int64],
            'SLEventSetWindowLocation': [ctypes.c_void_p, ctypes.c_double, ctypes.c_double],
        }
        for name, args in signatures.items():
            fn = getattr(lib, name)
            fn.argtypes, fn.restype = args, None
        return WindowRoute(lib)
    except (OSError, AttributeError):
        return None
