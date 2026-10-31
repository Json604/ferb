"""L39: the before-and-after record of a task's file changes (ferb/act/fs/changes.py), from the live case (task
39e35d57): a shell perl replace that also turned a table header "ID" into the student id, a PDF "corrected" by boxes
drawn over its text, and a corrected copy saved beside its original."""

import os
import subprocess
from pathlib import Path

import pytest

from ferb.act.fs.changes import Change, Changes, describe, named, text_of
from tests.support.documents import docx, overlay, pdf

COVER = [["Student Name", "Student Name"], ["Student ID", "ID"]]
TABLE_10 = [["ID", "Requirement", "Priority"], ["R1", "Show the queue position", "Must"]]


def _assignment(folder: Path) -> Path:
    return docx(folder / "Student_Name_ID_HCI_Assignment1.docx",
                ["Human Computer Interaction", COVER, "Table 10. Requirements", TABLE_10])


def _store(tmp_path: Path) -> Changes:
    return Changes(tmp_path / "changes")


def test_named_reads_paths_from_the_live_perl_command(tmp_path: Path) -> None:
    base = tmp_path / "HCI ASSIGNMENT"
    command = (f"set -eu\nbase='{base}'\nsrc=\"$base/Student_Name_ID_HCI_Assignment1.docx\"\n"
               "out=\"$base/Sam_2031xy04417_HCI_Assignment1.docx\"\nwork=$(mktemp -d)\n"
               "unzip -qq \"$src\" -d \"$work\"\nperl -0pi -e 's{x}{y}g' \"$work/word/document.xml\"\n")
    paths = named(command, tmp_path / "workspace")
    assert base in paths
    assert base / "Student_Name_ID_HCI_Assignment1.docx" in paths
    assert base / "Sam_2031xy04417_HCI_Assignment1.docx" in paths
    assert not any("work" in str(p) and "document.xml" in str(p) for p in paths)   # $(mktemp) is unknown


def test_named_reads_quoted_code_cd_and_globs(tmp_path: Path) -> None:
    home = tmp_path / "home"
    assert named('set p to POSIX file "/Users/k/a b/x.pdf"', tmp_path) == [Path("/Users/k/a b/x.pdf")]
    assert Path("/tmp/x.txt") in named("python3 -c \"open('/tmp/x.txt','w').write('hi')\"", tmp_path)
    found = named("cd ~/proj && sed -i '' 's/a/b/' notes.txt src/*.c", tmp_path, home)
    assert home / "proj" / "notes.txt" in found and home / "proj" / "src" in found
    assert named("echo hello world", tmp_path) == []


def test_a_shell_perl_replace_that_hits_a_table_header_shows_in_the_diff(tmp_path: Path) -> None:
    """The live case: every `<w:t>ID</w:t>` replaced, the cover's value cell and Table 10's header both."""
    folder = tmp_path / "HCI ASSIGNMENT"
    folder.mkdir()
    doc = _assignment(folder)
    store = _store(tmp_path)
    command = (f"set -eu\nf='{doc}'\nw=$(mktemp -d)\ncd \"$w\" && unzip -qq \"$f\"\n"
               "perl -0pi -e 's{<w:t xml:space=\"preserve\">ID</w:t>}{<w:t xml:space=\"preserve\">2031xy04417</w:t>}g' "
               "word/document.xml\nrm \"$f\" && zip -q -X -r \"$f\" .\n")
    counts = store.before("t.step.one", "t", named(command, tmp_path))
    assert counts["copied"] == 1
    subprocess.run(["/bin/sh", "-c", command], check=True, capture_output=True)
    changes = store.changes("t.step.one")
    assert [(c.path, c.what) for c in changes] == [(str(doc.resolve()), "modified")]
    text = describe(changes[0])
    assert "changed, named by this step" in text
    assert "-ID\n+2031xy04417" in text                     # the cover's value cell
    assert "+2031xy04417\n Requirement" in text           # and Table 10's column header: both visible to the checker


