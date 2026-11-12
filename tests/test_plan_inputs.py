"""A plan's input lists through the real daemon and the scripted model (ORCH-305): each step input is compared in the
inventory's own form (ORCH-580), so a path written relative to the workspace, through a link or as a folder still
counts; an input in no step or in two steps is told back; only text a worker could read counts toward its window."""
import json
from pathlib import Path
from typing import Any

from ferb.orchestrator.plans import Plan, Step
from ferb.cli.client import Client
from tests.conftest import RunningDaemon, chat, done, install, outcome, report, sup, task, turn_events
from tests.support.schedules import run


def _plan(steps: list[Step], input_paths: list[str]) -> dict[str, Any]:
    spec = task("Read the notes")
    spec["plan"] = Plan(steps=steps, input_paths=input_paths).model_dump()
    return spec


def _notes(*paths: str) -> str:
    return json.dumps(report("done", "Read.", input_notes=[{"input": p, "note": "Read it.", "evidence": [p]}
                                                           for p in paths]))


def _repairs(daemon: RunningDaemon) -> list[str]:
    return run(daemon, lambda: [" ".join(e.data["problems"]) for e in daemon.daemon.journal.of_kind("task.repair")])


def _raw(daemon: RunningDaemon, text: str) -> list[dict[str, Any]]:
    """A turn's events with its live lines (`chat` drops them once their contract is checked)."""
    c = Client(daemon.sock, timeout=30)
    try:
        events = list(c.stream("chat.send", {"session_id": "s1", "text": text}))
    finally:
        c.close()
    turn_events(events)
    return events


def _context(script: Any, key: str) -> str:
    return next(c for _, k, c, _ in script.contexts if k == key)


# decision: ORCH-305, ORCH-580
def test_inputs_written_as_typed_or_as_a_folder_match_the_inventory(daemon: RunningDaemon) -> None:
    """Live 2026-11-10 (Codex): `~/…/statement.csv` and a folder `~/…/family` were "absent from inventory" because
    the check compared them with the inventory's resolved paths as text. Here one input is relative to the workspace and
    one is a folder under the test's temp folder (/var, a link to /private/var): no repair, and each step's worker is
    handed, and notes, the inventoried paths."""
    workspace = daemon.daemon.home / "workspace"
    (workspace / "notes").mkdir(parents=True, exist_ok=True)
    (workspace / "notes" / "a.txt").write_text("first")
    more = workspace / "more"
    more.mkdir()
    (more / "b.txt").write_text("second")
    (more / "c.txt").write_text("third")
    real_a = str((workspace / "notes" / "a.txt").resolve())
    real_b, real_c = str((more / "b.txt").resolve()), str((more / "c.txt").resolve())
    spec = _plan([Step(id="one", kind="do", goal="Note the first file", inputs=["notes/a.txt"]),
                  Step(id="two", kind="do", goal="Note the folder", inputs=[str(more)]),
                  Step(id="sum", kind="do", goal="Combine the notes", depends_on=["one", "two"])],
                 [str(workspace / "notes" / "a.txt"), str(more)])
    script = install(daemon, [sup("task", "Reading.", spec)],
                     {"step.one": _notes(real_a), "step.two": _notes(real_b, real_c), "step.sum": done("All read.")})
    final = outcome(chat(daemon, "read my notes"))
    assert _repairs(daemon) == []
    assert final["status"] == "done"
    assert real_a in _context(script, "step.one") and real_b in _context(script, "step.two")


# decision: ORCH-305, ORCH-580
def test_an_input_in_no_step_or_in_two_is_told_back(daemon: RunningDaemon) -> None:
    """Every inventoried input is in exactly one step's list (ORCH-305): a folder listed by two steps covers its file
    twice, a file no step lists is unassigned and a path that isn't there is absent; each is told back for repair."""
    workspace = daemon.daemon.home / "workspace"
    (workspace / "notes").mkdir(parents=True, exist_ok=True)
    (workspace / "notes" / "a.txt").write_text("first")
    (workspace / "other.txt").write_text("other")
    twice = _plan([Step(id="one", kind="do", goal="Note it", inputs=["notes"]),
                   Step(id="two", kind="do", goal="Note it again", inputs=["notes", "missing.txt"]),
                   Step(id="sum", kind="do", goal="Combine", depends_on=["one", "two"])],
                  [str(workspace / "notes"), str(workspace / "other.txt")])
    install(daemon, [sup("task", "Reading.", twice), sup("reply", "I'll ask again.")], {})
    chat(daemon, "read my notes")
    said = _repairs(daemon)[0]
    assert "assigned more than once" in said and str((workspace / "notes" / "a.txt").resolve()) in said
    assert "unassigned inputs" in said and str((workspace / "other.txt").resolve()) in said
    assert "absent from inventory" in said and "missing.txt" in said


