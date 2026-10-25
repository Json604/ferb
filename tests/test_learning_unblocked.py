"""L36 §8: learning unblocked. The audit's refused consolidations (2026-10-24: 0 procedures, all 5 refused) as
fixtures: actions compared modulo per-run handles and expanded paths, app versions as observed ranges, and a receipt
when a run follows its procedure."""
import io
from pathlib import Path
from typing import Any

import pytest

from ferb.cli.admin import stats
from ferb.memory.learning import Learning, LearningRefused, comparable, observed_ranges
from ferb.store.db import Database
from tests.support.learning_fixtures import Clock

HOME = "/Users/owner"


@pytest.fixture
def store(tmp_path: Path) -> Any:
    db = Database(tmp_path / "ferb.db")
    db.migrate()
    learning = Learning(db, Clock(), lambda *args: None, lambda s: s)
    learning.home = HOME
    yield learning
    db.close()


def card(store: Learning, task_id: str, actions: list[dict[str, Any]], apps: dict[str, str] | None = None,
         **body: Any) -> Any:
    return store.put("episode", task_id, {"task_id": task_id, "verified": True, "request": "r", "intent": "i",
                                          "apps": apps or {}, "slots": {}, "status": "done", "ended_at": "2026-10-20",
                                          "actions": [a | {"outside_action": False} for a in actions]} | body,
                     [], origin_label="ferb", made_by="code:evidence-card/4", state="active")


def propose(store: Learning, cards: list[Any], steps: list[dict[str, Any]], slots: dict[str, str],
            values: list[dict[str, Any]]) -> Any:
    return store.propose_procedure({"intent": "i", "slots": slots, "preconditions": {"apps": {}}, "ui_hints": "",
                                    "steps": steps,
                                    "examples": [{"episode": c["id"], "slots": v} for c, v in zip(cards, values)]},
                                   "b1", "model:test|prompt:1")


def test_handles_paths_and_temp_folders_compare_by_kind() -> None:
    assert comparable({"tab": "t39", "element": "s1e8", "id": "p-e954619d", "path": "~/a.txt", "window_id": 7},
                      HOME) == {"tab": "<tab>", "element": "<element>", "id": "<process>", "path": f"{HOME}/a.txt",
                                "window_id": "<window>"}                  # fix round 1: each handle names its kind
    assert comparable({"path": "/private/var/folders/x/T/f.png"}, HOME) == {"path": "<temp>"}
    assert comparable({"timeout_s": 70}, HOME) != comparable({"timeout_s": 120}, HOME)


def test_a_slot_written_with_a_tilde_reproduces_an_expanded_path(store: Learning) -> None:
    cards = [card(store, f"w{i}", [{"id": f"w{i}-a", "tool": "fs.write",
                                    "args": {"path": f"{HOME}/ferb-test/n{i}.txt", "content": f"text {i}",
                                             "overwrite": True}}]) for i in range(3)]
    p = propose(store, cards, [{"tool": "fs.write", "args": {"path": "{path}", "content": "{content}", "overwrite": True},
                                "expect": "the file holds the text", "evidence": [f"w{i}-a" for i in range(3)]}],
                {"path": "string", "content": "string"},
                [{"path": f"~/ferb-test/n{i}.txt", "content": f"text {i}"} for i in range(3)])
    assert p["state"] in ("candidate", "learned")