def test_a_corrected_copy_beside_its_original_shows_as_its_changes_from_it(tmp_path: Path) -> None:
    folder = tmp_path / "HCI ASSIGNMENT"
    folder.mkdir()
    original = _assignment(folder)
    store = _store(tmp_path)
    made = folder / "Sam_2031xy04417_HCI_Assignment1.docx"
    store.before("t.step.one", "t", [folder, original, made])      # the live command named both (src=, out=)
    docx(made,
         ["Human Computer Interaction", [["Student Name", "Sam"], ["Student ID", "2031xy04417"]],
          "Table 10. Requirements", [["2031xy04417", "Requirement", "Priority"], TABLE_10[1]]], creator="Sam")
    store.before("t.step.one", "t", [original])          # fs.trash names it: already copied, nothing new
    original.unlink()                                      # (moved to the Trash)
    changes = {Path(c.path).name: c for c in store.changes("t.step.one")}
    made = changes["Sam_2031xy04417_HCI_Assignment1.docx"]
    assert made.what == "created" and made.source == str(original.resolve())
    text = describe(made)
    assert f"compared with {original.resolve()}" in text
    assert "-ID\n+2031xy04417\n Requirement" in text
    assert "-[document property] creator: Student Name\n+[document property] creator: Sam" in text
    assert changes[original.name].what == "removed" and changes[original.name].content


def test_a_pdf_covered_by_boxes_shows_annotations_added_and_its_text_layer_unchanged(tmp_path: Path) -> None:
    folder = tmp_path / "NP"
    folder.mkdir()
    doc = pdf(folder / "StudentID_StudentName_NP_Assignment.pdf", ["Student Name: StudentName", "Student ID: IdNumber"])
    store = _store(tmp_path)
    store.before("t.step.two", "t", [doc])
    covered = overlay(doc, folder / "covered.pdf", "Sam")
    os.replace(covered, doc)
    [change] = store.changes("t.step.two")
    text = describe(change)
    assert "2 annotations added" in text and 'FreeText "Sam"' in text and "Square" in text
    assert "text layer unchanged" in text
    assert text_of(doc) is not None and "StudentName" in text_of(doc)[0]       # the placeholder is still there


def test_untouched_and_rewritten_with_the_same_bytes_are_no_change(tmp_path: Path) -> None:
    f = tmp_path / "notes.txt"
    f.write_text("same\n")
    store = _store(tmp_path)
    store.before("t", "t", [f])
    f.write_text("same\n")                                 # touched: new time, same bytes
    assert store.changes("t") == []


def test_plain_text_edit_and_a_new_file_in_a_listed_folder(tmp_path: Path) -> None:
    f = tmp_path / "http_client.c"
    f.write_text("int fuction(void);\nint main(void);\n")
    store = _store(tmp_path)
    store.before("t", "t", [f])
    f.write_text("int function(void);\nint main(void);\n")
    (tmp_path / "out.log").write_text("built\n")
    changes = {Path(c.path).name: c for c in store.changes("t")}
    assert "-int fuction(void);\n+int function(void);" in describe(changes["http_client.c"])
    # a file the step didn't name, beside one it did: a fact (name, size, time), never its text (L39 I1)
    assert changes["out.log"].what == "created" and not changes["out.log"].content
    line = describe(changes["out.log"])
    assert "new during this step, 6 bytes" in line and "built" not in line


def test_nothing_is_copied_from_a_closed_store_or_a_trash(tmp_path: Path) -> None:
    secret = tmp_path / "secrets" / "keys.txt"
    secret.parent.mkdir()
    secret.write_text("k")
    trash = tmp_path / ".Trash" / "old.txt"
    trash.parent.mkdir()
    trash.write_text("o")
    store = Changes(tmp_path / "changes", closed=lambda p: "secrets" in p.parts)
    assert store.before("t", "t", [secret, trash, secret.parent]) == {"copied": 0, "not_copied": 0, "folders": 0,
                                                                       "protected": 3}


def test_a_record_survives_a_restart_and_goes_with_its_tree(tmp_path: Path) -> None:
    f = tmp_path / "a.txt"
    f.write_text("one\n")
    Changes(tmp_path / "changes").before("t.step.a", "t", [f])
    f.write_text("two\n")
    again = Changes(tmp_path / "changes")
    [change] = again.changes("t.step.a", "t")
    assert "-one\n+two" in describe(change)
    again.forget("t")
    assert not any((tmp_path / "changes").iterdir())


def test_a_loop_round_starts_its_own_record(tmp_path: Path) -> None:
    f = tmp_path / "a.txt"
    f.write_text("one\n")
    store = _store(tmp_path)
    store.before("t.loop.x", "t", [f])
    f.write_text("two\n")
    store.reset("t.loop.x")
    store.before("t.loop.x", "t", [f])
    f.write_text("three\n")
    [change] = store.changes("t.loop.x")
    assert "-two\n+three" in describe(change)


@pytest.mark.parametrize("what", ["removed", "created"])
def test_a_change_with_no_copy_says_so(tmp_path: Path, what: str) -> None:
    text = describe(Change(str(tmp_path / "gone.bin"), what, note="FERB kept no copy from before (it wasn't named)"))
    assert str(tmp_path / "gone.bin") in text
