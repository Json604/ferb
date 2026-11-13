"""Fixtures for the Rust port of ferb/daemon/areas/compose.py (ENG-3): a question's kind, its first line, the
journal rows of the questions FERB asks and their answers (DMN-583), `_publish`, the published events' consumers in
their order (`_wire_events` over the real ferb/daemon/events.py), today's activity device id, and the privacy lists
into the one Redactor (`_apply_privacy`). The real mixin methods run over a fake daemon whose other parts record what
they are handed.

    cd <repo> && uv run python rust/fixtures/daemon-b/compose.py

Writes compose.json beside this file: `kinds` ([capability, kind, answer]), `first_lines` ([text, answer]),
`questions` (scenarios: approvals and the events in order, the journal rows written, redacted as the journal stores
them), `publish` (scenarios: what the events got), `wiring` (scenarios: the consumers' calls in order),
`activity_device`, `privacy`. Invented data only; the same bytes on every run.
"""

import json
import os
import shutil
import sys
import tempfile
import time
from pathlib import Path
from types import SimpleNamespace
from typing import Any

REPO = Path(__file__).resolve().parents[3]
HERE = Path(__file__).resolve().parent
os.environ["TZ"] = "UTC"
time.tzset()
sys.path.insert(0, str(REPO))

from ferb.config import Config  # noqa: E402
from ferb.daemon.areas.compose import Compose, _first_line, question_kind  # noqa: E402
from ferb.daemon.events import Events  # noqa: E402
from ferb.daemon.questions import Approval  # noqa: E402
from ferb.sanitize.redact import Redactor  # noqa: E402

ROOT = Path(tempfile.gettempdir()) / "fxb-compose"


def js(v: Any) -> Any:
    return json.loads(json.dumps(v))


class Journal:
    def __init__(self, redact: Any) -> None:
        self.rows: list[Any] = []
        self.redact = redact

    def append(self, kind: str, data: dict[str, Any]) -> None:
        self.rows.append([kind, js(self.redact(data))])


def approval(a: dict[str, Any]) -> Approval:
    a = dict(a)
    if "hidden" in a:
        a["hidden"] = tuple(a["hidden"])
    return Approval(**a)


class Fake(Compose):
    def __init__(self, approvals: list[dict[str, Any]] | None = None, *, device: str = "mac-1",
                 home: Path | None = None) -> None:
        self.redact = Redactor()
        self.log: list[Any] = []
        self.jr = Journal(self.redact)
        self.state = SimpleNamespace(approvals={a["id"]: approval(a) for a in approvals or []})
        self._asked_kinds: dict[str, str] = {}
        self._answered_ids: set[str] = set()
        self._activity_device_id = None
        self.device_id = device
        self._shutting_down = False
        self.events = SimpleNamespace(publish=lambda topic, data: self.log.append(["events", topic, js(data)]))
        self.supervisor = SimpleNamespace(
            notify_plan_event=lambda topic, data: self.log.append(["plans", topic, js(data)]),
            conversations=SimpleNamespace(observe=lambda data: self.log.append(["conversations", js(data)])))
        self.daily_activity = SimpleNamespace(record=lambda row: self.log.append(["activity", js(row)]))
        self.bus = SimpleNamespace(publish=lambda topic, data: self.log.append(["bus", topic, js(data)]))
        if home is not None:
            self.config = Config(home)
            self._redact_override = None

    def _journal(self) -> Any:
        return self.jr

    def _attention_arrived(self, data: dict[str, Any]) -> None:
        self.log.append(["attention", js(data)])

    def _approval_notice(self, data: dict[str, Any]) -> None:
        self.log.append(["approval_notice", js(data)])

    def _tell_refused(self, data: dict[str, Any]) -> None:
        self.log.append(["tell_refused", js(data)])

    def _task_paused(self, data: dict[str, Any]) -> None:
        self.log.append(["task_paused", js(data)])

    def _task_resumed(self, data: dict[str, Any]) -> None:
        self.log.append(["task_resumed", js(data)])

    def _spawn(self, coro: Any, name: str) -> None:
        pass


# ---- question_kind
CAPS = ["ferb.confirm", "ferb.retry", "conversation.approve", "plan.ask", "downloads.delete", "browsing.choose",
        "owner.ask", "loop.ask", "runtime.switch", "memory.forget", "browser.act", "", "ferb.check_in"]
KINDS = ["owner_question", "owner_login_code", "owner_admin_password", "owner_secret", "secret", ""]
kinds = [[c, k, question_kind(c, k)] for c in CAPS for k in KINDS]

# ---- _first_line
TEXTS: list[Any] = [None, "", "   ", "one line", "  first  \nsecond", "\n\n  \t\nthird\n", "a\r\nb", "a\rb",
                    " x y", "\x0b\x0c\x1c\x1d\x1elast", "\x85next", "é" * 250, "word " * 60,
                    "  　full-width space　", "tab\tinside  ", " nbsp "]
