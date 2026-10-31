"""Owns the one-time memory clean-up before the core profile is first built (notes/memory.md fix list, "Clean memory
before Tier 1 is seeded"; storage decision 5). Must not delete a memory or lose its history.

Runs at the first start of the step-9a build, journaled once (`CLEANUP_KIND`), idempotent (each step finds nothing to do
a second time):
1. FERB's guesses still waiting for a Keep become FERB's understanding (decision 15: no Keep questions); their waiting
   questions are dropped by the daemon (`_restore_memory_questions`). The ones the owner already kept are recorded
   (`owner_kept`): they hold up for the core profile at once.
2. Subjects are normalized like keys (`subject_key`), so "project:us-b1b2-visa" and "project:us_b1b2_visa" are one.
3. The old setup step's app-use dump (`setup_persona`, stored as the owner's words) becomes observed: FERB wrote it,
   and the core profile never takes it.
4. Several current versions of one fact (kind, subject, key) merge into one: the owner's newest words, else the newest;
   the others end now and stay as history (the live store had four such pairs).
5. Identity nodes of the old "Layer 1" (learning_nodes kind 'identity') close: the core profile replaces them.
Dated facts whose time has passed are left to the first weekly profile pass (a model judges "the 16th" said on the
14th); it runs at once after this."""

from typing import Any, Callable

from ferb.memory.items import Memory, subject_key

CLEANUP_KIND = "memory.cleanup"
CLEANUP_VERSION = 1
PERSONA_KEYS = ("setup_persona",)


def clean_before_tier1(memory: Memory, record: Callable[[str, dict[str, Any]], Any]) -> dict[str, Any] | None:
    db = memory.db
    done = db.conn.execute("SELECT 1 FROM journal WHERE kind = ? AND json_extract(data_json, '$.version') >= ?",
                           (CLEANUP_KIND, CLEANUP_VERSION)).fetchone()
    if done is not None:
        return None
    now = memory.clock.now().isoformat()
    c = db.conn
    c.execute("BEGIN")
    try:
        # an older build's active inferred item is one the owner kept (Keep was its only way in): it holds up
        kept = [str(r[0]) for r in c.execute("SELECT id FROM memory_items WHERE origin = 'inferred' AND "
                                            "status = 'active' AND valid_to IS NULL")]
        guesses = c.execute("UPDATE memory_items SET status = 'active' WHERE status = 'proposed' AND origin = 'inferred' "
                            "AND valid_to IS NULL").rowcount
        subjects = 0
        for r in c.execute("SELECT DISTINCT subject FROM memory_items").fetchall():
            norm = subject_key(str(r["subject"]))
            if norm != r["subject"]:
                subjects += c.execute("UPDATE memory_items SET subject = ? WHERE subject = ?",
                                      (norm, r["subject"])).rowcount
        marks = ",".join("?" * len(PERSONA_KEYS))
        persona = c.execute(f"UPDATE memory_items SET origin = 'observed' WHERE key IN ({marks}) "
                            f"AND origin = 'owner_stated'", PERSONA_KEYS).rowcount
        merged: list[dict[str, Any]] = []
        groups = c.execute("SELECT kind, subject, key FROM memory_items WHERE status = 'active' AND valid_to IS NULL "
                           "GROUP BY kind, subject, key HAVING COUNT(*) > 1").fetchall()
        for g in groups:
            rows = c.execute("SELECT id, origin FROM memory_items WHERE kind = ? AND subject = ? AND key = ? "
                             "AND status = 'active' AND valid_to IS NULL ORDER BY origin = 'owner_stated' DESC, seq DESC",
                             (g["kind"], g["subject"], g["key"])).fetchall()
            keeper, others = rows[0]["id"], [r["id"] for r in rows[1:]]
            for other in others:
                c.execute("UPDATE memory_items SET valid_to = ? WHERE id = ?", (now, other))
            c.execute("UPDATE memory_items SET supersedes = COALESCE(supersedes, ?) WHERE id = ?", (others[0], keeper))
            merged.append({"key": g["key"], "kept": keeper, "ended": others})
        identity = c.execute("UPDATE learning_nodes SET valid_to = ? WHERE kind = 'identity' AND valid_to IS NULL",
                             (now,)).rowcount
        c.execute("COMMIT")
    except Exception:
        c.execute("ROLLBACK")
        raise
    out = {"version": CLEANUP_VERSION, "owner_kept": kept, "guesses_kept": guesses, "subjects_normalized": subjects,
           "persona_observed": persona, "merged": merged, "identity_closed": identity}
    record(CLEANUP_KIND, out)
    memory.on_change()
    return out
