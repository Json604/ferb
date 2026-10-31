"""Owns what FERB works out about the owner from their Mac (notes/memory.md decision 16). Must not ask the owner to
approve it, store it as the owner's words, or let it into the core profile before it holds up.

Code gathers the facts (ferb/device/persona.py: installed apps from Info.plist, their use, browsers and profiles); one
model call interprets them (what the owner mostly does, most-used apps, main browser and the main profile or account in
it); code stores each conclusion as an observed memory item resting on a `MAC_PROFILE_KIND` journal row that holds the
gathered facts (redacted), and the summary is shown once as "Here's what I saw: …" (no yes/no, decision 15). A refresh
whose facts changed replaces these items: same key, a new version; a key no longer given ends."""

import hashlib
import json
from typing import Any, Callable

from ferb.core.types import Identity, Task
from ferb.memory.items import KINDS, MAC_PROFILE_KIND, Memory, MemoryRefused

SEEN_PREFIX = "Here's what I saw on your Mac: "
SEEN_SUFFIX = " I saved this as what I observed; tell me if any of it is wrong."
MAC_SCHEMA: dict[str, Any] = {
    "type": "object", "additionalProperties": False, "required": ["summary", "facts"],
    "properties": {
        "summary": {"type": "string"},
        "facts": {"type": "array", "items": {
            "type": "object", "additionalProperties": False, "required": ["kind", "key", "content"],
            "properties": {"kind": {"type": "string", "enum": list(KINDS)}, "key": {"type": "string"},
                           "content": {"type": "string"}}}}}}


async def interpret(make_runtime: Callable[..., Any], facts: dict[str, Any], *, device_id: str = "dev",
                    spans: Any = None, usage: Callable[[str, dict[str, Any]], None] = lambda *a: None) -> dict[str, Any]:
    """One model call (the worker runtime, no tools, typed output) reads the gathered facts and returns the summary and
    the facts it draws."""
    from ferb.core.prompts import load_prompt
    from ferb.memory.calls import learning_call
    system = load_prompt("memory/mac_profile.md")
    task = Task(f"mac-profile-{facts.get('scanned_at', '')}", None, "what the owner does on this Mac", device_id,
                Identity.OWNER, set(), "verifier", "mac-profile", [], None)
    rt = make_runtime("worker", mode="supervisor", tool_names=[], schema=MAC_SCHEMA)
    answer = json.loads(await learning_call(rt, task, system, json.dumps(facts, ensure_ascii=False, indent=1),
                                            "mac profile facts", MAC_SCHEMA, role="mac_profile", spans=spans,
                                            usage=usage))
    if not isinstance(answer, dict):
        raise ValueError("the Mac profile answer is not an object")
    answer["made_by"] = (f"model:{getattr(rt, 'name', 'unknown')}|prompt:memory/mac_profile.md@"
                         f"{hashlib.sha256(system.encode()).hexdigest()[:8]}")
    return answer


def previous(memory: Memory) -> list[Any]:
    """The current observed items that rest on the gathered Mac facts."""
    rows = memory.db.conn.execute(
        "SELECT DISTINCT s.item_id FROM memory_sources s JOIN journal j ON j.id = s.source_id "
        "WHERE s.kind = 'journal' AND j.kind = ?", (MAC_PROFILE_KIND,)).fetchall()
    found = [memory.get(r["item_id"]) for r in rows]
    current = memory.current_ids("observed")
    return [it for it in found if it is not None and it.id in current]


def store(memory: Memory, append: Callable[[str, dict[str, Any]], Any], facts: dict[str, Any],
          answer: dict[str, Any]) -> dict[str, Any]:
    """Save the model's conclusions as observed items about the owner, each resting on the journal row of the facts;
    an earlier conclusion whose key isn't given again ends (the Mac changed). Returns what was saved and the summary as
    shown."""
    entry = append(MAC_PROFILE_KIND, {"facts": facts, "made_by": str(answer.get("made_by", ""))})
    old = previous(memory)
    saved, refused, keys = [], [], set()
    for f in answer.get("facts") or []:
        try:
            it = memory.save(str(f.get("kind", "fact")), "owner", str(f.get("key", "")), str(f.get("content", "")),
                             "observed", [("journal", entry.id)], 0.6, about="what FERB saw on your Mac").item
        except (MemoryRefused, ValueError) as e:
            refused.append(str(e))
            continue
        keys.add((it.kind, it.key))
        saved.append({"id": it.id, "kind": it.kind, "key": it.key, "text": " ".join(it.content.split())})
    for it in old:
        if (it.kind, it.key) not in keys and it.id not in {s["id"] for s in saved}:
            memory.retire(it.id, "the Mac's facts changed")
    summary = " ".join(str(answer.get("summary", "")).split())
    return {"summary": summary, "shown": (SEEN_PREFIX + summary + SEEN_SUFFIX) if summary else "",
            "facts": saved, "refused": refused, "journal_id": entry.id}
