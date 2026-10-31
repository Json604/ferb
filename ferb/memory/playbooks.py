"""Playbooks (notes/memory.md Tier 3, step 9b): verified how-notes of the same kind of work merged into one note, and
the measure of "gets better at repeated tasks". Must not let a model choose which tasks it sees, rewrite the owner's own
edit of a playbook, or count a run that wasn't verified.

Code finds the candidates: verified episode cards with a how-note whose goals are near each other by the local
embedding model (PLAYBOOK_SIMILARITY: a structural similarity of their recorded goals, never a word list). One model
call (playbook.md, typed output, no tools) says whether they are the same kind of work and writes the merged steps;
code checks every id it names. A playbook is a learned node (`playbook`), retrieved by the one search like an episode,
owner-visible (memory.learned, the memory page) and editable (`memory.learning` op edit makes the owner's version,
which no later merge replaces). A new verified run of the same kind rewrites it. A group the model said is not one kind
is not asked again.

`repeat_stats` measures each playbook's runs in order: attempts, seconds, verified, and whether the run was shown the
playbook or an earlier run's how-note (its context exposures cite them)."""
from __future__ import annotations

import hashlib
import json
from typing import Any, Callable

from ferb.core import origin
from ferb.core.types import Identity, Task

# Ruling (step 9b, measured on potion-base-8M): the goal-to-goal cosine at which two verified tasks are shown to the
# model as possibly the same kind of work. Same-kind goal pairs scored 0.54-0.75 (two visa-slot searches, two cold-mail
# runs), unrelated ones -0.01-0.15; a near miss (checking vs submitting on Kaggle, 0.60) is the model's to turn down
PLAYBOOK_SIMILARITY = 0.5
MERGES_PER_PASS = 3                 # Ruling: model calls per hourly pass; the rest wait for the next pass
GROUP_MOST = 6                      # the most runs one call is shown (the newest)
MADE_BY = "model:playbook/1"
DECLINED_KIND = "memory.playbook_declined"
SCHEMA: dict[str, Any] = {
    "type": "object", "additionalProperties": False, "required": ["same_kind", "members", "title", "text"],
    "properties": {"same_kind": {"type": "boolean"}, "members": {"type": "array", "items": {"type": "string"}},
                   "title": {"type": "string"}, "text": {"type": "string"}}}


def attempts(steps: list[dict[str, Any]]) -> int:
    """A run's attempts: its worker steps as the episode records them (a retry starts another)."""
    return sum(1 for s in steps if s.get("role") == "Worker") or 1


def _group_key(task_ids: list[str]) -> str:
    return hashlib.sha256(json.dumps(sorted(task_ids)).encode()).hexdigest()[:16]


def verified_cards(learning: Any) -> list[dict[str, Any]]:
    """Current episode cards of verified runs that have a how-note (the worker's note or recorded steps)."""
    from ferb.memory.learning import how_note
    out = []
    for node in learning.items("episode", sources=False):
        body = node["body"]
        if body.get("verified") is True and str(body.get("intent") or "").strip() and how_note(body):
            out.append(node)
    return out


def playbooks(learning: Any) -> list[dict[str, Any]]:
    return list(learning.items("playbook", sources=False))


def _goal(node: dict[str, Any]) -> str:
    body = node["body"]
    return " ".join(str(body.get(k) or "") for k in ("intent", "request")).strip()


