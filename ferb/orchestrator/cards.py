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
- file: a file the task made. `path`, `name`; only a path that exists when the card is drawn (ORCH-576: never from the
  worker's list alone).
"""

import json
from pathlib import Path
from typing import Any
from urllib.parse import urlsplit

from ferb.policy.confirm import site_key
from ferb.policy.rules import Kind, declared_of
from ferb.store.spans import SpanStore, tree_where

PAY_RULE = "floor:pay"                    # confirm.ALWAYS[Kind.PAY]: the owner's Yes to one payment
# Calls that send something to someone as the owner (the gateway's send class without a plain form submit, which is a
# site's form, not a message to a person)
SEND_KINDS = (Kind.SEND, Kind.REPLY, Kind.POST, Kind.MAILBOX_SEND, Kind.CALL)


def event(kind: str, ident: str, what: str, when: str, line: str, repeat: str | None = None,
          watch: str | None = None, until: str | None = None, started: str | None = None) -> dict[str, Any]:
    """A reminder or schedule just set, from its stored row; a timer's carries when it was set (`started`, APP-762),
    so its ring drains over its duration."""
    return {"type": "event", "kind": kind, "id": ident, "what": what, "when": when, "line": line,
            **({"repeat": repeat} if repeat else {}), **({"watch": watch} if watch else {}),
            **({"until": until} if until else {}), **({"started": started} if started else {})}


def note(item_id: str, text: str, state: str, line: str) -> dict[str, Any]:
    """A remembered item, from the memory store's row."""
    return {"type": "note", "item_id": item_id, "text": text, "state": state, "line": line}


def links(sites: list[str], line: str) -> list[dict[str, Any]]:
    """The sites FERB's records say the task contacted (`sites_contacted`), one card each."""
    return [{"type": "link", "host": host, "line": line} for host in sites]


def files(paths: list[str]) -> list[dict[str, Any]]:
    """The task's artifacts (the last attempt's, as the outcome lists them), each only when the path exists now: the
    worker's list alone never makes a file card (ORCH-576)."""
    return [{"type": "file", "path": p, "name": Path(p).name} for p in paths if p and _exists(p)]


def _exists(path: str) -> bool:
    try:
        return Path(path).expanduser().exists()
    except (OSError, ValueError):
        return False


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
        output = json.loads(row["output_json"] or "{}")
        output = output if isinstance(output, dict) else {}
        after = output.get("after") if isinstance(output.get("after"), dict) else {}
        # HAND-487: the acting call's own read-back saw the page change after it (a structural fact, not its words)
        out.append(data | {"observed_change": output.get("observed_change") is True or after.get("changed") is True})
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


def _yes_of(call: dict[str, Any]) -> list[str]:
    """The confirmations a call carries: those asked for it and the payment approval that covered it."""
    return [str(i) for i in [*(call.get("confirmations") or []), (call.get("payment") or {}).get("covered_by")] if i]


def _pay_granted(spans: SpanStore, calls: list[dict[str, Any]]) -> set[str]:
    """Which of these calls' confirmations are the owner's granted Yes to a payment."""
    ids = sorted({i for c in calls for i in _yes_of(c)})
    if not ids:
        return set()
    return {row["id"] for row in spans.db.conn.execute(
        f"SELECT id FROM confirmations WHERE rule = ? AND granted = 1 AND id IN ({','.join('?' * len(ids))})",
        (PAY_RULE, *ids))}


def purchase(call: dict[str, Any]) -> tuple[str, str]:
    """ORCH-629: the purchase a payment-class call belongs to: its site (as the gateway checked it, else the page's
    host) and its tab chain (one checkout and the tabs it opened)."""
    facts = call.get("facts") or {}
    site = site_key(str((call.get("target") or {}).get("site") or _host(facts.get("page_url"))))
    return site, str(facts.get("tab_chain") or (call.get("payment") or {}).get("scope") or "")


def withdrawn(spans: SpanStore, tree_id: str) -> set[tuple[str, str]]:
    """ORCH-629: the purchases whose last payment-class call FERB asked the owner about and didn't get their Yes for
    (declined on its card, set aside by their words, or never answered), with nothing of that purchase going ahead
    after it: the owner's last word on that purchase stands, so an earlier step they approved (Buy Now) is no payment.
    From the recorded calls only: a call that went ahead, or one refused after FERB asked (`declined`, or carrying the
    question's confirmation); a call refused before any question (a boundary) says nothing about it."""
    where, params = tree_where(tree_id)
    rows = spans.db.conn.execute("SELECT input_json FROM spans WHERE kind = 'tool.call' AND ended_at IS NOT NULL "
                                 f"AND {where} ORDER BY started_at, rowid", params).fetchall()
    last: dict[tuple[str, str], dict[str, Any]] = {}
    for row in rows:
        data = json.loads(row["input_json"] or "{}")
        if data.get("ferb_question") or Kind.PAY not in _kinds(data):
            continue
        if data.get("boundary_refused") and not (data.get("declined") or data.get("confirmations")):
            continue
        last[purchase(data)] = data
    return {key for key, call in last.items() if call.get("boundary_refused")}


def payments(spans: SpanStore, tree_id: str) -> list[dict[str, Any]]:
    """One expense row per purchase the owner said Yes to that then ran: payment-class calls covered by a granted
    `floor:pay` confirmation, grouped by that Yes; of a purchase's Yeses (ORCH-629: its site and tab chain, e.g. Buy Now
    then Place your order), the last one's row, and none when the owner's last word on it was no (`withdrawn`).
    Amount and merchant are the page's (`ActionFacts.page_amount` / `page_merchant`, read by code): the first the
    payment's calls recorded, i.e. the checkout's; the card is the saved entry's name the gateway recorded with them
    (`card_entry`, ORCH-321)."""
    calls = [c for c in _calls(spans, tree_id) if Kind.PAY in _kinds(c)]
    granted = _pay_granted(spans, calls)
    if not granted:
        return []
    gone = withdrawn(spans, tree_id)
    by: dict[str, dict[str, Any]] = {}
    of: dict[str, tuple[str, str]] = {}                  # each Yes's purchase: its first call's
    for c in calls:
        yes = next((i for i in _yes_of(c) if i in granted), None)
        if yes is None:
            continue
        facts, target = c.get("facts") or {}, c.get("target") or {}
        of.setdefault(yes, purchase(c))
        card = by.setdefault(yes, {"type": "payment"})
        for key, value in (("amount", facts.get("page_amount")), ("merchant", facts.get("page_merchant")),
                           ("site", _host(facts.get("page_url")) or target.get("site")),
                           ("card", c.get("card_entry"))):
            if value and key not in card:
                card[key] = str(value)
    final = {key: yes for yes, key in of.items()}        # each purchase's last Yes (the calls are in order)
    return [card for yes, card in by.items() if of[yes] not in gone and final[of[yes]] == yes]

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


def ran(spans: SpanStore, tree_id: str, *kinds: str) -> list[dict[str, Any]]:
    """The tree's calls of any of these kinds (their own, declared or fact kinds) that ran without an error: what
    ORCH-576's action checks read (no card is drawn from them, owner 2026-11-10)."""
    return [c for c in _calls(spans, tree_id) if _kinds(c) & set(kinds)]


def tried(spans: SpanStore, tree_id: str, *kinds: str) -> list[dict[str, Any]]:
    """ORCH-613: the tree's calls of any of these kinds that went ahead (ran, an error included) or that the owner
    declined on their own card (the gateway's `declined` on the refused call: their No, Don't pay or Skip, or their
    words in the card's box), in the order they were made. A call refused for any other reason (nobody there, a
    boundary) is neither. The latest one says whether the owner's decision stands for that action."""
    where, params = tree_where(tree_id)
    rows = spans.db.conn.execute("SELECT input_json FROM spans WHERE kind = 'tool.call' AND ended_at IS NOT NULL "
                                 f"AND {where} ORDER BY started_at, rowid", params).fetchall()
    out = []
    for row in rows:
        data = json.loads(row["input_json"] or "{}")
        if data.get("ferb_question") or (data.get("boundary_refused") and not data.get("declined")):
            continue
        if _kinds(data) & set(kinds):
            out.append(data)
    return out


def approved(spans: SpanStore, call: dict[str, Any]) -> bool:
    """Whether the gateway asked the owner about this call and they said Yes: a confirmation it carries is granted."""
    ids = sorted({str(i) for i in call.get("confirmations") or [] if i})
    return bool(ids) and spans.db.conn.execute(
        f"SELECT 1 FROM confirmations WHERE granted = 1 AND id IN ({','.join('?' * len(ids))}) LIMIT 1",
        ids).fetchone() is not None


def structural_kinds(call: dict[str, Any]) -> set[str]:
    """The kinds the gateway itself recorded for a call (its own and those its structural facts add), without the
    worker's declaration."""
    return {str(call.get("kind") or "")} | {str(k) for k in call.get("fact_kinds") or []}


def paid(spans: SpanStore, tree_id: str) -> list[dict[str, Any]]:
    """The payment-class calls that ran under a granted Yes to a payment (as `payments` counts them), leaving out a
    purchase the owner's last word was no to (ORCH-629, `withdrawn`)."""
    calls = ran(spans, tree_id, Kind.PAY)
    granted = _pay_granted(spans, calls)
    if not granted:
        return []
    gone = withdrawn(spans, tree_id)
    return [c for c in calls if purchase(c) not in gone and any(i in granted for i in _yes_of(c))]


def downloaded(spans: SpanStore, tree_id: str) -> list[str]:
    """The paths of the files the tree's browser tabs downloaded and FERB took (`downloads` rows), in order."""
    where, params = tree_where(tree_id)
    return [str(row["path"]) for row in spans.db.conn.execute(
        f"SELECT path FROM downloads WHERE state = 'taken' AND path IS NOT NULL AND {where} ORDER BY at, rowid",
        params).fetchall()]


def target_path(call: dict[str, Any]) -> str:
    return str((call.get("target") or {}).get("path") or "")
