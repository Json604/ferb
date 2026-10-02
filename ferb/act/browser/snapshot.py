"""Unified page snapshot format (section 9.4): elements with ids, role, label, value, rect, visibility.

The walk itself lives in the extension's content script (extensions/chromium/content.js, an isolated world the
page can't see or change); this module holds what every driver shares: the caps, the secret-field rule and the
model-facing form. Password / OTP values never leave the page: the walker reports only whether such a field is
filled. Viewport semantics (PLATFORM_BROWSER.md 6): `in_view` means clickable now; `offscreen` means scroll
first (the element's `container` says which scroller); `occluded` means something covers it.
"""

from typing import Any

from ferb.core.types import PageElement, PageSnapshot

MAX_ELEMENTS, MAX_TEXT, MAX_LABEL, MAX_VALUE = 400, 8000, 120, 200


def element(eid: str, frame_id: str, raw: dict[str, Any]) -> PageElement:
    x, y, w, h = (float(v) for v in raw.get("rect", [0, 0, 0, 0]))
    secret = bool(raw.get("secret"))
    value = raw.get("value")
    if secret:
        value = "filled" if value else ""          # belt and braces: the walker already reports only this
    state: list[str] = []
    if raw.get("disabled"):
        state.append("disabled")
    if raw.get("expanded") is True:
        state.append("expanded")
    elif raw.get("expanded") is False:
        state.append("collapsed")
    if raw.get("selected") is True:
        state.append("selected")
    in_view = raw.get("in_view", True) is not False
    level = raw.get("level")
    return PageElement(id=eid, frame_id=frame_id, role=str(raw.get("role") or "generic"),
                       label=str(raw.get("label") or "")[:MAX_LABEL], value=value, rect=(x, y, w, h), visible=True,
                       input_type=raw.get("input_type"), href=raw.get("href"), form=raw.get("form"), secret=secret,
                       in_view=in_view, offscreen=raw.get("offscreen") if not in_view else None,
                       occluded=bool(raw.get("occluded")), container=raw.get("container") or None,
                       state=tuple(state), level=int(level) if isinstance(level, (int, float)) else None)


def for_model(snap: PageSnapshot) -> dict[str, Any]:
    """What a worker sees. The gateway's redaction pipeline runs over it again (tokens in URLs, known secrets)."""
    els = []
    for e in snap.elements:
        d: dict[str, Any] = {"id": e.id, "role": e.role, "label": e.label}
        if e.secret:                               # password / code field: whether it's filled, never a value
            d["sensitive"], d["filled"] = True, bool(e.value)
        elif e.value not in (None, ""):
            d["value"] = e.value
        if e.input_type and e.input_type not in ("text", "submit", "button"):
            d["type"] = e.input_type
        if e.href:
            d["href"] = e.href
        if e.form in ("search", "login"):
            d["form"] = e.form
        if e.level is not None:
            d["level"] = e.level
        if not e.in_view:
            d["in_view"] = False
            if e.offscreen:
                d["offscreen"] = e.offscreen
            if e.occluded:
                d["occluded"] = True
        if e.container:
            d["container"] = e.container
        if e.state:
            d["state"] = list(e.state)
        els.append(d)
    out: dict[str, Any] = {"tab": snap.tab.tab_id, "url": snap.url, "title": snap.title, "elements": els,
                           "text": snap.text}
    if snap.scroll:
        out["scroll"] = snap.scroll
    if snap.scrollers:
        out["scrollers"] = snap.scrollers
    if snap.truncated:
        out["truncated"] = True
    if snap.dialogs:
        out["dismissed_dialogs"] = snap.dialogs
    return out
