"""L39: careful checks on steps that change files, from the live case (task 39e35d57: a shell perl replace corrupted a
table header, the step's checker made 0 tool calls in 11.8 s and passed it).

- The checker sees what changed: an old-vs-new diff of every file the step changed, shell writes included.
- A pass needs a look: a met=true on such a step without reading each changed file (or its change record) is refused,
  the check runs once more with the reason, and a second refusal is met=null, never a pass.
- Such a checker runs at high reasoning effort; other checkers are unchanged."""

import json
import re
from pathlib import Path
from typing import Any

from tests.conftest import FakeRuntime, Script, chat, cond, done, outcome, sup, task, tool
from tests.support.checker_calls import checker_context, verdict
from tests.support.documents import docx

COVER = [["Student Name", "Student Name"], ["Student ID", "ID"]]
TABLE_10 = [["ID", "Requirement", "Priority"], ["R1", "Show the queue position", "Must"]]
IDENTITY = task("put Kartikey and 2024eb02238 in the HCI assignment",
                conds=(cond("The assignment uses full name Kartikey and student ID 2024eb02238"),))
RECORD = re.compile(r"change record: task\.evidence span_id=([0-9A-Za-z-]+)")


def install(daemon: Any, workers: dict[str, Any], plan: dict[str, Any] = IDENTITY) -> tuple[Script, list[Any]]:
    """Scripted runtimes that also note the reasoning effort each run was made with (None: the model's default)."""
    script = Script([sup("task", "On it.", plan)], workers)
    efforts: list[tuple[str, str | None]] = []

    def make(role: str, *, mode: str, tool_names: list[str], schema: dict[str, Any],
             effort: str | None = None) -> FakeRuntime:
        efforts.append((role, effort))
        return FakeRuntime(script, mode, tool_names)
    daemon.daemon.supervisor.make_runtime = make
    return script, efforts


def checks(script: Script) -> list[str]:
    return [c for _, key, c, _ in script.contexts if key.endswith(".verify")]


def _assignment(tmp_path: Path) -> Path:
    folder = tmp_path / "HCI ASSIGNMENT"
    folder.mkdir()
    return docx(folder / "Student_Name_ID_HCI_Assignment1.docx",
                ["Human Computer Interaction", COVER, "Table 10. Requirements", TABLE_10])


def _perl(doc: Path) -> str:
    """The live case's replace (span 780bd068): every `<w:t ...>ID</w:t>` becomes the student id."""
    return (f"set -eu\nf='{doc}'\nw=$(mktemp -d)\ncd \"$w\" && unzip -qq \"$f\"\n"
            "perl -0pi -e 's{<w:t xml:space=\"preserve\">ID</w:t>}{<w:t xml:space=\"preserve\">2024eb02238</w:t>}g' "
            "word/document.xml\nrm \"$f\" && zip -q -X -r \"$f\" .\n")


def test_a_shell_replace_that_hits_a_table_header_reaches_the_checker_and_a_cited_line_fails_the_step(
        daemon: Any, tmp_path: Path) -> None:
    doc = _assignment(tmp_path)

    def worker(token: str) -> str:
        r = tool(daemon.sock, token, "shell.run", command=_perl(doc), effect="edit")
        assert not r["is_error"], r
        return done("Replaced the ID placeholder with 2024eb02238; matching identity values.")

    def checker(token: str) -> str:
        context = checker_context(script)
        [record] = RECORD.findall(context)
        page = tool(daemon.sock, token, "task.evidence", span_id=record)
        assert not page["is_error"], page
        assert "+2024eb02238\\n Requirement" in page["content"]["text"]
        assert "-ID\n+2024eb02238\n Requirement" in context          # the diff is in the check's context, as data
        return verdict(False, "Table 10's column header 'ID' became '2024eb02238' (diff line '+2024eb02238', "
                              "beside 'Requirement'): an unrequested change.")
    script, _ = install(daemon, {"worker": worker, "worker.verify": checker})
    out = outcome(chat(daemon, "unzip the HCI assignment, fullname=Kartikey, studentid=2024eb02238"))
    assert not out["verified"] and out["status"] == "failed"
    assert "Table 10's column header" in out["answer"]
    context = checks(script)[0]
    assert f"{doc.resolve()} (changed)" in context
    assert "<untrusted-data" in context.split("Files this step changed", 1)[1][:600]        # MP2: shown as data


def test_a_pass_with_no_look_at_a_changed_file_is_refused_rerun_once_then_unclear(daemon: Any, tmp_path: Path) -> None:
    notes = tmp_path / "notes.txt"
    notes.write_text("Student ID: ID\n")

    def worker(token: str) -> str:
        r = tool(daemon.sock, token, "fs.write", path=str(notes), content="Student ID: 2024eb02238\n",
                 overwrite=True, effect="edit")
        assert not r["is_error"], r
        return done("Set the student id.")
    script, _ = install(daemon, {"worker": worker, "worker.verify": verdict(True, "The report says so.")})
    out = outcome(chat(daemon, "set my student id to 2024eb02238 in notes.txt"))
    contexts = checks(script)
    assert len(contexts) == 2                                       # refused once, run once more with the reason
    assert "FERB refused this check's pass" in contexts[1] and str(notes.resolve()) in contexts[1]
    assert not out["verified"]
    met = [r["met"] for r in out["verification"]]
    assert met and all(m is None for m in met)                      # never a pass
    assert "without reading" in json.dumps(out["verification"])


def test_a_pass_that_read_the_changed_file_stands(daemon: Any, tmp_path: Path) -> None:
    notes = tmp_path / "notes.txt"
    notes.write_text("Student ID: ID\n")

    def worker(token: str) -> str:
        assert not tool(daemon.sock, token, "fs.write", path=str(notes), content="Student ID: 2024eb02238\n",
                        overwrite=True, effect="edit")["is_error"]
        return done("Set the student id.")

    def checker(token: str) -> str:
        r = tool(daemon.sock, token, "fs.read", path=str(notes))
        assert "2024eb02238" in r["content"]["text"]
        return verdict(True, "notes.txt now reads 'Student ID: 2024eb02238'.")
    script, efforts = install(daemon, {"worker": worker, "worker.verify": checker})
    out = outcome(chat(daemon, "set my student id to 2024eb02238 in notes.txt"))
    assert out["verified"] and len(checks(script)) == 1
    assert ("checker", "high") in efforts                           # a file-changing step's checker: high effort


def test_a_check_of_a_step_that_changed_no_file_is_unchanged(daemon: Any) -> None:
    script, efforts = install(daemon, {"worker": done("It is 64%."), "worker.verify": verdict(True)},
                              task("read the battery level", conds=(cond("The battery level is reported"),)))
    out = outcome(chat(daemon, "what's my battery level"))
    assert out["verified"] and len(checks(script)) == 1
    assert ("checker", None) in efforts and ("checker", "high") not in efforts
    assert "Files this step changed" not in checks(script)[0]