def test_per_run_process_ids_and_tabs_differ_but_a_real_mismatch_is_refused(store: Learning) -> None:
    def run_of(task_id: str, process: str, tab: str, timeout: int) -> Any:
        return card(store, task_id, [
            {"id": task_id + "-s", "tool": "shell.start", "args": {"command": "sleep 60", "cwd": f"{HOME}/.ferb/w"}},
            {"id": task_id + "-w", "tool": "shell.wait", "args": {"id": process, "since": 0, "timeout_s": timeout}},
            {"id": task_id + "-c", "tool": "browser.act", "args": {"tab": tab, "element": "s1e8", "action": "click"}}])
    steps = [{"tool": "shell.start", "args": {"command": "sleep 60", "cwd": f"{HOME}/.ferb/w"}, "expect": "started",
              "evidence": ["a-s", "b-s"]},
             {"tool": "shell.wait", "args": {"id": "p-e954619d", "since": 0, "timeout_s": 120}, "expect": "exited",
              "evidence": ["a-w", "b-w"]},
             {"tool": "browser.act", "args": {"tab": "t39", "element": "s1e8", "action": "click"}, "expect": "clicked",
              "evidence": ["a-c", "b-c"]}]
    good = [run_of("a", "p-e954619d", "t39", 120), run_of("b", "p-a46c91e9", "t41", 120)]
    assert propose(store, good, steps, {}, [{}, {}])["kind"] == "procedure"
    bad = [run_of("a2", "p-e954619d", "t39", 120), run_of("b2", "p-a46c91e9", "t41", 70)]
    for step, ref in zip(steps, ("s", "w", "c")):
        step["evidence"] = ["a2-" + ref, "b2-" + ref]
    with pytest.raises(LearningRefused, match="reproduce"):
        propose(store, bad, steps, {}, [{}, {}])


def learned_on_two_brave_versions(store: Learning) -> Any:
    versions = ["155.1.97.59", "151.1.93.136", "155.1.97.59"]
    cards = [card(store, f"k{i}", [{"id": f"k{i}-a", "tool": "browser.navigate", "args": {"url": f"https://k/{i}"}}],
                  apps={"com.brave.Browser": v}) for i, v in enumerate(versions)]
    return propose(store, cards, [{"tool": "browser.navigate", "args": {"url": "{url}"}, "expect": "the board",
                                   "evidence": [f"k{i}-a" for i in range(3)]}], {"url": "string"},
                   [{"url": f"https://k/{i}"} for i in range(3)])


def test_app_versions_are_an_observed_range(store: Learning) -> None:
    unknown = [{"body": {"apps": {"unresolved-browser": "unknown", "com.apple.Notes": "4.11"}}},
               {"body": {"apps": {"com.apple.Notes": "unknown"}}}]
    assert observed_ranges(unknown) == {"com.apple.Notes": ["4.11", "4.11"]}      # what FERB couldn't capture adds nothing
    p = learned_on_two_brave_versions(store)
    assert p["body"]["preconditions"]["apps"] == {"com.brave.Browser": ["151.1.93.136", "155.1.97.59"]}
    assert store.procedure(p["id"], {"com.brave.Browser": "160.0.1.1"})["state"] == "learned"
    newer = store.procedure_parts(p["id"], {"com.brave.Browser": "160.0.1.1"}, None)[0].text
    assert "New version: verify each step" in newer and "151.1.93.136–155.1.97.59" in newer
    within = store.procedure_parts(p["id"], {"com.brave.Browser": "153.0.0.1"}, None)[0].text
    assert "Eligible for learned fast path" in within


def test_a_run_that_follows_its_procedure_gets_a_receipt_and_one_that_does_not_is_noted(daemon: Any) -> None:
    from tests.support.memory_items import run
    d = daemon.daemon
    d.learning.home = HOME
    p = run(daemon, lambda: learned_on_two_brave_versions(d.learning))
    run(daemon, lambda: card(d.learning, "followed", [{"id": "f-a", "tool": "browser.navigate",
                                                        "args": {"url": "https://k/9"}}]))
    run(daemon, lambda: card(d.learning, "ignored", [{"id": "g-a", "tool": "browser.act", "args": {}}]))
    run(daemon, lambda: d._procedure_outcome(p["id"], "followed", {"url": "https://k/9"}))
    run(daemon, lambda: d._procedure_outcome(p["id"], "ignored", {"url": "https://k/9"}))
    episodes = run(daemon, lambda: {r[0] for r in d.db.conn.execute(
        "SELECT n.key FROM learning_receipts r JOIN learning_nodes n ON n.id = r.episode_id")})
    assert "followed" in episodes and "ignored" not in episodes
    assert run(daemon, lambda: [e.data["task_id"] for e in d._journal().of_kind("memory.procedure_not_followed")]) == \
        ["ignored"]