def candidates(learning: Any, embedder: Any) -> list[tuple[dict[str, Any] | None, list[dict[str, Any]]]]:
    """The merges to ask about: (an existing playbook or None, the verified runs to merge into it). A run already in a
    playbook is never grouped again; a group the model declined is not asked again."""
    from ferb.memory.embeddings import similarity
    cards = verified_cards(learning)
    if embedder is None or len(cards) < 2:
        return []
    vectors = {c["key"]: embedder.encode(_goal(c)) for c in cards}
    by_task = {c["key"]: c for c in cards}
    members: dict[str, dict[str, Any]] = {}
    books = playbooks(learning)
    for book in books:
        for task in book["body"].get("tasks") or []:
            members[str(task)] = book
    declined = {str(json.loads(r[0]).get("group")) for r in learning.db.conn.execute(
        "SELECT data_json FROM journal WHERE kind = ?", (DECLINED_KIND,))}
    jobs: list[tuple[dict[str, Any] | None, list[dict[str, Any]]]] = []
    taken: set[str] = set()

    def near(a: str, b: str) -> bool:
        va, vb = vectors.get(a), vectors.get(b)
        return va is not None and vb is not None and similarity(va, vb) >= PLAYBOOK_SIMILARITY
    for task, card in sorted(by_task.items(), key=lambda kv: str(kv[1]["body"].get("ended_at") or "")):
        if task in members or task in taken:
            continue
        home = next((b for b in books if b["origin"] != origin.OWNER and any(
            near(task, str(m)) for m in b["body"].get("tasks") or [])), None)
        if home is not None:
            group = [card]
            if _group_key([*home["body"].get("tasks", []), task]) not in declined:
                jobs.append((home, group))
                taken.add(task)
            continue
        group = [card] + [other for t, other in by_task.items()
                          if t != task and t not in members and t not in taken and near(task, t)]
        if len(group) >= 2:
            group = sorted(group, key=lambda c: str(c["body"].get("ended_at") or ""))[-GROUP_MOST:]
            if _group_key([c["key"] for c in group]) not in declined:
                jobs.append((None, group))
                taken.update(c["key"] for c in group)
    return jobs


def _run_line(db: Any, card: dict[str, Any]) -> str:
    from ferb.memory.learning import how_note
    body = card["body"]
    row = db.conn.execute("SELECT seconds, steps_json FROM episodes WHERE task_id = ?", (card["key"],)).fetchone()
    tries = attempts(json.loads(row[1])) if row else 1
    seconds = round(float(row[0] or 0)) if row else 0
    return (f"- task {card['key']} ({str(body.get('ended_at') or '')[:10]}; {tries} attempt(s), {seconds} s)\n"
            f"  request: {' '.join(str(body.get('request') or '').split())[:400]}\n"
            f"  goal: {' '.join(str(body.get('intent') or '').split())}\n  how-note: {how_note(body)}")


def context(db: Any, book: dict[str, Any] | None, group: list[dict[str, Any]]) -> str:
    lines = ["Verified past tasks:", *[_run_line(db, c) for c in group]]
    if book is not None:
        lines += ["", f"The playbook as it stands (from tasks {', '.join(book['body'].get('tasks') or [])}):",
                  f"title: {book['body'].get('title')}", str(book["body"].get("text") or "")]
    return "\n".join(lines)


def apply(learning: Any, book: dict[str, Any] | None, group: list[dict[str, Any]], answer: dict[str, Any],
          record: Callable[[str, dict[str, Any]], Any]) -> dict[str, Any] | None:
    """The model's answer, checked by code: members only from the runs it was shown; a new playbook needs two."""
    shown = {c["key"] for c in group}
    named = [str(t) for t in answer.get("members") or [] if str(t) in shown]
    old = [str(t) for t in (book["body"].get("tasks") or [])] if book is not None else []
    tasks = list(dict.fromkeys(old + named))
    if not answer.get("same_kind") or not named or len(tasks) < 2 or not str(answer.get("text") or "").strip():
        record(DECLINED_KIND, {"group": _group_key(old + sorted(shown)), "tasks": sorted(shown)})
        return None
    body = {"title": " ".join(str(answer.get("title") or "").split())[:120] or "Repeated work",
            "text": str(answer["text"]).strip()[:2000], "tasks": tasks}
    key = book["key"] if book is not None else "pb-" + _group_key(tasks)
    node: dict[str, Any] = learning.put("playbook", key, body, ["episode:" + t for t in tasks], made_by=MADE_BY,
                                        origin_label=origin.INFERRED, state="active")
    record("memory.playbook", {"id": node["id"], "key": key, "tasks": tasks, "rewritten": book is not None})
    return node


