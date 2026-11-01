"""A release can't skip its version bump or ship before the Web Store has the extension it needs (owner, 2026-11-01:
"make sure there is a systematic way that it happens anyway"). Also: the version lives in two files that agree, and the
extension in the repo is one this FERB accepts."""

import importlib.util
import json
import re
import tomllib
from pathlib import Path

ROOT = Path(__file__).parents[1]
_spec = importlib.util.spec_from_file_location("release_checks", ROOT / "scripts/release_checks.py")
assert _spec and _spec.loader
rc = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(rc)


def test_a_bumped_release_with_the_extension_published_passes() -> None:
    assert rc.problems("0.28.0", "v0.28.0", ["v0.1", "v0.27.0"], "0.19.0", "0.19.0") == []


def test_a_development_version_is_refused() -> None:
    assert any("development version" in p for p in rc.problems("0.27.0.dev1", "v0.27.0", [], "0.19.0", "0.19.0"))


def test_a_version_not_newer_than_the_last_release_is_refused() -> None:
    assert any("isn't newer" in p for p in rc.problems("0.27.0", "v0.27.0", ["v0.27.0"], "0.19.0", "0.19.0"))
    assert any("isn't newer" in p for p in rc.problems("0.26.9", "v0.26.9", ["v0.27.0"], "0.19.0", "0.19.0"))


def test_the_tag_must_name_the_version() -> None:
    assert any("doesn't name" in p for p in rc.problems("0.28.0", "v0.28.1", [], "0.19.0", "0.19.0"))


def test_an_older_store_extension_or_an_unknown_one_is_refused() -> None:
    assert any("Web Store serves extension 0.15.13" in p
               for p in rc.problems("0.28.0", "v0.28.0", [], "0.15.13", "0.19.0"))
    assert any("couldn't read" in p for p in rc.problems("0.28.0", "v0.28.0", [], "", "0.19.0"))


def test_the_command_exits_nonzero_on_a_problem() -> None:
    assert rc.main(["--version", "0.28.0", "--tag", "v0.28.0", "--store-extension", "0.19.0",
                    "--min-extension", "0.19.0"]) == 0
    assert rc.main(["--version", "0.28.0.dev1", "--tag", "v0.28.0", "--store-extension", "0.19.0",
                    "--min-extension", "0.19.0"]) == 1


def test_the_version_is_the_same_in_both_files() -> None:
    init = re.search(r'^__version__ = "(.*)"$', (ROOT / "ferb/__init__.py").read_text(), re.M)
    assert init and tomllib.loads((ROOT / "pyproject.toml").read_text())["project"]["version"] == init.group(1)


def test_the_extension_in_the_repo_is_one_this_ferb_accepts() -> None:
    from ferb.act.browser.ext_protocol import MIN_EXTENSION_VERSION, version_tuple
    manifest = json.loads((ROOT / "extensions/chromium/manifest.json").read_text())
    assert version_tuple(manifest["version"]) >= version_tuple(MIN_EXTENSION_VERSION)