def test_stats_show_procedures_learned_used_and_refused(store: Learning, tmp_path: Path) -> None:
    p = learned_on_two_brave_versions(store)
    run_card = card(store, "used", [{"id": "u-a", "tool": "browser.navigate", "args": {"url": "https://k/7"}}])
    store.receipt(p["id"], run_card["id"], {"url": "https://k/7"})
    store.db.conn.execute("INSERT INTO learning_jobs (source_ref, status, at, error, inputs) VALUES (?,?,?,?,?)",
                          ("intent:i-x", "failed", "2026-10-21", "every procedure proposed was refused: x", "g"))
    out = io.StringIO()
    assert stats(tmp_path, out, days=36500) == 0
    assert ("Procedures: 1 stored, 1 learned; used in 1 run(s); 1 kind(s) of task whose every proposed procedure was "
            "refused") in out.getvalue()


def test_a_phone_number_is_not_an_id_but_one_of_ferbs_ids_is() -> None:
    """Twelve digits read as an id body would make two recipients one (redaction's rule, L36 §7 ruling 6): in free
    text an id needs an a-f letter; under one of FERB's own id keys any body is one."""
    assert comparable({"to": "919876543210"}, HOME) != comparable({"to": "918888888888"}, HOME)
    assert comparable({"item_id": "m-" + "a" * 64}, HOME) == comparable({"item_id": "m-" + "b" * 64}, HOME) == \
        {"item_id": "<id>"}
    assert comparable({"task_id": "123456789012"}, HOME) == {"task_id": "<id>"}


def test_a_version_ferb_could_not_read_changes_nothing(store: Learning) -> None:
    """Installed now or stored by a procedure from before L36 (whose model copied a card's "unknown")."""
    p = learned_on_two_brave_versions(store)
    for unread in ("unknown", ""):
        text = store.procedure_parts(p["id"], {"com.brave.Browser": unread}, None)[0].text
        assert "Eligible for learned fast path" in text and "New version" not in text
    old = store.put("procedure", "notes", {"intent": "notes", "slots": {"x": "string"}, "steps": [],
                                           "preconditions": {"apps": {"com.apple.Notes": "unknown"}}},
                    [], made_by="code:test/1", state="candidate")
    for n in range(3):                                         # later runs, whose versions FERB captured
        store.receipt(old["id"], card(store, f"n{n}", [], apps={"com.apple.Notes": "4.11"})["id"], {"x": str(n)})
    text = store.procedure_parts(old["id"], {"com.apple.Notes": "4.11"}, None)[0].text
    assert "Eligible for learned fast path" in text and "New version" not in text


def test_a_run_follows_its_required_steps_with_the_planners_values(store: Learning) -> None:
    """`followed`: every required step by tool and arguments (slot values filled in); a step whose slots have no value
    by its tool; a failed action never counts; an optional step never decides it."""
    p = store.put("procedure", "i", {"intent": "i", "slots": {"url": "string"}, "preconditions": {"apps": {}},
                                     "steps": [{"tool": "browser.navigate", "args": {"url": "{url}"}, "optional": False},
                                               {"tool": "fs.write", "args": {"path": "~/x"}, "optional": True}]},
                  [], made_by="code:test/1", state="learned")

    def run(*actions: dict[str, Any]) -> dict[str, Any]:
        return {"body": {"actions": list(actions)}}
    navigate = {"tool": "browser.navigate", "args": {"url": "https://k/9"}}
    assert store.followed(p["id"], run(navigate), {"url": "https://k/9"})
    assert not store.followed(p["id"], run(navigate), {"url": "https://k/8"})
    assert store.followed(p["id"], run(navigate), None)                           # no values: by its tool
    assert not store.followed(p["id"], run(navigate | {"error": "timed out"}), {"url": "https://k/9"})
    assert not store.followed(p["id"], run({"tool": "browser.act", "args": {}}), None)