first_lines = [[t, _first_line(t)] for t in TEXTS]

# ---- _question_record
A_TASK = {"id": "ap-1", "capability": "owner.ask", "reason": "Which file?\nthe second line", "request_id": "r1",
          "kind": "owner_question", "task_id": "t1.w1", "summary": "", "exact": ""}
A_CONFIRM = {"id": "ap-2", "capability": "ferb.confirm", "reason": "", "request_id": "r1",
             "summary": "Send the mail to new@example.test?", "exact": "  \nnew@example.test\nmore",
             "task_id": "t1.w1"}
A_SECRET = {"id": "ap-3", "capability": "owner.ask", "reason": "Your code?", "request_id": "r2",
            "kind": "owner_login_code", "task_id": ""}
A_HIDDEN = {"id": "ap-4", "capability": "plan.ask", "reason": "Password for the share? It is hunter2-invented",
            "request_id": "", "kind": "owner_question"}
A_EMPTY = {"id": "ap-5", "capability": "loop.ask", "reason": "", "request_id": "r3", "summary": ""}

QUESTION_SCENARIOS: list[dict[str, Any]] = [
    {"name": "task_question_answered_in_words", "approvals": [A_TASK], "steps": [
        ["approval.requested", {"approval_id": "ap-1", "task_id": "t1.w1"}],
        ["set", "ap-1", {"state": "granted", "answer": "The second one, please\nthanks"}],
        ["approval.resolved", {"approval_id": "ap-1"}],
        ["approval.resolved", {"approval_id": "ap-1"}]]},
    {"name": "confirm_by_option", "approvals": [A_CONFIRM], "steps": [
        ["approval.requested", {"approval_id": "ap-2", "question_kind": "payment"}],
        ["approval.resolved", {"approval_id": "ap-2"}],
        ["set", "ap-2", {"state": "granted", "choice": "Yes", "answer": "Yes"}],
        ["approval.resolved", {"approval_id": "ap-2", "question_kind": "other"}]]},
    {"name": "secret_answer_never_said", "approvals": [A_SECRET], "steps": [
        ["approval.requested", {"approval_id": "ap-3", "task_id": None}],
        ["set", "ap-3", {"state": "granted", "answer": "123456"}],
        ["approval.resolved", {"approval_id": "ap-3"}]]},
    {"name": "hidden_in_answer", "approvals": [A_HIDDEN], "steps": [
        ["approval.requested", {"approval_id": "ap-4"}],
        ["set", "ap-4", {"state": "granted", "answer": "it is [hidden]", "hidden": ["password"]}],
        ["approval.resolved", {"approval_id": "ap-4"}]]},
    {"name": "denied_no_answer_and_unknown", "approvals": [A_EMPTY], "steps": [
        ["approval.requested", {"approval_id": "nope"}],
        ["approval.requested", {"approval_id": "ap-5", "task_id": ""}],
        ["set", "ap-5", {"state": "denied"}],
        ["approval.resolved", {"approval_id": "ap-5"}],
        ["approval.resolved", {}]]},
    {"name": "answered_without_asked_record", "approvals": [dict(A_TASK, state="superseded", answer="")], "steps": [
        ["approval.resolved", {"approval_id": "ap-1"}]]},
    {"name": "secret_words_in_question", "approvals": [
        {"id": "ap-6", "capability": "owner.ask", "reason": "Use password: hunter2-invented here?", "request_id": "r9",
         "summary": "", "exact": "card 4111 1111 1111 1111", "task_id": "t9"}], "steps": [
        ["approval.requested", {"approval_id": "ap-6", "task_id": "t9.x"}],
        ["set", "ap-6", {"state": "used", "answer": "my password: hunter2-invented ok"}],
        ["approval.resolved", {"approval_id": "ap-6"}]]},
]


def run_questions(sc: dict[str, Any]) -> dict[str, Any]:
    d = Fake(sc["approvals"])
    for step in sc["steps"]:
        if step[0] == "set":
            ap = d.state.approvals[step[1]]
            for k, v in step[2].items():
                setattr(ap, k, tuple(v) if k == "hidden" else v)
        else:
            d._question_record(step[0], dict(step[1]))
    return {**sc, "journal": d.jr.rows, "asked_kinds": dict(sorted(d._asked_kinds.items()))}


