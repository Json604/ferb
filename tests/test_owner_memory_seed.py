"""Prompts decision 4 (owner, 2026-10-31): the owner-confirmed memory items that replace the prompt patches are stored
once, on the first start of the step-10b build, in the owner's store only, as the owner's words; FERB's own inferred
duplicates of them end; a second start changes nothing; any other store gets none of them."""
from pathlib import Path
from typing import Any

from ferb.memory.upgrade import OWNER_SEED
from ferb.store.db import Database
from tests.test_planner_context import _own_daemon

AT = "2026-10-24T05:44:31.624567+05:30"
M5 = "When the owner says “message” or “call,” they mean WhatsApp unless they explicitly mention another app."
SCALER_GUESS = ("For Scaler-related Gmail, Docs, and assignment links, the owner prefers using the Chrome Scaler profile "
                "rather than another browser/profile.")


def _owner_store(home: Path) -> None:
    """The owner's live shapes (read 2026-10-31): M5 in their words with FERB's inferred copy beside it, and FERB's
    inferred Scaler item that M4 replaces."""
    home.mkdir(parents=True)
    db = Database(home / "ferb.db")
    db.migrate()
    c = db.conn

    def item(ident: str, kind: str, key: str, content: str, origin: str) -> None:
        c.execute("INSERT INTO journal (id, at, kind, data_json, origin) VALUES (?, ?, 'memory.added', ?, 'owner')",
                  ("j" + ident, AT, '{"content": "x"}'))
        c.execute("INSERT INTO memory_items (id, kind, subject, key, content, origin, status, confidence, observed_at, "
                  "valid_from, created_at) VALUES (?, ?, 'owner', ?, ?, ?, 'active', 1.0, ?, ?, ?)",
                  (ident, kind, key, content, origin, AT, AT, AT))
        c.execute("INSERT INTO memory_sources (item_id, kind, source_id, day) VALUES (?, 'journal', ?, '2026-10-24')",
                  (ident, "j" + ident))
    item("88bdea14092e42da8be4dc38983a4d69", "preference", "message_call_default_app", M5, "owner_stated")
    item("fc0f1e546b6b40f5a4e2b0d328c56d3a", "preference", "message_call_default_app", M5, "inferred")
    item("9f4693bcaec14c4c93b965117117af89", "preference", "scaler_chrome_profile", SCALER_GUESS, "inferred")
    db.close()


async def _go(fn: Any) -> Any:
    return fn()


def test_the_owner_items_are_stored_once_as_their_words(home: Path) -> None:
    _owner_store(home)
    d = _own_daemon(home)
    try:
        def read() -> dict[str, Any]:
            dd = d.daemon
            from ferb.identity.owner_secrets import OwnerSecrets
            return {"current": dd.memory.current_items(), "profile": dd.profile.ids(),
                    "seed": [e.data for e in dd._journal().of_kind("memory.owner_seed")],
                    "records": OwnerSecrets(dd.home).records()}
        got = d.run(_go(read))
    finally:
        d.stop()
    by_content = {" ".join(it.content.split()): it for it in got["current"]}
    for wanted in OWNER_SEED:
        it = by_content[" ".join(str(wanted["content"]).split())]
        assert it.origin == "owner_stated", wanted["id"]
        in_profile = it.id in got["profile"]
        assert in_profile == (wanted["subject"] == "owner"), wanted["id"]     # M7, M8: found by search only
    assert [it.id for it in got["current"] if it.content == M5] == ["88bdea14092e42da8be4dc38983a4d69"]
    current = {it.id for it in got["current"]}
    assert "fc0f1e546b6b40f5a4e2b0d328c56d3a" not in current and "9f4693bcaec14c4c93b965117117af89" not in current
    assert "kartikey060105@gmail.com" in " ".join(it.content for it in got["current"])
    [seed] = got["seed"]
    assert seed["skipped"] == ["M5"] and not seed["refused"]
    visa = got["records"]["https://www.usvisascheduling.com"]
    assert visa["username"] == "Kartikey Attri"

    again = _own_daemon(home, ids="again")
    try:
        def twice() -> tuple[int, int]:
            dd = again.daemon
            return len(dd._journal().of_kind("memory.owner_seed")), len(dd.memory.current_items())
        assert again.run(_go(twice)) == (1, len(got["current"]))
    finally:
        again.stop()


def test_another_store_gets_none_of_them(daemon: Any) -> None:
    def read() -> tuple[list[Any], list[Any]]:
        dd = daemon.daemon
        return dd.memory.current_items(), dd._journal().of_kind("memory.owner_seed")
    items, seeds = daemon.run(_go(read))
    assert items == [] and seeds == []
