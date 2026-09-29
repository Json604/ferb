"""MacComputerDriver (section 5 ComputerDriver): takes an action on an app, resolves the method, calls the parts.

Element actions try Accessibility first (press / set value / focus: no focus change, nothing for the owner to
notice); only if the app ignores that does FERB fall back to real input — activating the app, one marked event at a
time under the GUI lock, yielding to the owner — and then puts the previously frontmost app back. Element ids come
from the latest snapshot of that app and go stale with a newer one. Excluded apps are never read or acted on;
browsers are read-only here (ax_browser).
"""

import asyncio
from dataclasses import dataclass, field
from typing import Any, Callable, Literal

from ferb.act.mac import accessibility as axm
from ferb.act.mac import apps as appsm
from ferb.act.mac.ax_browser import ACT_REFUSED, is_browser
from ferb.act.mac.input import RealInput, click_steps, key_steps, type_steps, valid_combo
from ferb.core.errors import Refused, StaleRef
from ferb.core.types import ActionResult, ComputerAction

UNTRUSTED_APPS = {"com.apple.mail": "email", "com.apple.MobileSMS": "message", "net.whatsapp.WhatsApp": "message",
                  "com.tinyspeck.slackmacgap": "message", "com.hnc.Discord": "message",
                  "ru.keepcoder.Telegram": "message", "com.microsoft.teams2": "message"}


@dataclass
class _AppState:
    gen: int = 0
    nodes: dict[str, axm.AxNode] = field(default_factory=dict)


class MacComputerDriver:
    os: Literal["macos", "windows", "linux"] = "macos"

    def __init__(self, device_id: str, real_input: RealInput, excluded: Callable[[], set[str]] = set,
                 ax: Any = axm, apps: Any = appsm) -> None:
        self.device_id, self.input, self.excluded, self.ax, self.apps = device_id, real_input, excluded, ax, apps
        self._state: dict[str, _AppState] = {}

    def app(self, name_or_bundle: str) -> Any:
        a = self.apps.find(name_or_bundle)
        if a is None:
            raise Refused(f"{name_or_bundle} isn't running (open it first)")
        ex = {e.lower() for e in self.excluded()}
        if a.bundle_id.lower() in ex or a.name.lower() in ex:
            raise Refused(f"{a.name} is on the owner's exclusion list")
        return a

    async def snapshot(self, name_or_bundle: str) -> tuple[Any, dict[str, Any]]:
        a = self.app(name_or_bundle)
        tree = await asyncio.to_thread(self.ax.read_tree, a.pid, is_browser(a.bundle_id))
        st = self._state.setdefault(a.bundle_id, _AppState())
        st.gen += 1
        st.nodes = {f"s{st.gen}e{i + 1}": n for i, n in enumerate(tree.nodes)}
        els = []
        for eid, n in st.nodes.items():
            d: dict[str, Any] = {"id": eid, "role": n.role, "label": n.label}
            if n.secret:
                d["sensitive"], d["filled"] = True, n.value == "filled"
            elif n.value not in (None, ""):
                d["value"] = n.value
            if not n.enabled:
                d["disabled"] = True
            els.append(d)
        out = {"app": a.name, "bundle_id": a.bundle_id, "window": tree.title, "elements": els, "text": tree.text}
        if tree.truncated:
            out["truncated"] = True
        return a, out

    def node(self, bundle_id: str, element: str) -> axm.AxNode:
        st = self._state.get(bundle_id)
        n = st.nodes.get(element) if st else None
        if n is None or not element.startswith(f"s{st.gen}e" if st else "-"):
            raise StaleRef(f"element {element} is from an older snapshot of this app; take a new one")
        return n

    async def act(self, app: str, action: ComputerAction, holder: str = "") -> ActionResult:
        a = self.app(app)
        if is_browser(a.bundle_id):
            raise Refused(ACT_REFUSED)
        if action.kind == "key":
            if not action.text or not valid_combo(action.text):
                raise Refused(f"unsupported key {action.text!r} (e.g. return, tab, escape, cmd+s)")
            return await self._real(a, key_steps(action.text), holder, action.text)
        if action.element is None:
            raise Refused("this action needs an element id from a snapshot")
        n = self.node(a.bundle_id, action.element)
        if n.secret and action.kind in ("set_value", "type"):
            raise Refused("that's a password field; FERB never types passwords for the owner")
        if action.kind == "press":
            if await asyncio.to_thread(self.ax.press, n.ref):
                return ActionResult(True, "pressed")
            return await self._real(a, click_steps(*self.ax.center(n)), holder, "click")
        if action.kind == "click":
            return await self._real(a, click_steps(*self.ax.center(n)), holder, "click")
        if action.kind == "focus":
            return ActionResult(await asyncio.to_thread(self.ax.focus, n.ref), "focused")
        if action.kind == "set_value":
            if await asyncio.to_thread(self.ax.set_value, n.ref, action.text or ""):
                return ActionResult(True, "value set")
            await asyncio.to_thread(self.ax.focus, n.ref)
            return await self._real(a, type_steps(action.text or ""), holder, "typing")
        if action.kind == "type":
            await asyncio.to_thread(self.ax.focus, n.ref)
            return await self._real(a, type_steps(action.text or ""), holder, "typing")
        raise Refused(f"unknown action {action.kind}")

    async def _real(self, a: Any, steps: list[Any], holder: str, what: str) -> ActionResult:
        """Real input goes to the frontmost app: bring this one forward, act, then put the owner's app back."""
        before = self.apps.frontmost()
        if before is None or before.bundle_id != a.bundle_id:
            self.apps.activate(a.bundle_id)
            await asyncio.sleep(0.3)
        try:
            n = await self.input.run(steps, holder, what)
        finally:
            if before is not None and before.bundle_id != a.bundle_id:
                self.apps.activate(before.bundle_id)
        return ActionResult(True, f"{what} ({n} steps, real input)")
