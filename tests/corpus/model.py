"""The corpus format: a scenario is what a task read, in order, then one outbound action, with the verdict the owner's
decisions want (`ask` / `no_ask`). Plain data, JSON round-trippable (scenarios.json is the generator's output).

Read kinds (`Read.kind`), in the owner's terms; the harness maps each to how FERB records it today (replay.py):
  owner_words    the owner's typed message (the request's words)
  owner_paste    text the owner pasted into the message (their words since ORCH-518)
  front_window   the window in front of the owner when they sent the message (`where`: the page's URL)
  ferb_fact      a fact FERB's own code wrote (the clock, today's date). `via`: "summary" (it rides in the running
                 summary, inside the conversation) or "facts" (the live facts block)
  private        the owner's own data, nothing outside: a file (`where` "file:/path") or an app ("app:Contacts")
  outside        purely outside content: a web page (`where` an http(s) URL), an email or message from someone else
                 ("email:<id>", "message:<chat>")
  signed_in      a page in the owner's signed-in profile (`where` a URL) or a message in their own mail app
                 ("mail:<id>"): both the owner's data and outside content
  history        FERB's conversation carried from earlier tasks (`carried_from`: outside hosts earlier tasks read;
                 `where`, optional: the private text an earlier task read and FERB's entries were written from)
  memory         a memory fact FERB looked up (`carried_from` as above; empty: a fact FERB observed itself)
  owner_fact     a fact the owner stated about themselves, saved in memory as their words (looked up)
  schedule_run   a schedule's earlier run's answer, shown to this run (`carried_from` as above)
  visit          FERB itself opens `where` (an outbound step earlier in the same task, e.g. a search it was allowed)

`Read.phase` is "make" for what the turn that made a schedule saw, else "run" (the tree that acts).

Action kinds (`Action.kind`): browser_open, browser_navigate (from `start_url`), form_submit (type `text` into a page
on `site`, then submit it to `action_host`), message_send / email_send (`to`, `text`), post (`site`, `text`: a send with
no recipient). `Scenario.schedule` makes the action a schedule's later, unattended run."""
from __future__ import annotations

import json
from dataclasses import asdict, dataclass, field
from typing import Any

READ_KINDS = ("owner_words", "owner_paste", "front_window", "ferb_fact", "private", "outside", "signed_in", "history",
              "memory", "owner_fact", "schedule_run", "visit")
ACTION_KINDS = ("browser_open", "browser_navigate", "form_submit", "message_send", "email_send", "post")
VERDICTS = ("ask", "no_ask")


@dataclass(frozen=True)
class Read:
    kind: str
    text: str = ""
    where: str = ""
    via: str = ""
    carried_from: tuple[str, ...] = ()
    phase: str = "run"


@dataclass(frozen=True)
class Action:
    kind: str
    url: str = ""
    start_url: str = ""
    to: str = ""
    text: str = ""
    site: str = ""
    action_host: str = ""


@dataclass(frozen=True)
class Scenario:
    id: str
    family: str
    domain: str
    description: str
    reads: tuple[Read, ...]
    action: Action
    expected: str
    reason: str
    secret: str = ""                           # the owner's private value at stake, as its source holds it
    schedule: bool = False                     # the action is a schedule's later run
    approved_hosts: tuple[str, ...] = ()       # hosts the owner approved in an earlier task (GATE-205)
    known_today: str = ""                      # why today's code is known to get this wrong (else "")
    tags: tuple[str, ...] = field(default=())

    def check(self) -> None:
        assert self.expected in VERDICTS, self.id
        assert self.action.kind in ACTION_KINDS, self.id
        assert self.reads, self.id
        for r in self.reads:
            assert r.kind in READ_KINDS and r.phase in ("make", "run"), (self.id, r)
            assert r.phase == "run" or self.schedule, (self.id, "make-phase read without a schedule")


def to_json(scenarios: list[Scenario]) -> str:
    """One scenario per line: readable, and a change shows as the lines it touches."""
    rows = (json.dumps(_lean(asdict(s)), ensure_ascii=False, sort_keys=True) for s in scenarios)
    return "[\n" + ",\n".join(rows) + "\n]\n"


def _lean(d: dict[str, Any]) -> dict[str, Any]:
    """Without the fields left at their defaults (empty, false, the run phase): from_json fills them back."""
    out = {k: v for k, v in d.items() if v not in ("", (), [], False) and not (k == "phase" and v == "run")}
    if "reads" in out:
        out["reads"] = [_lean(r) for r in out["reads"]]
    if "action" in out:
        out["action"] = _lean(out["action"])
    return out


def from_json(text: str) -> list[Scenario]:
    out: list[Scenario] = []
    for d in json.loads(text):
        reads = tuple(Read(**(r | {"carried_from": tuple(r.get("carried_from", ()))})) for r in d.pop("reads"))
        action = Action(**d.pop("action"))
        out.append(Scenario(reads=reads, action=action, **(d | {
            "approved_hosts": tuple(d.get("approved_hosts", ())), "tags": tuple(d.get("tags", ()))})))
    return out


def as_dict(s: Scenario) -> dict[str, Any]:
    return asdict(s)
