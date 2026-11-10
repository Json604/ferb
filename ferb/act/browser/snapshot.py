"""Owns the browser snapshot data projection and marked initial previews.
Must not discard source labels, values or frame text; browser.read pages the complete saved snapshot.
Secret values expose only filled/empty state. in_view, offscreen, occluded and container describe observed UI facts."""

from typing import Any

from ferb.core.types import FormSubmit, PageElement, PageSnapshot

FIELD_KINDS = frozenset({"password", "new_password", "one_time_code", "card_number", "card_csc", "card_expiry"})

MAX_TEXT = 8000  # initial tool preview only; browser.read retrieves the complete saved snapshot

# HAND-208: the frames bot-check widgets draw in, by their address (host, and a path prefix where the host
# serves other things too). A structural fact like a field's kind: the widget's own frame, never the page's words.
CHALLENGE_FRAMES = (("challenges.cloudflare.com", "", "Cloudflare Turnstile"), ("hcaptcha.com", "", "hCaptcha"),
                    ("google.com", "/recaptcha/", "reCAPTCHA"), ("recaptcha.net", "/recaptcha/", "reCAPTCHA"),
                    ("arkoselabs.com", "", "Arkose"), ("funcaptcha.com", "", "Arkose"),
                    ("captcha-delivery.com", "", "DataDome"), ("awswaf.com", "", "AWS WAF"),
                    ("geetest.com", "", "GeeTest"))
BOT_CHECK_ABILITY = "browser_verification"
# HAND-300 (ruling, 2026-10-31): the fact says plainly what code saw, a widget's frame, and that it may be only
# a passive badge (a reCAPTCHA v3 or Turnstile footer badge draws in the same frames); no wording about bypasses
BOT_CHECK_FACT = ("A bot-check widget's frame is on this page. It may be only a passive badge that blocks nothing; "
                  "the page's own words say whether a check is asked.")


def bot_check(frame_urls: list[str]) -> dict[str, Any] | None:
    """The bot-check widgets a page's frames show (by CHALLENGE_FRAMES), stated with the ability that covers them."""
    from urllib.parse import urlsplit
    found: dict[str, str] = {}
    for url in frame_urls:
        try:
            parts = urlsplit(url)
        except ValueError:
            continue
        host = (parts.hostname or "").lower()
        for domain, path, name in CHALLENGE_FRAMES:
            if (host == domain or host.endswith("." + domain)) and parts.path.startswith(path):
                found.setdefault(host, name)
    if not found:
        return None
    return {"fact": BOT_CHECK_FACT, "widgets": sorted(set(found.values())), "frames": list(found),
            "ability": BOT_CHECK_ABILITY, "read": f"abilities.read name={BOT_CHECK_ABILITY}"}


def _submit(raw: Any) -> FormSubmit | None:
    """The extension's `submit` fact (protocol 18), kept only in its exact shape."""
    if not isinstance(raw, dict) or not (raw.get("click") is True or raw.get("enter") is True):
        return None
    host, method = raw.get("action_host"), raw.get("method")
    kinds = tuple(k for k in raw.get("kinds") or () if k in FIELD_KINDS) if isinstance(raw.get("kinds"), list) else ()
    return FormSubmit(raw.get("click") is True, raw.get("enter") is True,
                      host[:253] if isinstance(host, str) and host else None,
                      method.upper()[:10] if isinstance(method, str) and method else None, kinds,
                      raw["label"][:200] if isinstance(raw.get("label"), str) else "")


def element(eid: str, frame_id: str, raw: dict[str, Any]) -> PageElement:
    x, y, w, h = (float(v) for v in raw.get("rect", [0, 0, 0, 0]))
    kind = raw.get("field_kind") if raw.get("field_kind") in FIELD_KINDS else None
    secret = bool(raw.get("secret")) or kind is not None   # a card / password / code field is secret whatever else
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
                       state=tuple(state), level=int(level) if isinstance(level, (int, float)) else None,
                       terminal=raw.get("terminal") is True, field_kind=kind, submit=_submit(raw.get("submit")),
                       path=raw["path"][:2000] if isinstance(raw.get("path"), str) else "",
                       backend_node=raw["backend_node"] if isinstance(raw.get("backend_node"), int)
                       and not isinstance(raw.get("backend_node"), bool) else None,
                       in_dialog=raw.get("dialog") is True)


FIELD_ROLES = frozenset({"textbox", "searchbox", "combobox", "checkbox", "radio", "spinbutton", "slider", "switch",
                         "listbox"})


def _form_of(path: str) -> str | None:
    """The form an element is in, by its DOM path up to the nearest <form> (content.js domPath); None outside one."""
    parts = path.split(">") if path else []
    for i in range(len(parts) - 1, -1, -1):
        if parts[i].startswith("form:"):
            return ">".join(parts[:i + 1])
    return None


