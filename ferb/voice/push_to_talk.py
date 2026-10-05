"""Own the push-to-talk hotkey state machine and recording signals. Must not interpret dictated task meaning.

Push-to-talk: hold Right Option to talk, release to send.

A hold under MIN_HOLD_S is a tap and is ignored; any other key pressed while it's down makes it a shortcut and what was
heard is discarded. Fed from the daemon's listen-only event tap (owner input only; FERB's own marked events never)."""

from dataclasses import dataclass, field

RIGHT_OPTION = 61                          # kVK_RightOption
MIN_HOLD_S = 0.3


@dataclass
class TalkKey:
    keycode: int = RIGHT_OPTION
    min_hold_s: float = MIN_HOLD_S
    _down_at: float | None = field(default=None, init=False)
    _other: bool = field(default=False, init=False)

    @property
    def held(self) -> bool:
        return self._down_at is not None

    def key(self, keycode: int, down: bool, t: float) -> str | None:
        """A key event from the tap. Returns "start" (begin listening), "send", "ignore" (a tap) or "discard" (a
        shortcut) — or None when it doesn't concern the talk key."""
        if keycode != self.keycode:
            if self._down_at is not None and down:
                self._other = True                       # another key while held: a shortcut
            return None
        if down and self._down_at is None:
            self._down_at, self._other = t, False
            return "start"
        if not down and self._down_at is not None:
            held, other = t - self._down_at, self._other
            self._down_at, self._other = None, False
            return "discard" if other else "ignore" if held < self.min_hold_s else "send"
        return None