def test_a_superseded_versions_example_run_is_not_a_use(store: Learning, tmp_path: Path) -> None:
    """A revision keeps other examples: the runs the earlier version learned from still aren't uses of it."""
    p = learned_on_two_brave_versions(store)
    k3 = card(store, "k3", [{"id": "k3-a", "tool": "browser.navigate", "args": {"url": "https://k/3"}}],
              apps={"com.brave.Browser": "155.1.97.59"})
    cards = [store.current_card(f"k{i}") for i in (1, 2)] + [k3]
    revised = propose(store, cards, [{"tool": "browser.navigate", "args": {"url": "{url}"}, "expect": "the board",
                                      "evidence": [f"k{i}-a" for i in (1, 2, 3)]}], {"url": "string"},
                      [{"url": f"https://k/{i}"} for i in (1, 2, 3)])
    assert revised["supersedes"] == p["id"]
    out = io.StringIO()
    assert stats(tmp_path, out, days=36500) == 0
    assert "Procedures: 1 stored, 1 learned; used in 0 run(s); 0 kind(s)" in out.getvalue()


def test_runs_whose_app_versions_were_not_read_still_make_a_procedure_learned(store: Learning) -> None:
    """Concern ruling 1: versions are advisory, so learned no longer needs a latest run whose versions FERB captured
    (live 2026-10-25: only 6 of 32 verified cards could be that run). With no range there is no new-version line."""
    cards = [card(store, f"m{i}", [{"id": f"m{i}-a", "tool": "browser.navigate", "args": {"url": f"https://k/{i}"}}],
                  apps={"unresolved-mac-app": "unknown"}, app_versions_captured=False) for i in range(3)]
    p = propose(store, cards, [{"tool": "browser.navigate", "args": {"url": "{url}"}, "expect": "the board",
                                "evidence": [f"m{i}-a" for i in range(3)]}], {"url": "string"},
                [{"url": f"https://k/{i}"} for i in range(3)])
    assert p["body"]["preconditions"]["apps"] == {} and p["state"] == "learned"
    text = store.procedure_parts(p["id"], {"com.apple.Notes": "4.11"}, None)[0].text
    assert "Eligible for learned fast path" in text and "New version" not in text


def _receipts(daemon: Any, procedure_id: str) -> dict[str, int]:
    from tests.support.memory_items import run
    return run(daemon, lambda: {r[0]: r[1] for r in daemon.daemon.db.conn.execute(
        "SELECT n.key, r.successful FROM learning_receipts r JOIN learning_nodes n ON n.id = r.episode_id "
        "WHERE r.procedure_id = ? AND n.key NOT LIKE 'k%'", (procedure_id,))})


def test_a_run_that_stops_at_a_broken_step_records_a_failure_receipt(daemon: Any) -> None:
    """Concern ruling 2: a run offered a procedure that ended unverified writes its failure receipt whether or not it
    followed every step, so a procedure that breaks partway becomes suspect, as before L36."""
    from tests.support.memory_items import run
    d = daemon.daemon
    d.learning.home = HOME
    p = run(daemon, lambda: learned_on_two_brave_versions(d.learning))
    broken = [{"id": "b-a", "tool": "browser.navigate", "args": {"url": "https://k/9"}, "error": "net::ERR_TIMED_OUT"}]
    run(daemon, lambda: card(d.learning, "broken", broken, verified=False, status="failed"))
    run(daemon, lambda: card(d.learning, "elsewhere", [{"id": "e-a", "tool": "browser.act", "args": {}}],
                             verified=False, status="failed"))
    for task in ("broken", "elsewhere"):
        run(daemon, lambda task=task: d._procedure_outcome(p["id"], task, {"url": "https://k/9"}))
    assert _receipts(daemon, p["id"]) == {"broken": 0, "elsewhere": 0}
    assert run(daemon, lambda: d.learning.procedure(p["id"], {})["state"]) == "suspect"
    assert run(daemon, lambda: d._journal().of_kind("memory.procedure_not_followed")) == []


