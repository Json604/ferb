"""Typed cards for the notch (step 8e, owner 2026-10-31): each card is built from a record FERB itself keeps (a timer,
a schedule row, a memory item, a tool call's recorded kind, target and page facts, an owner's Yes, the sites list,
the task's artifacts), never from classifying text. Must not read a model's prose to choose a card or fill a field.

The text stays as it was (history, voice, older apps). Each card that stands for one of code's own lines names that
line (`line`, exactly as the text carries it), so an app that draws the card leaves the line out; nothing is parsed.

Card types and their fields (every field but `type` may be absent when FERB has no record of it):
- reminder: a schedule's run or a reminder or timer came due. `title` (its short name, T-1055: never the
  instruction), `kind` (schedule | reminder | timer, from its row), `due` (when it was due), `repeat` (FERB's words for
  the rule), `next_run` (the next run's time), `until` (its recorded end, ORCH-320: the countdown is to it when present,
  else to `next_run`), `schedule_id` or `timer_id`.
- event: a reminder, timer or schedule just set. `kind` (reminder | timer | schedule), `id`, `what` (its short name),
  `when` (its first time),
  `repeat`, `watch` (FERB's words for a watch), `until` (a schedule's recorded end: a countdown), `line`.
- note: a remembered item. `item_id`, `text`, `state` (saved | proposed), `line`.
- payment: a payment-class call that ran under the owner's Yes to a payment. `amount` and `merchant` (as code read them
  on the page; never the worker's numbers), `site`, `card` (ORCH-321: the name of the owner's saved card entry that
  filled the checkout's card number, as the gateway recorded it with the payment; none for a one-time hidden input).
  Never a card digit: FERB's facts carry none.
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
# Calls that did something for the owner (ORCH-317: a scheduled run that did any of these posts, whatever its worker
# reported): a send, a payment, a submitted form, a written, trashed or deleted file, an account-security change
ACTING_KINDS = frozenset({*SEND_KINDS, Kind.PAY, Kind.SUBMIT, Kind.WRITE_FILE, Kind.TRASH, Kind.DELETE_PERMANENT,
                          Kind.ACCOUNT_SECURITY})


def event(kind: str, ident: str, what: str, when: str, line: str, repeat: str | None = None,
          watch: str | None = None, until: str | None = None) -> dict[str, Any]:
    """A reminder or schedule just set, from its stored row."""
    return {"type": "event", "kind": kind, "id": ident, "what": what, "when": when, "line": line,
            **({"repeat": repeat} if repeat else {}), **({"watch": watch} if watch else {}),
            **({"until": until} if until else {})}


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


def acted(spans: SpanStore, tree_id: str) -> bool:
    """Whether a call of the tree that ran without an error was of an acting kind (its own, declared or fact kinds)."""
    return any(_kinds(c) & ACTING_KINDS for c in _calls(spans, tree_id))


def payments(spans: SpanStore, tree_id: str) -> list[dict[str, Any]]:
    """One expense row per payment the owner said Yes to that then ran: payment-class calls covered by a granted
    `floor:pay` confirmation, grouped by that Yes. Amount and merchant are the page's (`ActionFacts.page_amount` /
    `page_merchant`, read by code): the first the payment's calls recorded, i.e. the checkout's; the card is the saved
    entry's name the gateway recorded with them (`card_entry`, ORCH-321)."""
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
                           ("site", _host(facts.get("page_url")) or target.get("site")),
                           ("card", c.get("card_entry"))):
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
