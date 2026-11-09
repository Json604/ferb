"""Owns noticing that a page refused a value FERB filled through the sealed path (a password, a login code, a card
detail; GATE-562, owner GATE-563). Facts only: after a sealed fill on a tab and a form submit on that tab, the tab's
next read shows the same page (origin and path) asking for the same field again, empty. Then the page's text that
wasn't there before the fill is its own words for why (quoted as page words, already redacted as page text). No word
of the page decides anything (rule 4). Must not ask the owner or fill anything: the gateway tells the owner and the
hidden-input question reads the count."""

import json
from dataclasses import dataclass, field
from typing import Any

from ferb.policy import urls
from ferb.store.spans import tree_where

# The sealed fills whose value a page can refuse (browser tools: a page shows whether its field asks again).
SEALED_BROWSER = ("browser.secret", "ferb_browser.secret", "browser.login_code", "ferb_browser.login_code")
ASKS_AFTER_REFUSAL = 2           # owner GATE-563 ("Twice"): fresh asks for the same field in a task after a refusal
WORDS_SHOWN = 300                # the page's words quoted: whole, only a wall of text cut (display)


@dataclass
class Refusal:
    """One field's refusals in a task tree: how many times the page asked again, how many fresh asks FERB made since
    the first, and the page's words the last time."""
    what: str                    # "login code", "password", "card detail", "secret" (from the field's kind)
    place: str                   # the page's host
    count: int = 0
    asked_after: int = 0
    words: str = ""
    judged: set[str] = field(default_factory=set)       # fill spans already judged


def field_key(tool: str, kind: str | None) -> str:
    """The field a sealed fill filled, as refusals and questions key it: a login code, else the secret's field kind."""
    return "login_code" if tool.endswith("login_code") else "secret:" + (kind or "")


def what_of(key: str) -> str:
    kind = key.split(":", 1)[1] if ":" in key else ""
    if key == "login_code" or kind == "one_time_code":
        return "login code"
    if kind in ("password", "new_password"):
        return "password"
    if kind.startswith("card_"):
        return "card detail"
    return "secret"


def _page(url: str) -> tuple[str, str]:
    parsed = urls.parse(url or "")
    if parsed is None or not parsed.host:
        return "", ""
    return parsed.host.lower(), (parsed.path or "/").rstrip("/") or "/"


def host_of(url: str) -> str:
    return _page(url)[0]


class RefusedFills:
    """Per task tree, in memory (a restart forgets it: the next refusal starts again)."""

    def __init__(self, spans: Any) -> None:
        self.spans = spans
        self.trees: dict[str, dict[tuple[str, str], Refusal]] = {}

    def forget(self, roots: set[str]) -> None:
        for root in roots:
            self.trees.pop(root, None)

    def get(self, root: str, place: str, key: str) -> Refusal | None:
        return self.trees.get(root, {}).get((place.lower().removeprefix("www."), key))

    def check(self, root: str, tab: str, read: dict[str, Any], before_rowid: int) -> Refusal | None:
        """A new refusal this read of `tab` shows (its result `read`: url, elements, text), else None. `before_rowid`
        is the read's own span (the fill and the submit came before it)."""
        url = read.get("url")
        elements = read.get("elements")
        if not isinstance(url, str) or not isinstance(elements, list):
            return None
        where, params = tree_where(root)
        conn = self.spans.db.conn
        marks = ",".join("?" * len(SEALED_BROWSER))
        fill = conn.execute(
            f"SELECT rowid, id, input_json FROM spans WHERE kind = 'tool.call' AND rowid < ? AND ended_at IS NOT NULL "
            f"AND error IS NULL AND json_extract(output_json, '$.ok') = 1 AND json_extract(input_json, '$.tool') IN "
            f"({marks}) AND json_extract(input_json, '$.args.tab') = ? AND {where} ORDER BY rowid DESC LIMIT 1",
            (before_rowid, *SEALED_BROWSER, tab, *params)).fetchone()
        if fill is None:
            return None
        call = json.loads(fill["input_json"] or "{}")
        facts = call.get("facts") or {}
        kinds = [k for k in facts.get("field_kinds") or [] if isinstance(k, str)]
        key = field_key(str(call.get("tool") or ""), kinds[0] if kinds else None)
        place, path = _page(str(facts.get("page_url") or ""))
        if not place or _page(url) != (place, path):
            return None
        submitted = conn.execute(
            f"SELECT 1 FROM spans WHERE kind = 'tool.call' AND rowid > ? AND rowid < ? AND "
            f"json_extract(input_json, '$.args.tab') = ? AND (json_extract(input_json, '$.facts.submits_form') = 1 "
            f"OR json_extract(output_json, '$.facts.submits_form') = 1 OR "
            f"json_extract(output_json, '$.submitted[0].method') IS NOT NULL) AND {where} LIMIT 1",
            (fill["rowid"], before_rowid, tab, *params)).fetchone()
        if submitted is None:
            return None
        label = str(facts.get("control_label") or "")
        asks_again = any(isinstance(e, dict) and e.get("sensitive") and e.get("filled") is False and
                         ((e.get("field") == kinds[0]) if kinds else (label and e.get("label") == label))
                         for e in elements)
        if not asks_again:
            return None
        tree = self.trees.setdefault(root, {})
        refusal = tree.setdefault((place.removeprefix("www."), key), Refusal(what_of(key), place))
        if fill["id"] in refusal.judged:
            return None
        refusal.judged.add(fill["id"])
        refusal.count += 1
        refusal.words = self._new_words(root, tab, fill["rowid"], str(read.get("text") or ""))
        return refusal

    def _new_words(self, root: str, tab: str, fill_rowid: int, text: str) -> str:
        """The page's lines that weren't on it in the tab's last read before the fill: its own words now."""
        where, params = tree_where(root)
        row = self.spans.db.conn.execute(
            f"SELECT output_json FROM spans WHERE kind = 'tool.call' AND rowid < ? AND "
            f"json_extract(input_json, '$.args.tab') = ? AND json_extract(output_json, '$.elements') IS NOT NULL AND "
            f"{where} ORDER BY rowid DESC LIMIT 1", (fill_rowid, tab, *params)).fetchone()
        before = json.loads(row["output_json"] or "{}").get("text") if row is not None else ""
        seen = {" ".join(line.split()) for line in str(before or "").splitlines()}
        new = [one for line in text.splitlines() if (one := " ".join(line.split())) and one not in seen]
        words = " ".join(new)
        return words if len(words) <= WORDS_SHOWN else words[:WORDS_SHOWN - 1].rstrip() + "…"