# decision: ORCH-581
def test_only_text_counts_toward_a_workers_window(daemon: RunningDaemon) -> None:
    """Live 2026-11-10 (Codex, shrink a video): a 35 MB video was counted as 35 million tokens to read, so it could
    never be an input. A worker is handed paths, and only a text file can be read into its context as text: a file
    over the worker's whole window runs when it isn't text and is told back when it is."""
    workspace = daemon.daemon.home / "workspace"
    workspace.mkdir(parents=True, exist_ok=True)
    window = 1_000_000                                  # the scripted runtime's context window
    video = workspace / "demo.mov"
    video.write_bytes(b"\x00\x00\x00\x18ftypqt  " + bytes(range(256)) * (window // 256 + 1))
    words = workspace / "log.txt"
    words.write_text("word " * (window // 5 + 1))
    real = str(video.resolve())
    shrink = _plan([Step(id="shrink", kind="do", goal="Shrink the video", inputs=[str(video)])], [str(video)])
    install(daemon, [sup("task", "Shrinking.", shrink)], {"step.shrink": _notes(real)})
    assert outcome(chat(daemon, "shrink this video"))["status"] == "done"
    assert _repairs(daemon) == []
    read = _plan([Step(id="read", kind="do", goal="Read the log", inputs=[str(words)])], [str(words)])
    install(daemon, [sup("task", "Reading.", read), sup("reply", "That log is too long to read in one go.")], {})
    chat(daemon, "read this log")
    assert "tokens" in _repairs(daemon)[0]


# decision: ORCH-625
def test_a_plan_whose_steps_list_no_inputs_walks_none_of_its_folders(daemon: RunningDaemon, monkeypatch: Any) -> None:
    """Live 2026-11-11 14:14: the plan named ~/Desktop/ferb (31 GB) in input_paths while no step listed an input; the
    walk of every file took 4.5 minutes with nothing on screen, and no check used it. Now nothing is walked."""
    from ferb.orchestrator import plans

    def tripwire(*_: Any) -> Any:
        raise AssertionError("walked a folder no step lists")
    monkeypatch.setattr(plans, "_inventory", tripwire)
    workspace = daemon.daemon.home / "workspace"
    (workspace / "huge").mkdir(parents=True, exist_ok=True)
    spec = _plan([Step(id="gather", kind="do", goal="Gather the story"),
                  Step(id="write", kind="do", goal="Write it up", depends_on=["gather"])], [str(workspace / "huge")])
    install(daemon, [sup("task", "Working.", spec)], {"step.gather": done("Gathered."), "step.write": done("Written.")})
    events = _raw(daemon, "write up the story of my project")
    assert _repairs(daemon) == [] and outcome(events)["status"] == "done"
    assert not [e for e in events if e["type"] == "progress" and e.get("planning")]


# decision: ORCH-625, ORCH-305, ORCH-581
def test_a_listed_folder_too_big_for_a_worker_stops_the_walk_and_says_so(daemon: RunningDaemon) -> None:
    """A listed folder whose text passes a worker's whole context can't be handed over, whatever else it holds: the
    walk stops there, the turn says what FERB is looking at meanwhile, and the planner is told the folder is too big,
    with what each subfolder held by then. Subfolders after the stop are never read."""
    workspace = daemon.daemon.home / "workspace"
    window = 1_000_000                                  # the scripted runtime's context window
    big = workspace / "repo"
    for name in ("a", "b", "c"):
        (big / name).mkdir(parents=True, exist_ok=True)
    (big / "a" / "one.txt").write_text("word " * (window // 5 + 1))
    (big / "b" / "two.txt").write_text("word " * 10)
    (big / "c" / "three.txt").write_text("word " * 10)
    spec = _plan([Step(id="read", kind="do", goal="Read the repo", inputs=[str(big)])], [str(big)])
    install(daemon, [sup("task", "Reading.", spec), sup("reply", "That folder is too big to read in one go.")], {})
    events = _raw(daemon, "read my repo")
    said = _repairs(daemon)[0]
    assert "can't be handed over whole" in said and "a: at least" in said
    assert "b: at least" not in said and "c: at least" not in said and "Measured inventory" not in said
    lines = [e["text"] for e in events if e["type"] == "progress" and e.get("planning")]
    assert lines and lines[0].startswith("Looking at the files in ") and lines[0].endswith("repo")
