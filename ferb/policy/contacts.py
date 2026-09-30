"""Never guess between contacts (build step 16a, owner decisions 2026-09-30).

A call or a message sent in an app or web chat names its contact: `name` (as the owner said it), `selected` (the
contact's name as the screen shows it) and `detail` (a number, username or subtitle the screen shows). Code checks:
- the selected contact and its detail were actually shown in this task's snapshots of that app / tab;
- how many different entries on one screen matched the name: more than one, and nothing the owner said picks one
  ("Rahul from work"), means FERB asks the owner "Which Rahul?" with numbered options;
- the owner's answer applies to that one action only. A lasting mapping exists only when the owner states it as a
  memory ("Rahul always means Rahul Sharma"), which then counts as the owner's words.
"""

import re
from collections import deque
from dataclasses import dataclass, field
from typing import Any, Literal

from pydantic import BaseModel, Field

SHOTS_KEPT = 10
EDITABLE = re.compile(r"(text ?field|text ?area|textbox|search|combo ?box|input|editable)", re.I)
STOP = frozenset({"the", "and", "for", "from", "with", "call", "video", "voice", "message", "text", "send", "tell",
                  "whatsapp", "on", "my", "him", "her", "them", "that", "this", "please", "can", "you", "say",
                  "about", "who", "what", "just"})


class Contact(BaseModel):
    """Who a call / message is for. If several contacts match the name, don't ask the owner yourself: pick one and
    press — FERB shows the owner the numbered matches and tells you who they chose."""
    name: str = Field(description="the person as the owner named them, e.g. Rahul")
    selected: str = Field(description="the contact's name exactly as the app / page shows it")
    detail: str = Field(description="a number, username or subtitle the app / page shows for that contact")


def norm(text: str) -> str:
    return " ".join(re.findall(r"[\w+@.]+", text.lower()))


def _words(text: str) -> set[str]:
    return set(re.findall(r"[a-z0-9]+", text.lower()))


def entries(content: dict[str, Any]) -> list[str]:
    """What a snapshot showed: element labels / values (not text fields — what was typed isn't a contact) and the
    visible text, one line each, without duplicates."""
    out: list[str] = []
    seen: set[str] = set()
    items = [" ".join(str(x) for x in (e.get("label"), e.get("value")) if x)
             for e in content.get("elements", []) if not EDITABLE.search(str(e.get("role", "")))]
    shown = [norm(i) for i in items if norm(i)]
    # visible text: a line that already contains an element's text is that element again (a web page's text can be
    # one long line holding every contact), so it doesn't count separately
    items += [line for line in str(content.get("text", "")).splitlines()
              if not any(s in norm(line) for s in shown)]
    for it in items:
        it = it.strip()[:200]
        n = norm(it)
        if n and n not in seen:
            seen.add(n)
            out.append(it)
    return out


def matching(name: str, shot: list[str]) -> list[str]:
    """The different contacts on one screen matching the name. An entry containing another matching entry is the same
    contact (the open chat's header "Rahul Verma" and its row "Rahul Verma · +91 …"); each is shown by its fullest
    label (at most 120 characters)."""
    want = _words(name)
    m = [e for e in shot if want and want <= _words(e)]
    core = [e for e in m if not any(o != e and norm(o) in norm(e) for o in m)]
    out = []
    for e in core:
        holders = [x for x in m if norm(e) in norm(x) and len(x) <= 120
                   and not any(o != e and norm(o) in norm(x) for o in core)]
        out.append(max(holders, key=len, default=e))
    return out


class Seen:
    """This task's recent snapshots, per app or tab (the gateway records every snapshot result)."""

    def __init__(self) -> None:
        self._shots: dict[tuple[str, str], deque[list[str]]] = {}

    def record(self, task_id: str, where: str, content: dict[str, Any]) -> None:
        self._shots.setdefault((task_id, where.lower()), deque(maxlen=SHOTS_KEPT)).append(entries(content))

    def shots(self, task_id: str, where: str) -> list[list[str]]:
        return list(self._shots.get((task_id, where.lower()), ()))

    def forget(self, task_id: str) -> None:
        for k in [k for k in self._shots if k[0] == task_id]:
            del self._shots[k]


@dataclass
class Check:
    decision: Literal["ok", "ask", "refuse"]
    why: str = ""
    options: list[str] = field(default_factory=list)


def _same(selected: str, entry: str) -> bool:
    a, b = norm(selected), norm(entry)
    return bool(a) and (a in b or b in a)


def check(c: Contact, shots: list[list[str]], owner_words: str, chosen: str | None = None) -> Check:
    """`owner_words`: this request's words, the owner's recent messages and their stated memories about the name.
    `chosen`: the owner's answer for this one action, if they were just asked."""
    if not c.selected.strip() or not c.detail.strip():
        return Check("refuse", "say which contact: `selected` and `detail` exactly as the screen shows them")
    shown = [e for s in shots for e in s]
    if not any(_same(c.selected, e) for e in shown):
        return Check("refuse", f"“{c.selected}” wasn't on screen in this task; take a snapshot that shows the contact "
                               "and select it from there")
    if not any(norm(c.detail) in norm(e) for e in shown):
        return Check("refuse", f"the detail “{c.detail}” wasn't on screen; copy it from the snapshot")
    if chosen is not None:
        return Check("ok") if _same(c.selected, chosen) else Check(
            "refuse", f"the owner chose “{chosen}”; select that contact and try again")
    candidates = max((matching(c.name, s) for s in shots), key=len, default=[])
    if len(candidates) == 1:
        if _same(c.selected, candidates[0]):
            return Check("ok")
        return Check("ask", f"Is “{c.selected} ({c.detail})” the {c.name} you meant?",
                     [candidates[0], f"{c.selected} ({c.detail})"])
    if not candidates:                                  # the name isn't on screen: e.g. "Mom" saved under a name
        if _words(c.selected) & (_words(owner_words) - STOP):
            return Check("ok")                          # the owner's words (or stated memory) name this contact
        return Check("ask", f"Is “{c.selected} ({c.detail})” the {c.name} you meant?", [f"{c.selected} ({c.detail})"])
    hints = _words(owner_words) - STOP - _words(c.name)
    hints = {h for h in hints if len(h) >= 3 and not all(h in _words(e) for e in candidates)}
    picked = [e for e in candidates if hints & _words(e)]
    if len(picked) == 1:                                # "Rahul from work": the owner's words pick one
        return Check("ok") if _same(c.selected, picked[0]) else Check(
            "refuse", f"the owner's words point to “{picked[0]}”; select that contact and try again")
    return Check("ask", f"Which {c.name} did you mean?", candidates[:9])


def display(c: Contact) -> str:
    return f"{c.selected} ({c.detail})"
