"""Upgrade (STORE-403) and the one-time memory clean-up before the core profile is first built
(kb area memory), on a database at schema 52 shaped like the owner's live store (read 2026-10-31): four
duplicate same-key pairs (the owner's words and FERB's copy, both current), the old setup step's app-use dump stored as
the owner's words, an unnormalized project subject, a guess still waiting for its Keep with its question, identity
nodes of the old Layer 1. The first start of this build cleans it, journals it once and seeds Tier 1; a second start
changes nothing."""
import shutil
from pathlib import Path
from typing import Any

from ferb.store import db as dbmod
from ferb.store.db import Database
from tests.support.schedules import own_daemon as _own_daemon

AT = "2026-10-24T05:44:31.624567+05:30"
PERSONA_DUMP = ("Apps you use: Brave Browser (10652 recorded uses), Code (5162 recorded uses), Google Chrome (2022 "
                "recorded uses), WhatsApp (1419 recorded uses)")
PAIRS = [("preference", "owner", "message_call_default_app",
          "When the owner says “message” or “call,” they mean WhatsApp unless they explicitly mention another app."),
         ("preference", "owner", "word_meaning_only",
          "When the owner inputs a single word, they want only its meaning and nothing else."),
         ("project", "project:ferb", "owner_built_ferb",
          "You built FERB exactly as this personal Mac agent and plan to keep improving it."),
         ("preference", "owner", "communication_critical_not_yes_man",
          "You prefer FERB to think critically while talking to you and not just agree with you.")]


def _old_store(home: Path) -> None:
    """A database migrated to 52 (the build before step 9a) with the owner's shapes in it."""
    home.mkdir(parents=True)
    old = home.parent / "migrations-52"
    old.mkdir()
    for f in dbmod.MIGRATIONS.glob("[0-9][0-9][0-9][0-9]_*.sql"):
        if int(f.name[:4]) <= 52:
            shutil.copy(f, old / f.name)
    db = Database(home / "ferb.db")
    db.migrate(old)
    c = db.conn
    seq = 0

    def item(kind: str, subject: str, key: str, content: str, origin: str, status: str = "active") -> str:
        nonlocal seq
        seq += 1
        ident, src = f"m{seq:03d}", f"j{seq:03d}"
        c.execute("INSERT INTO journal (id, at, kind, data_json, origin) VALUES (?, ?, 'memory.added', ?, 'owner')",
                  (src, AT, '{"content": "x"}'))
        c.execute("INSERT INTO memory_items (id, kind, subject, key, content, origin, status, confidence, observed_at, "
                  "valid_from, created_at) VALUES (?, ?, ?, ?, ?, ?, ?, 1.0, ?, ?, ?)",
                  (ident, kind, subject, key, content, origin, status, AT, AT, AT))
        c.execute("INSERT INTO memory_sources (item_id, kind, source_id, day) VALUES (?, 'journal', ?, '2026-10-24')",
                  (ident, src))
        return ident
    for kind, subject, key, content in PAIRS:
        item(kind, subject, key, content, "owner_stated")
        item(kind, subject, key, content, "inferred")
    item("preference", "owner", "parallel_task_execution",
                "For independent work, the owner prefers multiple agents in parallel.", "inferred")   # their Keep
    item("preference", "owner", "setup_persona", PERSONA_DUMP, "owner_stated")
    item("project", "project:us-work-permit", "appointment_preferences",
         "Any date range man, just book. BUT notify me befire booking", "owner_stated")
    guess = item("fact", "owner", "piano_skill_level", "The owner is a beginner piano player.", "inferred", "proposed")
    c.execute("INSERT INTO memory_questions VALUES ('ap1', 'confirm', ?, 'Keep this?', 'memory-confirm', ?)",
              (guess, AT))
    c.execute("INSERT INTO learning_nodes (id, kind, key, body_json, origin, state, made_by, recorded_at, valid_from) "
              "VALUES ('n1', 'identity', 'word_meaning_only', '{\"text\": \"x\"}', 'owner', 'active', 'owner:x', ?, ?)",
              (AT, AT))
    db.close()


# decision: MEM-12, MEM-18
def test_the_first_start_cleans_memory_once_and_seeds_the_core_profile(home: Path) -> None:
    _old_store(home)
    d = _own_daemon(home)
    try:
        def read() -> dict[str, Any]:
            dd = d.daemon
            current = dd.memory.items("active")
            [cleanup] = dd._journal().of_kind("memory.cleanup")
            return {"current": current, "cleanup": cleanup.data, "profile": dd.profile.ids(),
                    "questions": dd.db.conn.execute("SELECT COUNT(*) FROM memory_questions").fetchone()[0],
                    "identity": dd.db.conn.execute("SELECT COUNT(*) FROM learning_nodes WHERE kind = 'identity' "
                                                   "AND valid_to IS NULL").fetchone()[0],
                    "text": dd._core_profile_text(), "approvals": dd.state.approvals.pending()}
        got = d.run(_go(read))
        by_key: dict[str, list[Any]] = {}
        for it in got["current"]:
            by_key.setdefault(it.key, []).append(it)
        for _, _, key, _ in PAIRS:                                     # one current version each: the owner's
            assert [i.origin for i in by_key[key]] == ["owner_stated"], key
        assert len(got["cleanup"]["merged"]) == 4
        [persona] = by_key["setup_persona"]
        assert persona.origin == "observed" and persona.id not in got["profile"]   # never in Tier 1
        assert PERSONA_DUMP not in got["text"]
        [permit] = by_key["appointment_preferences"]
        assert permit.subject == "project:us_work_permit"                 # normalized like the key
        [piano] = by_key["piano_skill_level"]
        assert (piano.origin, piano.status) == ("inferred", "active")   # FERB's understanding, no Keep
        assert got["questions"] == 0 and got["approvals"] == [] and got["identity"] == 0
        assert got["profile"] and "WhatsApp unless they explicitly mention" in got["text"]
        assert "Tier 1 — core profile" in got["text"]
        assert "multiple agents in parallel" in got["text"]          # kept under the old Keep: it holds up
    finally:
        d.stop()
    again = _own_daemon(home, ids="again")
    try:
        def twice() -> tuple[int, int, int]:
            dd = again.daemon
            return (len(dd._journal().of_kind("memory.cleanup")), len(dd._journal().of_kind("memory.profile_seeded")),
                    len(dd.memory.items("active")))
        assert again.run(_go(twice)) == (1, 1, len(got["current"]))
    finally:
        again.stop()


async def _go(fn: Any) -> Any:
    return fn()
