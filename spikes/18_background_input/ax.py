"""Spike 18 helpers: a small Accessibility reader/actor (throwaway; production code lives in ferb/act/mac)."""
import subprocess
import time

import ApplicationServices as AS
import AppKit


def pid_of(bundle: str) -> int | None:
    apps = AppKit.NSRunningApplication.runningApplicationsWithBundleIdentifier_(bundle)
    return int(apps[0].processIdentifier()) if apps.count() else None


def front() -> str:
    return str(AppKit.NSWorkspace.sharedWorkspace().frontmostApplication().bundleIdentifier())


def attr(el, name):
    err, v = AS.AXUIElementCopyAttributeValue(el, name, None)
    return v if err == 0 else None


def actions(el) -> list[str]:
    err, v = AS.AXUIElementCopyActionNames(el, None)
    return list(v) if err == 0 and v else []


def walk(el, depth=0, out=None, limit=4000):
    out = [] if out is None else out
    if len(out) > limit or depth > 40:
        return out
    out.append((depth, el))
    for c in attr(el, "AXChildren") or []:
        walk(c, depth + 1, out, limit)
    return out


def label(el) -> str:
    parts = [attr(el, k) for k in ("AXTitle", "AXDescription", "AXPlaceholderValue", "AXHelp", "AXIdentifier")]
    return " | ".join(str(p) for p in parts if p)


def app_el(pid: int):
    return AS.AXUIElementCreateApplication(pid)


def set_value(el, text: str) -> int:
    return AS.AXUIElementSetAttributeValue(el, "AXValue", text)


def press(el) -> int:
    return AS.AXUIElementPerformAction(el, "AXPress")


# ---------- WhatsApp probe helpers ----------
import Quartz

BUNDLE = "net.whatsapp.WhatsApp"


def window():
    return attr(app_el(pid_of(BUNDLE)), "AXMainWindow")


def find(pred):
    return [el for _, el in walk(window()) if pred(el)]


def fields():
    return find(lambda el: attr(el, "AXRole") in ("AXTextField", "AXSearchField", "AXTextArea"))


def rows() -> int:
    return len(find(lambda el: attr(el, "AXRole") == "AXButton" and "Double tap to open chat" in label(el)))


def keys_to_pid(pid: int, text: str) -> None:
    src = Quartz.CGEventSourceCreate(Quartz.kCGEventSourceStatePrivate)
    for ch in text:
        for down in (True, False):
            ev = Quartz.CGEventCreateKeyboardEvent(src, 0, down)
            Quartz.CGEventKeyboardSetUnicodeString(ev, 1, ch)
            Quartz.CGEventPostToPid(pid, ev)
            time.sleep(0.02)