async def merge(learning: Any, make_runtime: Callable[..., Any], embedder: Any, *, device_id: str = "dev",
                spans: Any = None, record: Callable[[str, dict[str, Any]], Any] = lambda k, d: None,
                usage: Callable[[str, dict[str, Any]], None] = lambda *a: None) -> list[str]:
    """One hourly pass's merges: at most MERGES_PER_PASS model calls. Returns the playbooks written."""
    from ferb.core.prompts import load_prompt
    from ferb.memory.calls import learning_call
    written: list[str] = []
    jobs = candidates(learning, embedder)[:MERGES_PER_PASS]
    if not jobs:
        return written
    system = load_prompt("memory/playbook.md")
    for book, group in jobs:
        ident = f"playbook-{_group_key([c['key'] for c in group])}"
        task = Task(ident, None, "playbook merge", device_id, Identity.OWNER, set(), "verifier", ident, [], None)
        rt = make_runtime("worker", mode="supervisor", tool_names=[], schema=SCHEMA)
        answer = json.loads(await learning_call(rt, task, system, context(learning.db, book, group),
                                                "playbook merge", SCHEMA, role="playbook", spans=spans, usage=usage))
        node = apply(learning, book, group, answer, record) if isinstance(answer, dict) else None
        if node is not None:
            written.append(node["id"])
    return written


def repeat_stats(conn: Any) -> list[dict[str, Any]]:
    """"Gets better at repeated tasks", measured (fix list): for each current playbook, its runs oldest first with
    attempts, seconds and verified, whether each later run was shown the playbook or an earlier run's how-note
    (`used_memory`; None once its trace aged out), and first-to-latest change. Read-only; works on a plain sqlite3
    connection (`ferb stats`) as on the daemon's."""
    from ferb.store.spans import tree_where
    out = []
    for nid, body_json in conn.execute("SELECT id, body_json FROM learning_nodes WHERE kind = 'playbook' AND valid_to IS "
                                       "NULL AND state NOT IN ('retired','rolled_back') ORDER BY recorded_at").fetchall():
        body = json.loads(body_json)
        tasks = [str(t) for t in body.get("tasks") or []]
        runs = []
        for task in tasks:
            row = conn.execute("SELECT started_at, seconds, steps_json, verified FROM episodes WHERE task_id = ?",
                               (task,)).fetchone()
            if row is None:
                continue
            runs.append({"task_id": task, "started_at": row[0], "attempts": attempts(json.loads(row[2] or "[]")),
                         "seconds": round(float(row[1] or 0), 1), "verified": bool(row[3])})
        runs.sort(key=lambda r: str(r["started_at"] or ""))
        earlier_cards: list[str] = []
        for n, run in enumerate(runs):
            where, params = tree_where(run["task_id"])
            traced = conn.execute(f"SELECT 1 FROM spans WHERE {where} LIMIT 1", params).fetchone() is not None
            refs = [nid] + earlier_cards
            run["used_memory"] = None if not traced or n == 0 else any(conn.execute(
                f"SELECT 1 FROM spans WHERE {where} AND kind IN ('context.exposure','context.source') "
                f"AND instr(input_json, ?) > 0 LIMIT 1", (*params, ref)).fetchone() for ref in refs)
            earlier_cards += [str(r[0]) for r in conn.execute(
                "SELECT id FROM learning_nodes WHERE kind = 'episode' AND key = ?", (run["task_id"],))]
        entry: dict[str, Any] = {"id": nid, "title": body.get("title"), "runs": runs}
        if len(runs) >= 2:
            entry["change"] = {"attempts": runs[-1]["attempts"] - runs[0]["attempts"],
                               "seconds": round(runs[-1]["seconds"] - runs[0]["seconds"], 1)}
        out.append(entry)
    return out
