"""L28 (review round 4): episode cards are read in `ferb.memory.card_reader`, a process started as an exact entry point
that finds the database in the owner's FERB home itself. FERB's own ids survive redaction by key, while a secret placed
under an id key is still redacted. A ref damaged by an earlier redaction is the only missing source a capture may skip.
The reader never holds the WAL while it decodes, and a forget that can't empty the WAL is journaled."""
import json
import os
import sqlite3
import subprocess
import sys
import uuid

import pytest

from ferb.core.ids import RandomIds
from ferb.memory.journal import Journal
from ferb.memory.learning import LearningRefused
from ferb.orchestrator import episodes
from ferb.trace import compose
from tests.support.cards import Clock, card_store, finished_task
from tests.support.trace_builder import TraceBuilder

READER = [sys.executable, "-I", "-m", "ferb.memory.card_reader"]


def _reader(home, *args: str, request: dict | None = None) -> dict:
    run = subprocess.run([*READER, *args], input=json.dumps(request or {}), capture_output=True, text=True,
                         env={**os.environ, "FERB_HOME": str(home)}, timeout=120)
    return json.loads(run.stdout)


# Decision A: the exact entry point, no arguments; the database is the owner's FERB home's -------------------------------

def test_the_card_reader_takes_no_arguments_and_reads_the_owners_home(tmp_path):
    """`?`, `#` and `%` in the home's path can't change which file SQLite opens; an argument naming a file is refused."""
    home = tmp_path / "we?ird#ho%me"
    home.mkdir()
    db, _, spans, learning = card_store(home)
    finished_task(db, spans, learning, "t-tea")
    request = {"task_id": "t-tea", "apps": None, "home": str(home)}
    answer = _reader(home, request=request)
    assert answer.get("prepared", {}).get("task_id") == "t-tea", answer
    refused = _reader(home, str(home / "ferb.db"), "t-tea", request=request)
    assert "prepared" not in refused and "argument" in refused["error"], refused
    db.close()


def test_the_card_reader_refuses_a_request_for_another_home(tmp_path):
    """The daemon also names its home on stdin, purely as a check: the reader never opens it, and refuses when it
    isn't the home it found itself (or is missing)."""
    home, other = tmp_path / "home", tmp_path / "other"
    home.mkdir()
    other.mkdir()
    db, _, spans, learning = card_store(home)
    finished_task(db, spans, learning, "t-tea")
    for request in ({"task_id": "t-tea", "apps": None, "home": str(other)}, {"task_id": "t-tea", "apps": None}):
        refused = _reader(home, request=request)
        assert "prepared" not in refused and "home" in refused["error"], refused
    db.close()


# Finding 5: the reader holds no statement while it decodes; a forget that can't empty the WAL is journaled ---------------

def test_the_reader_reads_each_query_fully_before_decoding_so_the_wal_can_be_emptied(tmp_path, monkeypatch):
    """Measured in review: forget's wal_checkpoint(TRUNCATE) waited 5.21 s and returned busy while the reader held a
    statement open. While the reader decodes its rows, the daemon can now empty the WAL."""
    from ferb.memory import card_reader
    from ferb.memory.learning import Learning
    home = tmp_path / "home"
    home.mkdir()
    db, _, spans, learning = card_store(home)
    finished_task(db, spans, learning, "t-wal", reads=3)
    db.conn.execute("PRAGMA busy_timeout=100")
    seen = []
    real = Learning._action_sources

    def decoding(self, inp, result):                      # runs while the reader works through the task's spans
        seen.append(tuple(db.conn.execute("PRAGMA wal_checkpoint(TRUNCATE)").fetchone()))
        return real(self, inp, result)
    monkeypatch.setattr(Learning, "_action_sources", decoding)
    prepared = card_reader.read("t-wal", None, home=home)
    assert prepared is not None and prepared["task_id"] == "t-wal"
    assert seen and all(busy == 0 for busy, _, _ in seen), seen
    db.close()