def test_the_owners_fail_verdict_on_a_run_that_did_not_follow_counts_against_its_procedure(daemon: Any) -> None:
    """A success that didn't follow the procedure gets no receipt; the owner's later fail verdict makes it a failed run
    offered the procedure, so it gets its failure receipt then (concern ruling 2)."""
    from tests.support.memory_items import run
    d = daemon.daemon
    d.learning.home = HOME
    p = run(daemon, lambda: learned_on_two_brave_versions(d.learning))
    run(daemon, lambda: card(d.learning, "ignored", [{"id": "g-a", "tool": "browser.act", "args": {}}]))
    run(daemon, lambda: d._procedure_outcome(p["id"], "ignored", {"url": "https://k/9"}))
    assert _receipts(daemon, p["id"]) == {}
    run(daemon, lambda: d.db.conn.execute("INSERT INTO owner_verdicts (id, task_id, verdict, note, at) VALUES "
                                          "('v-1', 'ignored', 'fail', '', 'now')"))
    run(daemon, lambda: d._refresh_learning_verdict("ignored"))
    assert _receipts(daemon, p["id"]) == {"ignored": 0}


def test_the_consolidator_is_not_asked_for_app_versions() -> None:
    """Concern ruling 3: code records the versions (`observed_ranges`), so the prompt and schema no longer ask."""
    from ferb.core.prompts import load_prompt
    from ferb.memory.consolidate import PROPOSAL_SCHEMA
    words = " ".join(load_prompt("memory/consolidate.md").split())
    assert "version" not in words and "precondition" not in words
    procedure = PROPOSAL_SCHEMA["properties"]["procedures"]["items"]
    assert "preconditions" not in procedure["properties"] and "preconditions" not in procedure["required"]


def test_a_failed_run_offered_a_procedure_is_not_a_use(store: Learning, tmp_path: Path) -> None:
    """Since a failed run gets its failure receipt whether or not it followed (concern ruling 2), "used in" counts the
    successes, each of which followed its procedure (`record_use`)."""
    p = learned_on_two_brave_versions(store)
    failed = card(store, "failed", [{"id": "x-a", "tool": "browser.act", "args": {}}], verified=False, status="failed")
    assert store.record_use(p["id"], failed, None) is None                  # None: its failure receipt is written
    out = io.StringIO()
    assert stats(tmp_path, out, days=36500) == 0
    assert "Procedures: 1 stored, 1 learned; used in 0 run(s);" in out.getvalue()



# Fix round 1 (task-10-review.md I1–I3, M1–M4) -------------------------------------------------------------------------

def _clicked_and_uploaded(store: Learning) -> tuple[Any, list[dict[str, Any]]]:
    """The reviewer's probe: three runs clicked t39/s1e8, t41/s2e5 and t3/s1e12 and uploaded one temporary file each;
    the model's template carries the first run's literal values."""
    runs = [("t39", "s1e8", "/tmp/invoice-aug.pdf"), ("t41", "s2e5", "/tmp/invoice-sep.pdf"),
            ("t3", "s1e12", "/private/tmp/invoice-oct.pdf")]
    cards = [card(store, f"u{i}", [
        {"id": f"u{i}-c", "tool": "browser.act", "args": {"tab": tab, "element": el, "action": "click"}},
        {"id": f"u{i}-u", "tool": "browser.act",
         "args": {"tab": tab, "element": el, "action": "upload", "files": [temp]}}])
             for i, (tab, el, temp) in enumerate(runs)]
    steps = [{"tool": "browser.act", "args": {"tab": "t39", "element": "s1e8", "action": "click"},
              "expect": "the upload form", "evidence": [f"u{i}-c" for i in range(3)]},
             {"tool": "browser.act", "args": {"tab": "t39", "element": "s1e8", "action": "upload",
                                              "files": ["/tmp/invoice-aug.pdf"]},
              "expect": "the file is attached", "evidence": [f"u{i}-u" for i in range(3)]}]
    return propose(store, cards, steps, {}, [{}, {}, {}]), cards