# ---- _publish
PUBLISH = [
    {"name": "adds_request_id", "approvals": [A_TASK],
     "event": ["approval.requested", {"approval_id": "ap-1", "task_id": "t1.w1", "x": 1}]},
    {"name": "keeps_request_id", "approvals": [A_TASK],
     "event": ["approval.requested", {"approval_id": "ap-1", "task_id": "t1.w1", "request_id": None}]},
    {"name": "no_task", "approvals": [A_TASK], "event": ["approval.requested", {"approval_id": "ap-1"}]},
    {"name": "empty_request", "approvals": [A_HIDDEN],
     "event": ["approval.requested", {"approval_id": "ap-4", "task_id": "t"}]},
    {"name": "unknown_approval", "approvals": [], "event": ["approval.requested", {"task_id": "t"}]},
    {"name": "other_topic", "approvals": [A_TASK],
     "event": ["approval.resolved", {"approval_id": "ap-1", "task_id": "t1.w1"}]},
]


def run_publish(sc: dict[str, Any]) -> dict[str, Any]:
    d = Fake(sc["approvals"])
    d._publish(sc["event"][0], dict(sc["event"][1]))
    return {**sc, "got": d.log}


# ---- _wire_events
WIRING = [
    {"name": "every_topic", "approvals": [A_TASK], "shutting_down": False, "supervisor": True, "events": [
        ["trail.event", {"source": "browser", "id": "tr-1", "url": "https://x.test/?k=v", "note": "pw: hunter2x"}],
        ["trail.event", {"source": "accessibility", "id": "tr-2"}],
        ["action.observed", {"what": "clicked", "tool": "mac.click"}],
        ["action.observed", {"what": None, "source": "mac"}],
        ["perception.attention", {"notice": "n-1"}],
        ["approval.requested", {"approval_id": "ap-1", "task_id": "t1.w1"}],
        ["secret.refused", {"site": "a.test"}],
        ["approval.resolved", {"approval_id": "ap-1"}],
        ["task.paused", {"task_id": "t1", "schedule_id": "s-1"}],
        ["task.resumed", {"task_id": "t1"}],
        ["task.done", {"task_id": "t1", "summary": "card 4111 1111 1111 1111"}]]},
    {"name": "no_supervisor_shutting_down", "approvals": [], "shutting_down": True, "supervisor": False, "events": [
        ["trail.event", {"source": "files", "what": "kept what"}],
        ["perception.attention", {"notice": "n-2"}],
        ["plan.step", {"x": 1}]]},
]


def run_wiring(sc: dict[str, Any]) -> dict[str, Any]:
    d = Fake(sc["approvals"])
    d._shutting_down = sc["shutting_down"]
    if not sc["supervisor"]:
        d.supervisor = None
    d.events = Events(lambda: d.redact)
    d._wire_events()
    for topic, data in sc["events"]:
        d.events.publish(topic, dict(data))
    return {**sc, "log": d.log, "journal": d.jr.rows}


# ---- _activity_device
d_dev = Fake([])
seq = []
for dev in ["mac-1", "mac-1", "owner@example.test", "4111111111111111", "Käse-Mac"]:
    d_dev.device_id = dev
    seq.append([dev, d_dev._activity_device()])
activity_device = seq

# ---- _apply_privacy
PROBES = ["see https://www.excluded.example/path and https://kept.example/x",
          "mail.excluded.example/inbox", "1Password says hi", "Notes app", "plain text"]
PRIVACY = [
    {"name": "domains_and_apps",
     "config": '[privacy]\nexclude_domains = ["excluded.example"]\nexclude_apps = ["com.example.notes"]\n'},
    {"name": "defaults", "config": ""},
    {"name": "empty_lists", "config": '[privacy]\nexclude_domains = []\nexclude_apps = []\n'},
]


def run_privacy(sc: dict[str, Any]) -> dict[str, Any]:
    if ROOT.exists():
        shutil.rmtree(ROOT)
    home = ROOT / "home"
    home.mkdir(parents=True, mode=0o700)
    (home / "config.toml").write_text(sc["config"])
    (home / "config.toml").chmod(0o600)
    d = Fake([], home=home)
    before = d.redact
    d._apply_privacy()
    assert d.redact is before
    return {**sc, "probes": [[p, d.redact(p), d.redact.excluded_host(p.split("/")[0])] for p in PROBES]}


def main() -> None:
    try:
        out = {"kinds": kinds, "first_lines": first_lines,
               "questions": [run_questions(s) for s in QUESTION_SCENARIOS],
               "publish": [run_publish(s) for s in PUBLISH],
               "wiring": [run_wiring(s) for s in WIRING],
               "activity_device": activity_device,
               "privacy": [run_privacy(s) for s in PRIVACY]}
    finally:
        shutil.rmtree(ROOT, ignore_errors=True)
    text = json.dumps(out, ensure_ascii=False, indent=1)
    (HERE / "compose.json").write_text(text + "\n", encoding="utf-8")
    print(f"compose.json: {len(text)} bytes")


if __name__ == "__main__":
    main()
