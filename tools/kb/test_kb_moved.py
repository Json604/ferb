#!/usr/bin/env python3
"""The kb registry's `moved` setting (kb/config.toml): a decision or area path under a folder that moved is read at
its new place in the current tree, before `historical`; `kb show` and the generated rules follow it. Runs kb.py in
throwaway git repositories; never touches this one. Run: python3 -I -B tools/kb/test_kb_moved.py  (exit 0 = passed)."""

from __future__ import annotations

import contextlib
import importlib.util
import io
import os
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

# a hook's GIT_DIR / GIT_INDEX_FILE would point kb.py's git calls at this repository instead of the throwaway ones
for _k in [k for k in os.environ if k.startswith("GIT_")]:
    del os.environ[_k]

HERE = Path(__file__).resolve().parent
_spec = importlib.util.spec_from_file_location("kb_moved_under_test", HERE / "kb.py")
assert _spec and _spec.loader
kb = importlib.util.module_from_spec(_spec)
sys.modules[_spec.name] = kb                 # dataclasses look the module up while it loads
_spec.loader.exec_module(kb)

ENV = dict(os.environ, GIT_CONFIG_GLOBAL=os.devnull, GIT_CONFIG_NOSYSTEM="1", GIT_AUTHOR_NAME="Test",
           GIT_AUTHOR_EMAIL="t@example.com", GIT_COMMITTER_NAME="Test", GIT_COMMITTER_EMAIL="t@example.com",
           GIT_TERMINAL_PROMPT="0")

AREAS = """[core]
title = "Core"
prefix = "CORE"
paths = [{paths}]
"""

DECISION = """[CORE-1]
title = "A rule about code that moved"
by = "agent"
date = 2026-11-16
status = "active"
rule = "The moved module does it."
untestable = "an example"
paths = [{paths}]
"""

MOVED = ('moved = [{from = "pkg", to = "engine/runtime", why = "moved"}, '
         '{from = "pkg/sub", to = "elsewhere", why = "moved apart"}]\n')


def toml_list(paths: list[str]) -> str:
    return ", ".join(f'"{p}"' for p in paths)


class Repo:
    """At `before`: pkg/{keep.md, code.py, gone.py, sub/deep.txt}. Then gone.py is removed and pkg moves:
    pkg -> engine/runtime (keep.md, code.py) and pkg/sub -> elsewhere (deep.txt)."""

    def __init__(self, root: Path):
        self.root = root
        self.git("init", "-q", "-b", "main")
        (root / "kb/decisions").mkdir(parents=True)
        (root / "pkg/sub").mkdir(parents=True)
        (root / "pkg/keep.md").write_text("# kept\n")
        (root / "pkg/code.py").write_text("def moved_function():\n    return 1\n\ndef dropped_since():\n    return 2\n")
        (root / "pkg/gone.py").write_text("def removed_function():\n    return 1\n")
        (root / "pkg/sub/deep.txt").write_text("deep\n")
        self.git("add", "-A")
        self.git("commit", "-q", "-m", "the old layout")
        self.before = self.git("rev-parse", "HEAD").strip()
        self.git("rm", "-q", "pkg/gone.py")
        (root / "engine").mkdir()
        self.git("mv", "pkg/sub", "elsewhere")
        self.git("mv", "pkg", "engine/runtime")
        (root / "engine/runtime/code.py").write_text("def moved_function():\n    return 1\n")
        self.git("add", "-A")
        self.git("commit", "-q", "-m", "the new layout")

    def git(self, *args: str) -> str:
        return subprocess.run(["git", *args], cwd=self.root, env=ENV, check=True, capture_output=True,
                              text=True).stdout

    def kb(self, paths: list[str], extra: str = "", area: list[str] | None = None) -> "kb.KB":
        (self.root / "kb/decisions/core.toml").write_text(DECISION.format(paths=toml_list(paths)))
        (self.root / "kb/areas.toml").write_text(AREAS.format(paths=toml_list(area or ["engine", "elsewhere"])))
        (self.root / "kb/config.toml").write_text("[kb]\ncoverage = false\nownership = false\n" + extra)
        return kb.KB(self.root)

    def problems(self, paths: list[str], extra: str = "", area: list[str] | None = None) -> list[str]:
        found = self.kb(paths, extra, area).check()
        # only what the settings are about: the paths' verdicts and the settings' own validation
        return [e for e in found if "path `" in e or "moved" in e or "historical" in e]


