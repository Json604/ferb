"""Owns the browser snapshot data projection and marked initial previews.
Must not discard source labels, values or frame text; browser.read pages the complete saved snapshot.
Secret values expose only filled/empty state. in_view, offscreen, occluded and container describe observed UI facts."""

from typing import Any

from ferb.core.types import PageElement, PageSnapshot

MAX_TEXT = 8000  # initial tool preview only; browser.read retrieves the complete saved snapshot


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
                       label=str(raw.get("label") or ""), value=value, rect=(x, y, w, h), visible=True,
                       input_type=raw.get("input_type"), href=raw.get("href"), form=raw.get("form"), secret=secret,
                       code_fill=secret and raw.get("code_fill") is True,
                       in_view=in_view, offscreen=raw.get("offscreen") if not in_view else None,
                       occluded=bool(raw.get("occluded")), container=raw.get("container") or None,
                       state=tuple(state), level=int(level) if isinstance(level, (int, float)) else None)


def for_model(snap: PageSnapshot, *, full: bool = False) -> dict[str, Any]:
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
        if e.form:
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
    omitted = 0
    if not full:
        import json
        budget, shown = 24000, []
        for item in els:
            size = len(json.dumps(item, ensure_ascii=False))
            if size <= budget:
                shown.append(item)
                budget -= size
            else:
                omitted += 1
        els = shown
    text = snap.text if full else snap.text[:MAX_TEXT]
    out: dict[str, Any] = {"tab": snap.tab.tab_id, "url": snap.url, "title": snap.title, "elements": els,
                           "text": text}
    if snap.scroll:
        out["scroll"] = snap.scroll
    if snap.scrollers:
        out["scrollers"] = snap.scrollers
    if snap.media:                                 # L26: confirm playback before listening or recording
        out["media"] = snap.media
    if snap.truncated or omitted or len(text) < len(snap.text):
        out["truncated"] = True
        out["omitted_elements"] = omitted
        out["total_text_chars"] = len(snap.text)
        out["next"] = "browser.read / ferb_browser.read with this tab and offset=0 retrieves the full saved snapshot as paged JSON"
        if len(text) < len(snap.text):
            out["text"] += "\n[Text preview ends; fetch the full snapshot with browser.read]"
    if snap.dialogs:
        out["dialogs"] = snap.dialogs
    return out
