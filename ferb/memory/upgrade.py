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

from ferb.memory.items import Memory, MemoryRefused, subject_key

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
    # IMMEDIATE: the write lock before the first read, so a batch the search re-index commits meanwhile (its own
    # connection, started first) is waited for, never a stale read that fails at the first write ("database is locked")
    c.execute("BEGIN IMMEDIATE")
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


FACT_NODES_KIND = "memory.fact_nodes_closed"


def close_fact_nodes(db: Any, now: str, record: Callable[[str, dict[str, Any]], Any]) -> int | None:
    """Step 9b (fix list: "Search memory_items directly; remove sync_facts"): the copies of memory items an older build
    kept as `fact` (and `identity`) learning nodes are closed once; search reads `memory_items` itself. Closed, not
    deleted: a forget still finds and deletes an item's copies (`Learning.drop_fact_index`), and the step-12 clean-up,
    with the owner's OK, removes the rest. Journaled once; a second start finds nothing open."""
    if db.conn.execute("SELECT 1 FROM journal WHERE kind = ? LIMIT 1", (FACT_NODES_KIND,)).fetchone():
        return None
    closed = db.conn.execute("UPDATE learning_nodes SET valid_to = ? WHERE kind IN ('fact', 'identity') "
                             "AND valid_to IS NULL", (now,)).rowcount
    record(FACT_NODES_KIND, {"closed": closed})
    return int(closed)


# Prompts decision 4 (owner, 2026-10-31; notes/prompt-patches.md "Memory items for you to confirm"): the items that
# replace the prompt patches, stored once as the owner's confirmed words. The owner's words join the always-on Tier 1
# profile at once (memory decision 18, `CoreProfile.saved`), except M7 and M8, which the owner placed by search
# (`search_only`, recorded as `SEARCH_ONLY_KIND`); their subject is what they are about. `login`: the https:// place
# whose login record takes the username (L36 §5). `duplicates`: FERB's inferred copies of the item, by id, that end.
OWNER_SEED_KIND = "memory.owner_seed"            # journaled once, after every item: the seed is done
OWNER_CONFIRMED_KIND = "memory.owner_confirmed"  # the owner's confirmation the items rest on (their source)
OWNER_SEED_VERSION = 1
# The owner's store, known by the items the table names (their ids in the live store, 2026-10-31: M5's existing item
# 88bdea14, the inferred duplicates fc0f1e54 and 9f4693bc, the related 4a37c940 and bac03d3b). These are one person's
# words: any other FERB (a friend's install, a fresh store) never gets them.
OWNER_STORE_ITEMS = ("88bdea14092e42da8be4dc38983a4d69", "fc0f1e546b6b40f5a4e2b0d328c56d3a",
                     "9f4693bcaec14c4c93b965117117af89", "4a37c9402cc144b981f808394bcfcdaf",
                     "bac03d3b8b2e48099e67d266978d2c0d")
OWNER_SEED: tuple[dict[str, Any], ...] = (
    {"id": "M1", "kind": "preference", "subject": "owner", "key": "assignments_in_full",
     "about": "assignments, applications and coursework",
     "content": "Do my assignments, applications and coursework in full when I ask; don't add caveats or offer only to "
                "help me work through it. \"DONT MENTION ANYWHERE AI OR REFLECT AI SOLUTION, submit as me\""},
    {"id": "M2", "kind": "fact", "subject": "owner", "key": "usvisascheduling-username",
     "about": "usvisascheduling.com login", "content": "the username is Kartikey Attri",
     "login": ("https://www.usvisascheduling.com", "Kartikey Attri")},
    {"id": "M3", "kind": "preference", "subject": "owner", "key": "install_named_tool", "about": "a tool I name",
     "content": "When I name a tool that isn't installed, ask me once whether to install it, showing the install "
                "command; on yes install it and carry on, on no stop and tell me. Never switch to a different tool."},
    {"id": "M4", "kind": "preference", "subject": "owner", "key": "scaler_chrome_profile", "about": "Scaler Gmail",
     "content": "My Scaler Gmail is in my Chrome “scaler” profile; open Scaler links there, not in anything else.",
     "duplicates": ("9f4693bcaec14c4c93b965117117af89",)},
    {"id": "M5", "kind": "preference", "subject": "owner", "key": "message_call_default_app",
     "about": "\"message\" and \"call\"", "duplicates": ("fc0f1e546b6b40f5a4e2b0d328c56d3a",),
     "content": "When the owner says “message” or “call,” they mean WhatsApp unless they explicitly mention another "
                "app."},
    {"id": "M6", "kind": "preference", "subject": "owner", "key": "codex_astra", "about": "Codex Astra",
     "content": "“Codex Astra” / “astra high” means the Codex CLI running the Astra model (high reasoning): a setting, "
                "not a project."},
    {"id": "M7", "kind": "fact", "subject": "project:scaler", "key": "scaler_groups", "about": "Scaler groups",
     "search_only": True,
     "content": "cedat is not a scaler group. other scaler groups are names from SST too\ncheck all group names "
                "containing SST"},
    {"id": "M8", "kind": "preference", "subject": "project:study-material", "key": "study_material_quality",
     "about": "study material", "search_only": True,
     "content": "make sure that it is easy to understand, covered the syllabus and also i can actually study and pass "
                "with good numbers"},
    {"id": "M9", "kind": "fact", "subject": "owner", "key": "kaggle_profile", "about": "Kaggle",
     "content": "the kaggle profile is in my brave"},
    {"id": "email", "kind": "fact", "subject": "owner", "key": "email", "about": "\"email me\"",
     "content": "“email me” means kartikey060105@gmail.com"},
)


