"""A release can't skip its version bump or ship before the Web Store has the extension it needs (owner, 2026-11-01:
"make sure there is a systematic way that it happens anyway"). Also: the version lives in two Rust files that agree.
Ported from tests/test_release_checks.py to stdlib unittest; the repo's extension against the engine's minimum is the
Rust engine's own test now."""

from __future__ import annotations

import importlib.util
import unittest

from scripts.tests.support import CRATES, ROOT, SCRIPTS, rust_const, workspace_version

_spec = importlib.util.spec_from_file_location("release_checks", SCRIPTS / "release_checks.py")
assert _spec and _spec.loader
rc = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(rc)


class ReleaseChecks(unittest.TestCase):
    # decision: REL-2
    def test_a_bumped_release_with_the_extension_published_passes(self) -> None:
        self.assertEqual(rc.problems("0.28.0", "v0.28.0", ["v0.1", "v0.27.0"], "0.19.0", "0.19.0"), [])

    # decision: REL-2
    def test_a_development_version_is_refused(self) -> None:
        found = rc.problems("0.27.0.dev1", "v0.27.0", [], "0.19.0", "0.19.0")
        self.assertTrue(any("development version" in p for p in found), found)
        self.assertTrue(any("engine/crates/ferbd/src/compose/mod.rs" in p and "engine/Cargo.toml" in p for p in found))

    # decision: REL-2
    def test_a_version_not_newer_than_the_last_release_is_refused(self) -> None:
        self.assertTrue(any("isn't newer" in p for p in rc.problems("0.27.0", "v0.27.0", ["v0.27.0"], "0.19.0", "0.19.0")))
        self.assertTrue(any("isn't newer" in p for p in rc.problems("0.26.9", "v0.26.9", ["v0.27.0"], "0.19.0", "0.19.0")))

    # decision: REL-2
    def test_the_tag_must_name_the_version(self) -> None:
        self.assertTrue(any("doesn't name" in p for p in rc.problems("0.28.0", "v0.28.1", [], "0.19.0", "0.19.0")))

    # decision: REL-2
    def test_an_older_store_extension_or_an_unknown_one_is_refused(self) -> None:
        older = rc.problems("0.28.0", "v0.28.0", [], "0.15.13", "0.19.0")
        self.assertTrue(any("Web Store serves extension 0.15.13" in p for p in older), older)
        self.assertTrue(any("ferb browsers extension package --out build/store" in p for p in older), older)
        self.assertTrue(any("couldn't read" in p for p in rc.problems("0.28.0", "v0.28.0", [], "", "0.19.0")))

    # decision: REL-2
    def test_the_command_exits_nonzero_on_a_problem(self) -> None:
        self.assertEqual(rc.main(["--version", "0.28.0", "--tag", "v0.28.0", "--store-extension", "0.19.0",
                                  "--min-extension", "0.19.0"]), 0)
        self.assertEqual(rc.main(["--version", "0.28.0.dev1", "--tag", "v0.28.0", "--store-extension", "0.19.0",
                                  "--min-extension", "0.19.0"]), 1)

    # decision: REL-2, REL-425
    def test_the_version_is_the_same_in_both_files(self) -> None:
        """ferbd's VERSION (what FERB.app and the engine report) equals engine/Cargo.toml's workspace version."""
        version = rust_const(CRATES / "ferbd/src/compose/mod.rs", "VERSION")
        self.assertEqual(version, workspace_version())
        self.assertFalse((ROOT / "engine/crates/ferbd/src/compose/mod.rs").read_text().count('pub const VERSION:') != 1)

    def test_the_release_checks_name_no_python_source(self) -> None:
        text = (SCRIPTS / "release_checks.py").read_text()
        for gone in ("ferb/__init__.py", "pyproject.toml", "uv run"):
            self.assertNotIn(gone, text)


if __name__ == "__main__":
    unittest.main()