def test_a_stored_step_names_handle_kinds_never_one_runs_values(store: Learning) -> None:
    """I1: a template that reproduces its runs only modulo handles keeps the kinds, and the worker is told to find
    each one in its own run; it never replays one run's tab, element or temporary file."""
    p, _ = _clicked_and_uploaded(store)
    stored = [s["args"] for s in p["body"]["steps"]]
    assert stored == [{"tab": "<tab>", "element": "<element>", "action": "click"},
                      {"tab": "<tab>", "element": "<element>", "action": "upload", "files": ["<temp>"]}]
    text = store.procedure_parts(p["id"], {}, None)[0].text
    assert not any(literal in text for literal in ("t39", "s1e8", "invoice-aug"))
    assert "find its id in this run" in text and "a temporary file this run makes" in text
    later = {"body": {"actions": [
        {"tool": "browser.act", "args": {"tab": "t50", "element": "s9e2", "action": "click"}},
        {"tool": "browser.act", "args": {"tab": "t50", "element": "s9e3", "action": "upload",
                                         "files": ["/tmp/invoice-nov.pdf"]}}]}}
    assert store.followed(p["id"], later, None)                   # a kind matches this run's own handle


def test_a_path_that_leaves_a_temp_folder_is_not_temporary(store: Learning) -> None:
    """I2: `..` is resolved before the temp check, so a home-folder path never passes as the run's temp file."""
    for path in ("/tmp/../Users/owner/.ssh/id_rsa",
                 "/private/var/folders/x/T/../../../../Users/owner/Documents/tax.pdf"):
        assert comparable({"path": path}, HOME) != comparable({"path": "/tmp/shot.png"}, HOME)
    cards = [card(store, f"r{i}", [{"id": f"r{i}-a", "tool": "fs.read", "args": {"path": f"/tmp/shot{i}.png"}}])
             for i in range(2)]
    with pytest.raises(LearningRefused, match="reproduce"):
        propose(store, cards, [{"tool": "fs.read", "args": {"path": "/tmp/../Users/owner/Documents/tax.pdf"},
                                "expect": "read", "evidence": ["r0-a", "r1-a"]}], {}, [{}, {}])


def test_handle_shapes_count_only_under_the_tools_handle_arguments() -> None:
    """I3: free text must be equal; a handle's shape names a handle only under the argument FERB's tools give it."""
    assert comparable({"text": "s1e8"}, HOME) != comparable({"text": "s2e3"}, HOME)          # TV episodes typed
    assert comparable({"text": "t5"}, HOME) != comparable({"text": "t6"}, HOME)
    md5 = ("d41d8cd98f00b204e9800998ecf8427e", "9e107d9d372bb6826bd81d3542a419d6")
    assert comparable({"expected": md5[0]}, HOME) != comparable({"expected": md5[1]}, HOME)
    assert comparable({"commit": "5bd6483abcde"}, HOME) != comparable({"commit": "8327839abcde"}, HOME)
    assert comparable({"text": "~"}, HOME) != comparable({"text": HOME}, HOME)
    assert comparable({"tab": "t5"}, HOME) == comparable({"tab": "t6"}, HOME)
    assert comparable({"split_elements": ["s1e2", "s1e3"]}, HOME) == {"split_elements": ["<element>", "<element>"]}
    assert comparable({"window": "Inbox"}, HOME) == {"window": "Inbox"}          # a Mac window title is data
    assert comparable({"window": "4242"}, HOME) == {"window": "<window>"}        # its id from a snapshot is a handle