def _ranks(elements: list[PageElement]) -> list[int]:
    """HAND-483: what the preview keeps first. 0: inside an open dialog, or the submit button of a form with a field on
    screen; 1: on screen (in view, or covered there); 2: the rest."""
    def shown(e: PageElement) -> bool:
        return e.in_view or (e.occluded and not e.offscreen)
    forms = {f for e in elements if shown(e) and e.role in FIELD_ROLES and (f := _form_of(e.path))}
    return [0 if e.in_dialog or (e.submit is not None and e.submit.on_click and _form_of(e.path) in forms)
            else 1 if shown(e) else 2 for e in elements]


OMITTED_LABELS_CHARS = 3000   # the omitted buttons' and links' labels listed in a preview's index, at most


def _omitted_index(elements: list[PageElement]) -> dict[str, Any]:
    """A compact index of what the preview left out: how many of each role, and the labels of its buttons and links
    (each once, up to OMITTED_LABELS_CHARS in all; how many more, if any)."""
    by_role: dict[str, int] = {}
    labels: list[str] = []
    used, more = 0, 0
    for e in elements:
        by_role[e.role] = by_role.get(e.role, 0) + 1
        if e.role in ("button", "link") and e.label and e.label[:80] not in labels:
            if used + len(e.label[:80]) <= OMITTED_LABELS_CHARS:
                labels.append(e.label[:80])
                used += len(e.label[:80])
            else:
                more += 1
    out: dict[str, Any] = {"by_role": dict(sorted(by_role.items(), key=lambda kv: -kv[1]))}
    if labels:
        out["buttons_and_links"] = labels
    if more:
        out["more_labels"] = more
    return out


def for_model(snap: PageSnapshot, *, full: bool = False, probe: bool = False) -> dict[str, Any]:
    """What a worker sees. The gateway's redaction pipeline runs over it again (tokens in URLs, known secrets). A loop
    probe's read (`probe`, L31) also gets the media keyed by id, for its dot path; a model reads the ids in the list."""
    els = []
    for e in snap.elements:
        d: dict[str, Any] = {"id": e.id, "role": e.role, "label": e.label}
        if e.secret:                               # password / code / card field: whether it's filled, never a value
            d["sensitive"], d["filled"] = True, bool(e.value)
            if e.field_kind:
                d["field"] = e.field_kind
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
        if e.in_dialog:
            d["dialog"] = True
        els.append(d)
    omitted = omitted_in_view = 0
    index: dict[str, Any] = {}
    if not full:
        # HAND-483: the budget goes first to an open dialog's elements and the submit buttons of forms with a field on
        # screen, then to what is on screen, then the rest, each in the list's order; the preview keeps the list's
        # order. Live 2026-11-10: an Amazon buy box in view was cut for 321 off-screen items.
        import json
        ranks = _ranks(snap.elements)
        budget, keep, left = 24000, set(), []
        for rank in (0, 1, 2):
            for i, item in enumerate(els):
                if ranks[i] != rank:
                    continue
                size = len(json.dumps(item, ensure_ascii=False))
                if size <= budget:
                    keep.add(i)
                    budget -= size
                else:
                    omitted += 1
                    omitted_in_view += rank < 2
                    left.append(i)
        els = [item for i, item in enumerate(els) if i in keep]
        if left:
            index = _omitted_index([snap.elements[i] for i in sorted(left)])
    text = snap.text if full else snap.text[:MAX_TEXT]
    out: dict[str, Any] = {"tab": snap.tab.tab_id, "url": snap.url, "title": snap.title}
    if snap.loading:                               # read at once, not after every frame loaded (HAND-434): a fact
        out["loading"] = "the page was still loading when it was read; parts of it may be missing"
    check = bot_check(snap.frame_urls)
    if check is not None:                          # HAND-208: stated first, with the ability that covers it
        out["bot_check"] = check
    # L26: confirm playback before listening or recording. First, ahead of up to 24,000 characters of elements and the
    # text, so whoever reads the recorded result sees whether it plays without paging to its end (L31, live 2026-10-25)
    if snap.media:
        out["media"] = snap.media
        by_id = {m["id"]: m for m in snap.media if "id" in m} if probe else {}
        if by_id:                                  # L31: one element by its own id, wherever the list moves it
            out["media_by_id"] = by_id
    out |= {"elements": els, "text": text}
    if snap.scroll:
        out["scroll"] = snap.scroll
    if snap.scrollers:
        out["scrollers"] = snap.scrollers
    if snap.truncated or omitted or len(text) < len(snap.text):
        out["truncated"] = True
        out["omitted_elements"] = omitted
        if index:
            out["omitted_index"] = index
        if omitted_in_view:
            out["omitted_in_view_elements"] = omitted_in_view
            out["omitted_note"] = (f"{omitted_in_view} of the left-out elements are on screen; browser.find with "
                                   "their words returns their ids, and browser.read pages the full snapshot")
        out["total_text_chars"] = len(snap.text)
        out["next"] = "browser.read / ferb_browser.read with this tab and offset=0 retrieves the full saved snapshot as paged JSON"
        if len(text) < len(snap.text):
            out["text"] += "\n[Text preview ends; fetch the full snapshot with browser.read]"
    if snap.dialogs:
        out["dialogs"] = snap.dialogs
    return out