def test_a_forget_that_cannot_empty_the_wal_is_journaled(daemon):
    """Forget empties the WAL so no copy of what was forgotten stays on disk. When a reader keeps it from doing that, the
    journal says so instead of passing it over."""
    async def setup():
        db = daemon.daemon.db
        db.conn.execute("PRAGMA busy_timeout=50")
        daemon.daemon.journal.append("note.test", {"text": "a row so the WAL has frames"})
        return db.path
    path = daemon.run(setup())
    reader = sqlite3.connect(path.absolute().as_uri() + "?mode=ro", uri=True)
    rows = iter(reader.execute("SELECT id FROM journal"))
    next(rows)                                            # a statement held open, as the old card reader did

    async def forget():
        daemon.daemon.learning.forget(["memory:nothing-here"])
        return [e.data for e in daemon.daemon.journal.of_kind("store.wal_not_truncated")]
    try:
        kept = daemon.run(forget())
    finally:
        reader.close()
    assert kept and kept[-1]["after"] == "memory.forget", kept


# Decision B: only a ref that provably carries the redaction marker may be skipped ----------------------------------------

DAMAGED = ["trail:0525142c8c[redacted]a36a40c24",                       # shapes from the owner's journal
           "m-8e953d2b8ca8f[redacted]b46e986ee416a81e4cc270dfe91fe0f6a23784"]


def test_a_capture_skips_a_ref_damaged_by_redaction(tmp_path):
    """60 old rows in the owner's journal carry ids an earlier redaction altered; every capture whose lineage reached
    them failed. No FERB id contains '[' or ']', so such a ref can never resolve: it is skipped, and reported."""
    db, _, spans, learning = card_store(tmp_path)
    real = learning.put("entity", "owner", {"text": "The owner"}, [], made_by="code:entity-summary/1", state="active")
    finished_task(db, spans, learning, "t-old", refs=[real["id"], *DAMAGED])
    prepared = learning.prepare_episode("t-old")
    assert prepared is not None
    assert real["id"] in prepared["sources"] and not set(DAMAGED) & set(prepared["sources"])
    assert prepared["damaged"] == sorted(DAMAGED)
    assert {"private", "external:unlabeled"} <= set(prepared["labels"])    # what it held is unknown: never less private
    card = learning.write_episode(prepared)
    assert card is not None and card["key"] == "t-old"
    db.close()


def test_any_other_missing_ref_still_fails_the_capture(tmp_path):
    """A missing ref without the marker may be an explicit forget: the card is not built past it."""
    db, _, spans, learning = card_store(tmp_path)
    finished_task(db, spans, learning, "t-gone", refs=["trail:" + uuid.uuid4().hex])
    with pytest.raises(LearningRefused, match="source is missing"):
        learning.prepare_episode("t-gone")
    db.close()


# Findings 2 and 3: FERB's own id keys keep their ids whole; a secret under an id key is still redacted ------------------

def _ids() -> dict:
    task, request, journal_id = uuid.uuid4().hex, uuid.uuid4().hex, uuid.uuid4().hex
    return {"task_id": task + ".step.inspect", "request_id": request, "session_id": "chat-" + uuid.uuid4().hex[:12],
            "covered": ["entry:" + journal_id], "source_refs": ["trail:" + uuid.uuid4().hex, "task:" + task]}


