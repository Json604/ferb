"""Typed cards for the notch (step 8e, owner 2026-10-31): each card is built from a record FERB itself keeps (a timer,
a schedule row, a memory item, a tool call's recorded kind, target and page facts, an owner's Yes, the sites list,
the task's artifacts), never from classifying text. Must not read a model's prose to choose a card or fill a field.

The text stays as it was (history, voice, older apps). Each card that stands for one of code's own lines names that
line (`line`, exactly as the text carries it), so an app that draws the card leaves the line out; nothing is parsed.

Card types and their fields (every field but `type` may be absent when FERB has no record of it):
- reminder: a schedule's run came due. `title` (the schedule's recorded request), `due` (when the run was due),
  `repeat` (FERB's words for the rule), `next_run` (the next run's time: a countdown), `schedule_id`.
- event: a reminder or schedule just set. `kind` (reminder | schedule), `id`, `what`, `when` (its first time),
  `repeat`, `watch` (FERB's words for a watch), `line`.
- note: a remembered item. `item_id`, `text`, `state` (saved | proposed), `line`.
- payment: a payment-class call that ran under the owner's Yes to a payment. `amount` and `merchant` (as code read them
  on the page; never the worker's numbers), `site`. No card number: FERB's facts carry none.
- send: a send-class call that ran. `recipient` (the call's checked destination), `kind` (send, reply, post,
  mailbox_send, call), `site` or `app` (a Mac app's bundle id), `account`.
- link: a site the task contacted. `host`, `line` (the "Sites contacted" line).
- file: a file the task made. `path`, `name`."""

import json
from pathlib import Path
from typing import Any
from urllib.parse import urlsplit

from ferb.policy.rules import Kind, declared_of
from ferb.store.spans import SpanStore, tree_where

PAY_RULE = "floor:pay"                    # confirm.ALWAYS[Kind.PAY]: the owner's Yes to one payment
# Calls that send something to someone as the owner (the gateway's send class without a plain form submit, which is a
# site's form, not a message to a person)
SEND_KINDS = (Kind.SEND, Kind.REPLY, Kind.POST, Kind.MAILBOX_SEND, Kind.CALL)


def event(kind: str, ident: str, what: str, when: str, line: str, repeat: str | None = None,
          watch: str | None = None) -> dict[str, Any]:
    """A reminder or schedule just set, from its stored row."""
    return {"type": "event", "kind": kind, "id": ident, "what": what, "when": when, "line": line,
            **({"repeat": repeat} if repeat else {}), **({"watch": watch} if watch else {})}


def note(item_id: str, text: str, state: str, line: str) -> dict[str, Any]:
    """A remembered item, from the memory store's row."""
    return {"type": "note", "item_id": item_id, "text": text, "state": state, "line": line}


def links(sites: list[str], line: str) -> list[dict[str, Any]]:
    """The sites FERB's records say the task contacted (`sites_contacted`), one card each."""
    return [{"type": "link", "host": host, "line": line} for host in sites]


def files(paths: list[str]) -> list[dict[str, Any]]:
    """The task's artifacts (the last attempt's, as the outcome lists them)."""
    return [{"type": "file", "path": p, "name": Path(p).name} for p in paths if p]


def _calls(spans: SpanStore, tree_id: str) -> list[dict[str, Any]]:
    """The tree's tool calls that ran and finished without an error (an unknown outcome is recorded as an error)."""
    where, params = tree_where(tree_id)
    rows = spans.db.conn.execute("SELECT input_json, output_json FROM spans WHERE kind = 'tool.call' AND "
                                 f"ended_at IS NOT NULL AND error IS NULL AND {where} ORDER BY started_at, rowid",
                                 params).fetchall()
    out = []
    for row in rows:
        data = json.loads(row["input_json"] or "{}")
        if data.get("boundary_refused") or data.get("ferb_question"):
            continue
        out.append(data)
    return out


def _kinds(call: dict[str, Any]) -> set[str]:
    """The kinds the gateway recorded for a call: its own, the declared ones, and those its structural facts add."""
    return {str(call.get("kind") or "")} | declared_of(call.get("declared")) | {str(k) for k in call.get("fact_kinds")
                                                                                or []}


def _host(url: Any) -> str:
    try:
        return (urlsplit(str(url)).hostname or "") if url else ""
    except ValueError:
        return ""


def payments(spans: SpanStore, tree_id: str) -> list[dict[str, Any]]:
    """One expense row per payment the owner said Yes to that then ran: payment-class calls covered by a granted
    `floor:pay` confirmation, grouped by that Yes. Amount and merchant are the page's (`ActionFacts.page_amount` /
    `page_merchant`, read by code): the first the payment's calls recorded, i.e. the checkout's."""
    calls = [c for c in _calls(spans, tree_id) if Kind.PAY in _kinds(c)]
    ids = {str(i) for c in calls for i in [*(c.get("confirmations") or []),
                                           (c.get("payment") or {}).get("covered_by")] if i}
    if not ids:
        return []
    granted = {row["id"] for row in spans.db.conn.execute(
        f"SELECT id FROM confirmations WHERE rule = ? AND granted = 1 AND id IN ({','.join('?' * len(ids))})",
        (PAY_RULE, *sorted(ids)))}
    by: dict[str, dict[str, Any]] = {}
    for c in calls:
        yes = next((str(i) for i in [*(c.get("confirmations") or []), (c.get("payment") or {}).get("covered_by")]
                    if i and str(i) in granted), None)
        if yes is None:
            continue
        facts, target = c.get("facts") or {}, c.get("target") or {}
        card = by.setdefault(yes, {"type": "payment"})
        for key, value in (("amount", facts.get("page_amount")), ("merchant", facts.get("page_merchant")),
                           ("site", _host(facts.get("page_url")) or target.get("site"))):
            if value and key not in card:
                card[key] = str(value)
    return list(by.values())


def sends(spans: SpanStore, tree_id: str) -> list[dict[str, Any]]:
    """One contact card per recipient and channel a send-class call reached: the destination the gateway checked for
    that call (its recorded target: recipients, or the contact as the app showed it), never words of a report."""
    out: dict[tuple[str, str], dict[str, Any]] = {}
    for c in _calls(spans, tree_id):
        kinds = _kinds(c) & set(SEND_KINDS)
        if not kinds:
            continue
        target = c.get("target") or {}
        recipients = [str(r) for r in target.get("recipients") or []] + ([str(target["contact"])]
                                                                          if target.get("contact") else [])
        site = str(target.get("site") or "")
        place = {"app": site} if str(c.get("tool") or "").startswith("mac.") and site else (
            {"site": site} if site else {})
        for recipient in recipients or [""]:
            if not recipient and not place:
                continue
            key = (recipient, site)
            if key in out:
                continue
            out[key] = {"type": "send", "kind": sorted(kinds)[0], **({"recipient": recipient} if recipient else {}),
                        **place, **({"account": str(target["account"])} if target.get("account") else {})}
    return list(out.values())
