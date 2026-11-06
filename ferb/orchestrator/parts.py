"""Answers in steps (ORCH-319): one card per multi-part turn, a row per part, each answer posted once.

A part is one of the owner's asks, not a plan step (plans.Part). A part answered directly is said in the first reply;
every other part has a row that fills once, when its answer step's checker accepts it (accept or accept-unconfirmed),
or says it couldn't be done. Posted rows are durable: one `chat.part` journal row per change, keyed by (task, request,
part), with the redacted text, so a part is never posted twice, also across a restart (a resumed task re-sends nothing
already recorded, and the app rebuilds its rows from the record). A later step that finds a posted answer wrong posts
one `updated` row with one line why; nothing posts the same answer again. Live states (asks, paused, working) are
events only. No model writes anything here: code records and counts."""

from collections.abc import Callable, Sequence
from datetime import datetime, timedelta, timezone
from typing import Any

from ferb.memory.journal import Journal
from ferb.orchestrator.plans import Part

KIND = "chat.part"
DONE = ("done", "updated")           # counted as done ("N of M done")
UPDATED_WHY = "Updated"              # the prefix of an updated row's one line why ("Updated: <why>")
WHY_CORRECTION = "done after your correction"     # a failed part a correction in the same run carried through
WHY_FOLLOW_UP = "changed after your follow-up"    # a part a later turn of the task changed