def test_journal_rows_keep_ferb_ids_whole_by_key_and_lineage_still_resolves(tmp_path):
    """A registered secret value can occur inside an id (live: task_id became 'aa[redacted]bb…', silently dropping a
    reply's lineage, so a later forget never reached its card). Ids under FERB's id keys are kept whole; the same
    value elsewhere in the row is still redacted."""
    db, redactor, spans, learning = card_store(tmp_path)
    journal = Journal(db, Clock(), RandomIds(), redactor)
    ids = _ids()
    root = ids["task_id"].split(".")[0]
    secrets_inside = [root[4:16], ids["request_id"][3:15], ids["session_id"][6:14],
                      ids["covered"][0][9:21], ids["source_refs"][0][10:22]]
    for value in secrets_inside:
        redactor.add_secret_value(value)
    row = journal.append("chat.reply", ids | {"text": "typed " + " and ".join(secrets_inside)})
    stored = json.loads(db.conn.execute("SELECT data_json FROM journal WHERE id=?", (row.id,)).fetchone()[0])
    for key in ("task_id", "request_id", "session_id", "covered", "source_refs"):
        assert stored[key] == ids[key], key
    assert not any(value in stored["text"] for value in secrets_inside)

    trace = TraceBuilder(task_id=ids["task_id"], request="tea", goal="tea").finish()
    episodes.write(db, compose.compose(trace) | {"made_by": episodes.MADE_BY, "built_at": learning.now()})
    assert "episode:" + ids["task_id"] in learning.resolve(["entry:" + row.id])      # the reply's task: still reached
    db.close()


def test_owner_question_and_reminder_ids_stay_whole_in_journal_rows_and_events(tmp_path):
    """Integration with Tasks 14 and 6: an owner question's approval_id is what a client answers by, and a reminder's
    timer_id is read back from its timer.set row (reminder dedupe). A registered value inside either id leaves it whole,
    in a journal row and in a redacted event; the same value elsewhere is still redacted."""
    db, redactor, _, _ = card_store(tmp_path)
    journal = Journal(db, Clock(), RandomIds(), redactor)
    approval, timer = "ap-" + uuid.uuid4().hex[:12], "tm-" + uuid.uuid4().hex
    inside = [approval[5:11], timer[8:20]]
    for value in inside:
        redactor.add_secret_value(value)
    row = journal.append("plan.question", {"approval_id": approval, "timer_id": timer,
                                           "question": "typed " + " and ".join(inside)})
    stored = json.loads(db.conn.execute("SELECT data_json FROM journal WHERE id=?", (row.id,)).fetchone()[0])
    assert (stored["approval_id"], stored["timer_id"]) == (approval, timer)
    assert not any(value in stored["question"] for value in inside)
    assert redactor({"approval_id": approval, "timer_id": timer}) == {"approval_id": approval, "timer_id": timer}
    db.close()


SECRET_WORD = "hunter2horse"
PROBES = ["sk-proj-" + "A1b2C3d4E5f6" * 3,                                       # the review's probe values
          "eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiJvd25lciJ9.c2lnbmF0dXJlLXZhbHVl",
          SECRET_WORD]


@pytest.mark.parametrize("value", PROBES)
def test_a_secret_under_an_id_key_is_still_redacted(tmp_path, value):
    """Round 3 kept any `[A-Za-z0-9_.:-]+` value under source_refs unredacted, so these three passed. Under every id key,
    in journal rows and span inputs alike, they are redacted; only a FERB id stays whole."""
    db, redactor, spans, learning = card_store(tmp_path)
    redactor.add_secret_value(SECRET_WORD)
    journal = Journal(db, Clock(), RandomIds(), redactor)
    ref = "journal:" + uuid.uuid4().hex
    data = {"source_refs": [value, ref], "covered": [value], "task_id": value, "request_id": value,
            "session_id": value}
    row = journal.append("chat.reply", data)
    span = spans.start("context.exposure", input={"source_refs": [value, ref], "reference": "x"})
    stored = [db.conn.execute("SELECT data_json FROM journal WHERE id=?", (row.id,)).fetchone()[0],
              db.conn.execute("SELECT input_json FROM spans WHERE id=?", (span.id,)).fetchone()[0]]
    for text in stored:
        assert value not in text, text
        assert ref in text
    db.close()