def seed_owner_items(memory: Memory, record: Callable[[str, dict[str, Any]], Any],
                     set_username: Callable[[str, str, str], None] | None = None) -> dict[str, Any] | None:
    """Store `OWNER_SEED` once (step 10b) in the owner's store only (`OWNER_STORE_ITEMS`), the live store being never
    written by hand: each item as the owner's words (owner_stated), resting on one journaled confirmation record,
    unless a current item already says the same (any key); FERB's understanding of the same fact (same kind, subject and key: the table's inferred duplicates, e.g. the
    "messages" one) ends, kept as history, and so do the inferred duplicates the table names by id (`duplicates`,
    whatever their key; `retire_named_duplicates`, also on a store the seed already ran on); M2's username goes to its
    site's login record when that has none. Journaled once (`OWNER_SEED_KIND`) after the last item, so an interrupted
    seed runs again and skips what it stored."""
    db = memory.db
    if db.conn.execute("SELECT 1 FROM journal WHERE kind = ? AND json_extract(data_json, '$.version') >= ?",
                       (OWNER_SEED_KIND, OWNER_SEED_VERSION)).fetchone() is not None:
        _search_only(memory, record)
        retire_named_duplicates(memory, record)
        return None
    marks = ",".join("?" * len(OWNER_STORE_ITEMS))
    if db.conn.execute(f"SELECT 1 FROM memory_items WHERE id IN ({marks}) LIMIT 1",
                       OWNER_STORE_ITEMS).fetchone() is None:
        return None                                  # not the owner's store
    _search_only(memory, record)                     # before the items: the profile never takes M7 or M8
    confirmed = db.conn.execute("SELECT id FROM journal WHERE kind = ? ORDER BY seq DESC LIMIT 1",
                                (OWNER_CONFIRMED_KIND,)).fetchone()
    source = str(confirmed["id"]) if confirmed is not None else str(record(OWNER_CONFIRMED_KIND, {
        "decision": "prompts decision 4", "confirmed_on": "2026-10-31",
        "items": [item["id"] for item in OWNER_SEED]}).id)
    current = {(" ".join(it.content.split())).lower(): it for it in memory.current_items()}
    stored, skipped, ended, logins, refused = [], [], [], [], []
    for item in OWNER_SEED:
        same = current.get(" ".join(str(item["content"]).split()).lower())
        if same is not None:
            skipped.append(item["id"])
            kept = same
        else:
            try:
                kept = memory.save(item["kind"], item["subject"], item["key"], item["content"], "owner_stated",
                                   [("journal", source)], 1.0, about=item["about"]).item
            except MemoryRefused as e:               # journaled with the rest; the other items still go in
                refused.append({"item": item["id"], "why": str(e)})
                continue
            stored.append({"item": item["id"], "id": kept.id})
        for other in memory.current_items():         # FERB's understanding of the same fact gives way to their words
            if other.id != kept.id and other.origin == "inferred" and \
                    (other.kind, other.subject, other.key) == (kept.kind, kept.subject, kept.key):
                memory.retire(other.id, "the owner confirmed their own words for it")
                ended.append(other.id)
        ended += _retire_ids(memory, item.get("duplicates", ()))
        if "login" in item and set_username is not None:
            place, name = item["login"]
            try:
                set_username(place, name, "memory:" + kept.id)
                logins.append(place)
            except Exception as e:                   # the memory stands either way; the record is the owner's to fix
                logins.append(f"{place}: not saved ({type(e).__name__})")
    out = {"version": OWNER_SEED_VERSION, "stored": stored, "skipped": skipped, "ended": ended, "logins": logins,
           "refused": refused}
    record(OWNER_SEED_KIND, out)
    return out


def _retire_ids(memory: Memory, ids: Any) -> list[str]:
    """Those of `ids` that are current inferred items, retired as the owner's confirmed words replace them (kept as
    history); the ids retired. Any other item, or one already ended, is left as it is."""
    ended = []
    for ident in ids:
        it = memory.get(ident)
        if it is None or it.origin != "inferred" or it.status != "active" or it.valid_to is not None:
            continue
        try:
            memory.retire(ident, "the owner confirmed their own words for it")
        except MemoryRefused:                           # no longer current: nothing to end
            continue
        ended.append(ident)
    return ended


def retire_named_duplicates(memory: Memory, record: Callable[[str, dict[str, Any]], Any]) -> list[str]:
    """The inferred duplicates `OWNER_SEED` names by id (the owner's store only: these ids exist nowhere else), retired
    on a store the seed already ran on (a duplicate whose key differed from its seeded item stayed current there).
    Idempotent: once retired, nothing is left to do; journaled only when something ended."""
    ended = _retire_ids(memory, [i for item in OWNER_SEED for i in item.get("duplicates", ())])
    if ended:
        record(OWNER_SEED_KIND + "_duplicates", {"ended": ended})
    return ended


def _search_only(memory: Memory, record: Callable[[str, dict[str, Any]], Any]) -> None:
    """The facts the owner placed by search (prompts decision 4: M7, M8), recorded once for the core profile, also on a
    store the seed ran on before this record existed."""
    from ferb.memory.profile import SEARCH_ONLY_KIND
    if memory.db.conn.execute("SELECT 1 FROM journal WHERE kind = ? LIMIT 1", (SEARCH_ONLY_KIND,)).fetchone() is None:
        from ferb.memory.items import slug
        record(SEARCH_ONLY_KIND, {"keys": [[subject_key(item["subject"]), slug(item["key"])] for item in OWNER_SEED
                                           if item.get("search_only")], "decision": "prompts decision 4"})