class Moved(unittest.TestCase):
    def setUp(self) -> None:
        self.tmp = tempfile.TemporaryDirectory()
        self.repo = Repo(Path(self.tmp.name).resolve())   # as git rev-parse gives it (/private/var)
        self.hist = (f'historical = [{{commit = "{self.repo.before}", paths = ["pkg"], why = "removed"}}]\n')

    def tearDown(self) -> None:
        self.tmp.cleanup()

    # decision: PROJ-511
    def test_without_the_setting_a_moved_path_is_an_error(self) -> None:
        got = self.repo.problems(["pkg/keep.md", "engine/runtime/keep.md"], area=["pkg", "engine"])
        self.assertEqual(len(got), 2, got)            # the decision's pkg/keep.md and the area's pkg
        self.assertTrue(any("pkg/keep.md` is not a tracked file" in e for e in got), got)

    # decision: PROJ-511
    def test_a_moved_file_folder_glob_and_symbol_are_read_at_the_new_place(self) -> None:
        paths = ["pkg/keep.md", "pkg", "pkg/*.md", "pkg/code.py::moved_function", "pkg/sub/deep.txt", "pkg/sub"]
        self.assertEqual(self.repo.problems(paths, MOVED, area=["pkg", "pkg/sub"]), [])

    # decision: PROJ-511
    def test_the_longest_from_wins(self) -> None:
        k = self.repo.kb([], MOVED)
        self.assertEqual(k.moved("pkg/sub/deep.txt"), "elsewhere/deep.txt")
        self.assertEqual(k.moved("pkg/keep.md::x"), "engine/runtime/keep.md::x")
        self.assertEqual(k.moved("pkg/subway.txt"), "engine/runtime/subway.txt")   # a name's prefix is not a folder
        self.assertIsNone(k.moved("pkgs/other.txt"))
        self.assertEqual(k.moved_to("pkg/sub/deep.txt"), "elsewhere/deep.txt")
        self.assertIsNone(k.moved_to("engine/runtime/keep.md"))                    # tracked as written

    # decision: PROJ-511, PROJ-509
    def test_the_current_tree_wins_over_history(self) -> None:
        # code.py exists at its new place: a symbol it no longer has is an error, even though the old commit had it
        got = self.repo.problems(["pkg/code.py::dropped_since"], MOVED + self.hist)
        self.assertEqual(len(got), 1, got)
        self.assertIn("has no symbol dropped_since", got[0])
        self.assertIn("new place engine/runtime/code.py", got[0])

    # decision: PROJ-511, PROJ-509
    def test_a_path_removed_before_the_move_is_still_read_in_history(self) -> None:
        paths = ["pkg/gone.py", "pkg/gone.py::removed_function", "pkg/code.py::moved_function"]
        self.assertEqual(self.repo.problems(paths, MOVED + self.hist), [])
        got = self.repo.problems(["pkg/gone.py"], MOVED)                       # no history: nowhere to read it
        self.assertEqual(len(got), 1, got)
        k = self.repo.kb(["pkg/gone.py"], MOVED + self.hist)
        self.assertEqual(k.removed_at("pkg/gone.py"), self.repo.before)
        self.assertIsNone(k.removed_at("pkg/keep.md"))                         # moved, not removed
        self.assertIsNone(k.moved_to("pkg/gone.py"))

    # decision: PROJ-511
    def test_a_file_at_its_new_place_finds_the_records_that_name_its_old_place(self) -> None:
        k = self.repo.kb(["pkg/keep.md"], MOVED, area=["pkg", "pkg/sub"])
        self.assertEqual(k.governing("engine/runtime/keep.md"), ["CORE-1"])
        self.assertEqual(k.governing("elsewhere/deep.txt"), [])
        self.assertEqual(k.areas_of("elsewhere/deep.txt"), ["core"])
        out = io.StringIO()
        cwd = os.getcwd()
        try:
            os.chdir(self.repo.root)
            with contextlib.redirect_stdout(out):
                kb.cmd_show(k, "engine/runtime/keep.md")
                kb.cmd_show(k, "pkg/keep.md")
                kb.print_record(k, "CORE-1")
        finally:
            os.chdir(cwd)
        text = out.getvalue()
        self.assertEqual(text.count("CORE-1 (agent): The moved module does it."), 2, text)
        self.assertIn("moved: now engine/runtime/keep.md", text)
        self.assertIn("moved path: pkg/keep.md (now engine/runtime/keep.md", text)

    # decision: PROJ-511
    def test_the_generated_rules_name_the_new_places(self) -> None:
        k = self.repo.kb([], MOVED, area=["pkg", "pkg/sub/deep.txt", "engine/*"])
        rules = k.gen_rules()["kb-core.md"]
        for glob in ('"engine/runtime/**"', '"elsewhere/deep.txt"', '"engine/*"'):
            self.assertIn(glob, rules)
        self.assertNotIn('"pkg', rules)

    # decision: PROJ-511
    def test_the_setting_is_validated(self) -> None:
        bad = {
            'moved = "pkg"\n': "must be a list",
            'moved = ["pkg"]\n': "must be a table",
            'moved = [{from = "pkg", to = "engine/runtime", note = "x"}]\n': "unknown keys",
            'moved = [{to = "engine/runtime"}]\n': "`from` is required",
            'moved = [{from = "pkg/*", to = "engine/runtime"}]\n': "is a glob",
            'moved = [{from = "pkg", to = "pkg/"}]\n': "the same path",
            'moved = [{from = "pkg", to = "engine/runtime"}, {from = "pkg", to = "elsewhere"}]\n': "also moved[0]",
            'moved = [{from = "pkg", to = "nowhere"}]\n': "names nothing tracked now",
            # a cycle and a chain: each `to` must be the final place
            'moved = [{from = "engine", to = "elsewhere"}, {from = "elsewhere", to = "engine"}]\n': "a chain or a cycle",
            'moved = [{from = "pkg", to = "engine/runtime"}, {from = "engine", to = "elsewhere"}]\n': "a chain or a cycle",
            'moved = [{from = "pkg", to = "engine/runtime", why = ""}]\n': "`why`",
        }
        for cfg, want in bad.items():
            got = [e for e in self.repo.problems(["engine/runtime/keep.md"], cfg) if "moved" in e]
            self.assertTrue(got and any(want in e for e in got), (cfg, got))
        self.assertEqual(self.repo.problems(["engine/runtime/keep.md"], MOVED), [])


if __name__ == "__main__":
    sys.dont_write_bytecode = True
    unittest.main()
