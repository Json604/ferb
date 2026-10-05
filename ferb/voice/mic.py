"""Apply the owner's documented microphone preference and report device facts. Must not open a Bluetooth microphone.

Which microphone push-to-talk opens: never a Bluetooth headset's microphone — opening it,
even briefly, drops the headset into low-quality call mode. The choice is made from the device list FERB.app reports
(read from device properties only, nothing opened); FERB.app refuses a Bluetooth input as a second guard."""

from dataclasses import dataclass

BLUETOOTH = frozenset({"blue", "blea"})       # Core Audio transport types: Bluetooth, Bluetooth LE
BUILT_IN = "bltn"


@dataclass(frozen=True)
class InputDevice:
    uid: str
    name: str
    transport: str                            # Core Audio transport type as its four characters
    default: bool = False


def choose(devices: list[InputDevice], mode: str = "auto") -> InputDevice | None:
    """auto / default: the default input unless it's Bluetooth, then the built-in mic; builtin: always the built-in
    mic. Never a Bluetooth device, whatever the mode."""
    usable = [d for d in devices if d.transport not in BLUETOOTH]
    built_ins = [d for d in usable if d.transport == BUILT_IN]
    built_in = built_ins[0] if len(built_ins) == 1 else None
    if mode == "builtin":
        return built_in
    defaults = [d for d in usable if d.default]
    if len(defaults) > 1:
        return None
    default = defaults[0] if defaults else None
    return default or built_in
