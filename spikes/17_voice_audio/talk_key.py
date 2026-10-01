"""Spike 17 (throwaway): the push-to-talk key from a listen-only event tap, like FERB's owner-input watcher.
Hold Right Option to talk, release to send; a hold under 0.3 s is ignored; another key pressed while it's down makes
it a shortcut and the "recording" is discarded. Logs decisions (never which keys) to /tmp/ferb-talk-key.log."""

import sys
import time

import Quartz

RIGHT_OPTION, MIN_HOLD_S, LOG = 61, 0.3, "/tmp/ferb-talk-key.log"
state = {"down_at": None, "other": False}


def log(line: str) -> None:
    print(line, flush=True)
    with open(LOG, "a") as f:
        f.write(line + "\n")


def cb(proxy, etype, event, refcon):  # type: ignore[no-untyped-def]
    code = Quartz.CGEventGetIntegerValueField(event, Quartz.kCGKeyboardEventKeycode)
    if etype == Quartz.kCGEventFlagsChanged and code == RIGHT_OPTION:
        down = bool(Quartz.CGEventGetFlags(event) & Quartz.kCGEventFlagMaskAlternate)
        now = time.monotonic()
        if down and state["down_at"] is None:
            state["down_at"], state["other"] = now, False
            log(f"{time.strftime('%H:%M:%S')} right-option down -> start recording")
        elif not down and state["down_at"] is not None:
            held = now - state["down_at"]
            verdict = ("shortcut: discard" if state["other"] else
                       "tap: ignore" if held < MIN_HOLD_S else "talk: send")
            log(f"{time.strftime('%H:%M:%S')} released after {held:.2f}s -> {verdict} "
                f"(detected {(time.monotonic() - now) * 1000:.1f} ms after the event)")
            state["down_at"] = None
    elif state["down_at"] is not None and etype in (Quartz.kCGEventKeyDown, Quartz.kCGEventFlagsChanged):
        state["other"] = True                                    # another key while held: a shortcut
    return event


mask = (Quartz.CGEventMaskBit(Quartz.kCGEventFlagsChanged) | Quartz.CGEventMaskBit(Quartz.kCGEventKeyDown))
tap = Quartz.CGEventTapCreate(Quartz.kCGSessionEventTap, Quartz.kCGHeadInsertEventTap,
                              Quartz.kCGEventTapOptionListenOnly, mask, cb, None)
if tap is None:
    log("no event tap: this terminal needs Input Monitoring or Accessibility")
    sys.exit(1)
src = Quartz.CFMachPortCreateRunLoopSource(None, tap, 0)
Quartz.CFRunLoopAddSource(Quartz.CFRunLoopGetCurrent(), src, Quartz.kCFRunLoopCommonModes)
Quartz.CGEventTapEnable(tap, True)
seconds = float(sys.argv[1]) if len(sys.argv) > 1 else 30
log(f"listening {seconds:.0f}s")
Quartz.CFRunLoopRunInMode(Quartz.kCFRunLoopDefaultMode, seconds, False)
log("done")