class Card:
    """The rows of one multi-part turn of a task (`task_id`, `request_id`). Rows are keyed by (task, part) across
    turns (a part is the same ask when its id and quote are): a continued run's card carries only the parts that run
    changes, and counts only those (ORCH-543); a part finished in an earlier turn stays as posted in the card that showed it, and changes only through an
    `updated` row. `emit` sends a part event to the task's stream and the bus; `origin` gives the journal origin of a
    model-written row (the task's)."""

    def __init__(self, journal: Journal, task_id: str, request_id: str, parts: list[Part],
                 emit: Callable[[dict[str, Any]], None], origin: Callable[[], str]) -> None:
        self.journal, self.task_id, self.request_id = journal, task_id, request_id
        self.parts, self.emit, self.origin = list(parts), emit, origin
        self._live: dict[str, dict[str, Any]] = {}     # part id -> its latest live (not durable) state
        self._seq = ""
        records = self.records()
        # this card's own rows: every part not already finished in an earlier turn of the task
        self.here = {p.id for p in self.parts if records.get(p.id, {}).get("state") not in DONE}

    # ---------- reading ----------

    def part_of(self, step_id: str) -> Part | None:
        return next((p for p in self.parts if p.answer_step == step_id), None)

    def records(self) -> dict[str, dict[str, Any]]:
        """Each part's latest durable row in any turn of this task (the record the app rebuilds from): rows of the same
        part id and quote; a reused id with other words is another ask."""
        quotes = {p.id: p.quote for p in self.parts}
        latest: dict[str, dict[str, Any]] = {}
        for entry in self.journal.of_kind(KIND, "task_id", self.task_id):
            part = str(entry.data.get("part"))
            if part in quotes and entry.data.get("quote", quotes[part]) == quotes[part]:
                latest[part] = dict(entry.data)
        return latest

    def _shown(self, part: Part, records: dict[str, dict[str, Any]]) -> bool:
        """A row of this card: a part this run started with unfinished, or one this run changed."""
        return part.id in self.here or records.get(part.id, {}).get("request_id") == self.request_id

    def rows(self) -> list[dict[str, Any]]:
        """This card's rows now, in the owner's order: a direct part's answer, a posted one, or its live state (a row
        not posted yet is working)."""
        records = self.records()
        out = []
        for p in self.parts:
            if not self._shown(p, records):
                continue
            row = {"part": p.id, "label": p.label, "state": "working", "direct": p.direct is not None,
                   **({"step": p.answer_step} if p.answer_step else {})}
            posted, live = records.get(p.id), self._live.get(p.id)
            if live is not None and (posted is None or str(live["seq"]) > str(posted.get("seq", ""))):
                row |= {k: v for k, v in live.items() if k not in ("type", "task_id", "request_id")}
            elif posted is not None:
                row |= {k: posted[k] for k in ("state", "text", "note", "why", "seq", "verified",
                                               "card_request_id") if posted.get(k)}
            out.append(row)
        return out

    def tally(self) -> tuple[int, int]:
        """(done, total) over this card's rows (ORCH-319's "N of M done"; ORCH-543: the count is of the rows the card
        shows, so it always agrees with them): a direct part's row counts as done, a failed one doesn't. A continued
        run's card leaves out parts an earlier turn finished, and so does its count; those were done, so whether every
        ask is done reads the same either way."""
        records = self.records()
        shown = [p for p in self.parts if self._shown(p, records)]
        done = sum(1 for p in shown if p.direct is not None or records.get(p.id, {}).get("state") in DONE)
        return done, len(shown)

    def unposted(self) -> list[Part]:
        """This card's answer rows that have neither filled nor failed in this run."""
        records = self.records()
        return [p for p in self.parts if p.direct is None and p.id in self.here
                and records.get(p.id, {}).get("request_id") != self.request_id]

    # ---------- writing ----------

    def _next_seq(self) -> str:
        """Strictly increasing, also across a restart: the wall clock's UTC time to the microsecond (an ISO string, so
        redaction never takes it for a number), bumped past the last one. The app applies a part event only when its
        seq sorts after the row's, so the same event reaching it twice (stream and bus) is applied once."""
        floor = max([self._seq, *(str(r.get("seq", "")) for r in self.records().values())])
        now = datetime.now(timezone.utc)
        while (stamp := now.isoformat(timespec="microseconds")) <= floor:
            now += timedelta(microseconds=1)
        self._seq = stamp
        return stamp

    def say_direct(self) -> list[dict[str, Any]]:
        """The direct parts not said in an earlier turn, recorded once as done (their text is in the first reply,
        never sent again as an event); returned for the plan line, in the owner's order."""
        records = self.records()
        said = []
        for p in self.parts:
            if p.direct is None or p.id not in self.here or records.get(p.id, {}).get("request_id") == self.request_id:
                continue
            self._record(p, "done", p.direct, direct=True)
            said.append({"part": p.id, "label": p.label, "text": p.direct})
        return said

    def settle(self, part: Part, outcome: str, text: str, *, note: str = "", verified: bool = False,
               step_task: str = "") -> bool:
        """A part's answer step ended: done (its checked answer and at most one "Couldn't confirm" line) or failed
        ("Couldn't do: reason"). Posted once: a rerun in the same run (a restart) never posts it again, and a later
        failure never replaces an answer. A changed answer is an `updated` row with one line why: a failed part a
        correction carried through, or a part a follow-up turn changed (the same answer posts nothing). Whether it
        posted."""
        latest = self.records().get(part.id)
        if latest is None:
            return self._post(part, outcome, text, note=note, verified=verified, step_task=step_task)
        same_run, before = latest.get("request_id") == self.request_id, latest.get("state")
        if outcome == "failed":
            if before == "failed" and not same_run:          # still failing in a follow-up: this card says so
                return self._post(part, "failed", text, step_task=step_task)
            return False
        if before in DONE and (same_run or latest.get("text") == text):
            return False
        why = WHY_CORRECTION if same_run else WHY_FOLLOW_UP
        return self._post(part, "updated", text, note=note, why=why, verified=verified, step_task=step_task)

    def update(self, part: Part, text: str, why: str, step_task: str = "") -> bool:
        """A later step found a posted answer wrong or out of date (its `part_updates`, judged by its check): one
        `updated` row, never a new message. Only a posted answer, only when the text changes."""
        latest = self.records().get(part.id)
        if latest is None or latest.get("state") not in DONE or latest.get("text") == text:
            return False
        return self._post(part, "updated", text, why=why, step_task=step_task)

    def _record(self, part: Part, state: str, text: str, **fields: Any) -> dict[str, Any]:
        first = next((e.data.get("request_id") for e in self.journal.of_kind(KIND, "task_id", self.task_id)
                      if e.data.get("part") == part.id and e.data.get("quote", part.quote) == part.quote),
                     self.request_id)
        data = {"task_id": self.task_id, "request_id": self.request_id, "part": part.id, "label": part.label,
                "quote": part.quote, "state": state, "text": text, "seq": self._next_seq(),
                "card_request_id": first, **{k: v for k, v in fields.items() if v not in (None, "", False)}}
        self.journal.append(KIND, data, origin=self.origin())
        return data

    def _post(self, part: Part, state: str, text: str, *, note: str = "", why: str = "",
              verified: bool = False, step_task: str = "") -> bool:
        data = self._record(part, state, text, note=note, why=why, verified=verified,
                            step=part.answer_step, step_task_id=step_task)
        self._live.pop(part.id, None)
        self.emit({"type": "part", **{k: v for k, v in data.items() if k != "quote"}})
        return True

    def show(self, part: Part, state: str, **fields: Any) -> None:
        """A live state of a row that hasn't posted an answer in this run (asks, paused, working again): an event, never
        recorded. A posted answer keeps its row; a row that failed in this run shows its rerun after a correction."""
        posted = self.records().get(part.id, {})
        if posted.get("request_id") == self.request_id and posted.get("state") in DONE:
            return
        event = {"type": "part", "task_id": self.task_id, "request_id": self.request_id, "part": part.id,
                 "label": part.label, "state": state, "seq": self._next_seq(),
                 **({"step": part.answer_step} if part.answer_step else {}),
                 **{k: v for k, v in fields.items() if v not in (None, "", [], {})}}
        self._live[part.id] = event
        self.emit(event)