def test_a_registered_value_that_is_a_whole_id_body_is_redacted(tmp_path):
    """Kept whole is an id a short registered value merely occurs inside; a registered value that is the entire id-shaped
    token is the secret itself."""
    db, redactor, spans, _ = card_store(tmp_path)
    body = uuid.uuid4().hex
    redactor.add_secret_value(body)
    span = spans.start("context.exposure", input={"source_refs": ["trail:" + body], "reference": "x"})
    assert span.input["source_refs"] == ["trail:[redacted]"]
    db.close()



# Round 5: a registered secret that holds an id-shaped run is still redacted -------------------------------------------

SHAPES = {"uuid-format token": str(uuid.uuid4()),             # its last group is a 12-hex run
          "key-<32 hex>": "key-" + uuid.uuid4().hex,
          "pw:<32 hex>": "pw:" + uuid.uuid4().hex}


@pytest.mark.parametrize("shape", list(SHAPES))
def test_a_registered_secret_holding_an_id_shaped_run_is_redacted_under_every_id_key(tmp_path, shape):
    """Round 4 spared every id-shaped run from registered-value replacement, so a registered secret containing one
    (a UUID token, key-<hex>, pw:<hex>) survived whole under an id key, in journal rows and in span outputs, which the
    gateway also hands to the model. Only a hit strictly inside one generated id body is spared."""
    db, redactor, spans, _ = card_store(tmp_path)
    secret = SHAPES[shape]
    redactor.add_secret_value(secret)
    journal = Journal(db, Clock(), RandomIds(), redactor)
    for key in ("session_id", "request_id", "task_id", "source_refs"):
        value = [secret] if key == "source_refs" else secret
        row = journal.append("chat.reply", {key: value, "text": "x"})
        span = spans.start("tool.call", input={"tool": "http.get", key: value})
        spans.finish(span, output={"body": {"user": {key: value}}}, origin="external:web")
        stored = [db.conn.execute("SELECT data_json FROM journal WHERE id=?", (row.id,)).fetchone()[0],
                  *db.conn.execute("SELECT input_json, output_json FROM spans WHERE id=?", (span.id,)).fetchone()]
        assert not any(secret in text for text in stored), (key, stored)
        assert secret not in json.dumps(redactor({"page": {key: value}}))        # what reaches a model
    db.close()


def test_a_registered_value_reaching_past_an_id_body_is_redacted_one_inside_it_is_not(tmp_path):
    """A value straddling a ref's prefix and its body is redacted. A shorter value lying strictly inside a longer
    generated body is the accepted residual (L16): the id stays whole, or its lineage breaks."""
    db, redactor, spans, _ = card_store(tmp_path)
    straddled, inside = "trail:" + uuid.uuid4().hex, "trail:" + uuid.uuid4().hex
    redactor.add_secret_value(straddled[2:16])                     # "ail:" + the body's first 10 characters
    redactor.add_secret_value(inside[10:22])                       # 12 characters strictly inside the body
    span = spans.start("context.exposure", input={"source_refs": [straddled, inside], "reference": "x"})
    assert straddled[2:16] not in json.dumps(span.input) and span.input["source_refs"][1] == inside
    db.close()


def test_a_cards_key_keeps_its_task_id_whole(tmp_path):
    """`Learning.put` redacted a card's key as text, so a registered value inside a task id altered the episode card's
    key and every lookup by task id missed it. Ids in a card's key are protected by structure, like id keys."""
    from ferb.memory.learning import Learning
    db, redactor, spans, _ = card_store(tmp_path)
    learning = Learning(db, Clock(), lambda *args: None, redactor.text, redact_id=redactor.ident)   # as the daemon's
    task_id = uuid.uuid4().hex
    finished_task(db, spans, learning, task_id)
    redactor.add_secret_value(task_id[4:16])
    card = learning.capture_episode(task_id)
    assert card is not None and card["key"] == task_id
    assert learning.task_sources(task_id) and card["id"] in learning.task_sources(task_id)
    db.close()