def test_a_successful_run_of_a_procedure_that_is_gone_is_journaled_as_unavailable(daemon: Any) -> None:
    """M1: a missing procedure is not "not followed"."""
    from tests.support.memory_items import run
    d = daemon.daemon
    run(daemon, lambda: card(d.learning, "orphan", [{"id": "o-a", "tool": "browser.navigate", "args": {}}]))
    run(daemon, lambda: d._procedure_outcome("m-" + "0" * 64, "orphan", None))
    assert run(daemon, lambda: [e.kind for e in d._journal().of_kind("memory.receipt_unavailable")]) == \
        ["memory.receipt_unavailable"]
    assert run(daemon, lambda: d._journal().of_kind("memory.procedure_not_followed")) == []


def test_a_sealed_word_given_as_a_slot_value_still_counts_as_followed(tmp_path: Path) -> None:
    """M2: the card's arguments were stored redacted; the planner's values are redacted the same way before they are
    compared, keyed by the tool's own argument names."""
    from ferb.sanitize.redact import Redactor
    redactor = Redactor()
    redactor.remember_hidden_secret("sunshine")
    db = Database(tmp_path / "ferb.db")
    db.migrate()
    try:
        store = Learning(db, Clock(), lambda *args: None, redactor.text)
        store.secret_values = redactor.secret_values
        p = store.put("procedure", "answer", {"intent": "answer", "slots": {"answer": "string"}, "steps": [
            {"tool": "browser.act", "args": {"tab": "<tab>", "element": "<element>", "action": "type",
                                             "text": "{answer}"}, "optional": False}],
            "preconditions": {"apps": {}}}, [], made_by="code:test/1", state="candidate")
        typed = card(store, "typed", [{"id": "t-a", "tool": "browser.act",
                                       "args": {"tab": "t2", "element": "s1e4", "action": "type", "text": "sunshine"}}])
        assert "sunshine" not in str(typed["body"])                                  # the card holds it redacted
        assert store.followed(p["id"], typed, {"answer": "sunshine"})
    finally:
        db.close()


def test_a_version_with_no_digit_is_unknown(store: Learning) -> None:
    """M3: third-party version text without a digit ("Beta", "dev") would sort above every number."""
    assert observed_ranges([{"body": {"apps": {"x.app": "Beta"}}}, {"body": {"apps": {"x.app": "1.2"}}}]) == \
        {"x.app": ["1.2", "1.2"]}
    p = learned_on_two_brave_versions(store)
    assert "New version" not in store.procedure_parts(p["id"], {"com.brave.Browser": "dev"}, None)[0].text


def test_a_run_the_owner_stopped_or_said_no_in_writes_no_failure_receipt(daemon: Any) -> None:
    """M4: the owner's stop (status "cancelled", which only "stopped by you" sets) and the owner's No to one of FERB's
    confirmations are facts code records; such a run says nothing about the procedure."""
    from tests.support.memory_items import run
    d = daemon.daemon
    d.learning.home = HOME
    p = run(daemon, lambda: learned_on_two_brave_versions(d.learning))
    run(daemon, lambda: card(d.learning, "stopped", [], verified=False, status="cancelled"))
    run(daemon, lambda: card(d.learning, "declined", [], verified=False, status="failed"))
    run(daemon, lambda: d.db.conn.execute("INSERT INTO confirmations (id, tree_id, rule, key, granted, ask_span, used, "
                                          "at) VALUES ('c-1', 'declined', 'floor:pay', 'pay 40', 0, NULL, 0, 'now')"))
    for task in ("stopped", "declined"):
        run(daemon, lambda task=task: d._procedure_outcome(p["id"], task, None))
    assert _receipts(daemon, p["id"]) == {}
    assert run(daemon, lambda: sorted(e.data["why"] for e in d._journal().of_kind("memory.receipt_skipped"))) == \
        ["the owner said No to a confirmation", "the owner stopped it"]