NOT_FINISHED = "not finished"                     # an index line's part that neither filled nor failed
PARTLY_UNCONFIRMED = "not all of it confirmed"    # an index line's part whose row has a "Couldn't confirm" line
RESULTS = "Results: "                             # an index line naming where the task's files are


def composed(card: Card, lines: list[str], artifacts: Sequence[str] = (), chars: int = 300) -> str:
    """The task's answer as the record keeps it (history, voice, task results, the planner's short form, clients
    without rows): an index of the card, one paragraph (owner D7, ORCH-534). Per part in the owner's order: its label
    and its row's first line (the outcome: a done row's result, a failed row's "Couldn't do: …", a question it asks),
    else "not finished"; then where the results are (`artifacts`), the done count and code's lines. Each line is
    clipped to `chars`. Built by code from the rows: nothing new is said; the full rows stay in the card and in
    `chat.part`."""
    out = []
    for row in card.rows():
        state, text = row.get("state"), _first_line(row.get("text"))
        if state == "asks" and isinstance(row.get("question"), dict):
            text = _first_line(row["question"].get("text"))
        said = text if text and (state in (*DONE, "failed", "asks")) else NOT_FINISHED
        if row.get("note") and state in DONE:
            said += f" ({PARTLY_UNCONFIRMED})"
        out.append(f"{row['label']}: {said}")
    if artifacts:
        out.append(RESULTS + ", ".join(artifacts))
    done, total = card.tally()
    return "\n".join(_clip(line, chars) for line in [*out, done_line(done, total), *lines])


def _first_line(text: Any) -> str:
    return next((line.strip() for line in str(text or "").strip().split("\n") if line.strip()), "")


def _clip(line: str, chars: int) -> str:
    return line if len(line) <= chars else line[:chars - 1].rstrip() + "…"


def done_line(done: int, total: int) -> str:
    return f"{done} of {total} done"
