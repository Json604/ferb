#!/usr/bin/env python3
"""The kb registry's `historical` setting (kb/config.toml): a decision or area path naming a file that was removed
from the repository is checked against the commit named there, and only then. Runs kb.py in throwaway git
repositories; never touches this one. Run: python3 -I -B tools/kb/test_kb_historical.py  (exit 0 = all passed)."""

from __future__ import annotations

import importlib.util
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
_spec = importlib.util.spec_from_file_location("kb_under_test", HERE / "kb.py")
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
paths = ["src", "old.py"]
"""

DECISION = """[CORE-1]
title = "A rule about removed code"
by = "agent"
date = 2026-11-15
status = "active"
rule = "The removed module did it."
untestable = "an example"
paths = [{paths}]
"""


class Repo:
    def __init__(self, root: Path):
        self.root = root
        self.git("init", "-q", "-b", "main")
        (root / "kb/decisions").mkdir(parents=True)
        (root / "kb/areas.toml").write_text(AREAS)
        (root / "src").mkdir()
        (root / "src/keep.txt").write_text("stays\n")
        (root / "src/gone.py").write_text("def removed_function():\n    return 1\n\nVALUE = 2\n")
        (root / "src/gone_too.py").write_text("x = 1\n")
        (root / "old.py").write_text("y = 1\n")
        self.git("add", "-A")
        self.git("commit", "-q", "-m", "with the Python files")
        self.before = self.git("rev-parse", "HEAD").strip()
        self.git("rm", "-q", "src/gone.py", "src/gone_too.py", "old.py")
        self.git("commit", "-q", "-m", "the Python files removed")

    def git(self, *args: str) -> str:
        return subprocess.run(["git", *args], cwd=self.root, env=ENV, check=True, capture_output=True,
                              text=True).stdout

    def problems(self, paths: list[str], historical: str | None) -> list[str]:
        (self.root / "kb/decisions/core.toml").write_text(
            DECISION.format(paths=", ".join(f'"{p}"' for p in paths)))
        cfg = "[kb]\ncoverage = false\nownership = false\n"
        if historical is not None:
            cfg += historical
        (self.root / "kb/config.toml").write_text(cfg)
        found = kb.KB(self.root).check()
        # only what the setting is about: the paths' verdicts and the setting's own validation
        return [e for e in found if "path `" in e or "historical" in e]


class Historical(unittest.TestCase):
    def setUp(self) -> None:
        self.tmp = tempfile.TemporaryDirectory()
        self.repo = Repo(Path(self.tmp.name))
        self.hist = (f'historical = [{{commit = "{self.repo.before}", paths = ["src/gone.py", "src/gone_too.py", '
                     f'"old.py"], why = "removed"}}]\n')

    def tearDown(self) -> None:
        self.tmp.cleanup()

    # decision: PROJ-509
    def test_without_the_setting_a_removed_path_is_an_error(self) -> None:
        got = self.repo.problems(["src/gone.py", "src/keep.txt"], None)
        self.assertEqual(len(got), 2, got)          # the decision's path and the area's old.py
        self.assertTrue(any("src/gone.py` is not a tracked file" in e for e in got), got)

    # decision: PROJ-509, ENG-16
    def test_a_removed_file_folder_glob_and_symbol_are_read_at_the_commit(self) -> None:
        paths = ["src/gone.py", "src/gone.py::removed_function", "src/gone.py::VALUE", "src/*.py",
                 "src/gone_too.py", "src/keep.txt"]
        self.assertEqual(self.repo.problems(paths, self.hist), [])

    # decision: PROJ-509
    def test_a_symbol_the_old_file_never_had_is_still_an_error(self) -> None:
        got = self.repo.problems(["src/gone.py::never_there"], self.hist)
        self.assertEqual(len(got), 1, got)
        self.assertIn("has no symbol never_there", got[0])

    # decision: PROJ-509
    def test_a_path_outside_the_historical_paths_is_not_rescued(self) -> None:
        got = self.repo.problems(["src/never_existed.py", "lib/other.py"], self.hist)
        self.assertEqual(len(got), 2, got)
        got = self.repo.problems(["src/keep.txt.bak"], self.hist)   # never in the listed paths, nor at the commit
        self.assertEqual(len(got), 1, got)

    # decision: PROJ-509
    def test_the_current_tree_wins(self) -> None:
        # src/keep.txt exists now: a symbol it lacks is an error even if the commit's copy had it
        (self.repo.root / "src/keep.txt").write_text("nothing here\n")
        self.repo.git("commit", "-q", "-am", "keep changed")
        hist = (f'historical = [{{commit = "{self.repo.before}", paths = ["src", "old.py"], why = "removed"}}]\n')
        got = self.repo.problems(["src/keep.txt::stays"], hist)
        self.assertEqual(len(got), 1, got)
        self.assertIn("has no symbol stays", got[0])

    # decision: PROJ-509
    def test_the_setting_is_validated(self) -> None:
        bad = {
            'historical = [{commit = "0000000000000000000000000000000000000000", paths = ["old.py"], why = "x"}]\n':
                "does not resolve",
            f'historical = [{{commit = "{self.repo.before}", paths = [], why = "x"}}]\n': "paths",
            f'historical = [{{commit = "{self.repo.before}", paths = ["old.py"]}}]\n': "why",
            f'historical = [{{commit = "{self.repo.before}", paths = ["old.py"], why = "x", note = "y"}}]\n':
                "unknown",
            'historical = ["HEAD"]\n': "table",
        }
        for cfg, want in bad.items():
            got = [e for e in self.repo.problems(["src/keep.txt"], cfg) if "historical" in e]
            self.assertTrue(got and any(want in e for e in got), (cfg, got))

    # decision: PROJ-509
    def test_show_names_the_commit_a_path_was_removed_at(self) -> None:
        self.repo.problems(["src/gone.py"], self.hist)
        k = kb.KB(self.repo.root)
        self.assertEqual(k.removed_at("src/gone.py"), self.repo.before)
        self.assertIsNone(k.removed_at("src/keep.txt"))
        self.assertIsNone(k.removed_at("src/never_existed.py"))


if __name__ == "__main__":
    sys.dont_write_bytecode = True
    unittest.main()
