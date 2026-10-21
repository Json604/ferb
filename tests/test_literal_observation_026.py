"""Unavailable literal file reads remain evidence; they must not prevent outcome judgment."""

import asyncio
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import AsyncMock

import pytest

from ferb.core.types import Identity, Task
from ferb.orchestrator.schemas import Check, Condition
from ferb.orchestrator.supervisor import Supervisor
from ferb.orchestrator.verify import Checks


@pytest.mark.parametrize("failure", ["invalid_utf8", "permission"])
def test_unreadable_literal_predicate_still_reaches_checker(tmp_path, monkeypatch, failure):
    artifact = tmp_path / "report.dat"
    artifact.write_bytes(b"Expected text\n\xff" if failure == "invalid_utf8" else b"Expected text")
    if failure == "permission":
        original = Path.open

        def open_file(self, *args, **kwargs):
            if self == artifact:
                raise PermissionError("Fixture denies this read")
            return original(self, *args, **kwargs)

        monkeypatch.setattr(Path, "open", open_file)
    conditions = [Condition(text="Requested content is present", check=Check(
        kind="file_contains", path=str(artifact), text="Expected text", url_contains=None, site=None, expected=None)),
        Condition(text="Artifact exists", check=Check(kind="file_exists", path=str(artifact), text=None,
                                                     url_contains=None, site=None, expected=None))]
    supervisor = Supervisor.__new__(Supervisor)
    supervisor.device_id = "test"
    supervisor.roots = SimpleNamespace(workspace=str(tmp_path), owner_roots=(), untrusted_roots=())
    supervisor.checks = Checks(lambda _: [])
    supervisor._verifier = AsyncMock(return_value=[])
    worker = Task("task.worker", "task", "Inspect artifact", "test", Identity.FERB, set(), "worker", "request", [], None)
    tree = SimpleNamespace(worker=worker, spec=SimpleNamespace(success_conditions=conditions), watch=False,
                           watch_met=None)

    asyncio.run(supervisor.verify(tree, "parent-span"))

    supervisor._verifier.assert_awaited_once()
    observations = supervisor._verifier.call_args.args[4]
    assert [observation.index for observation in observations] == [0, 1]
    assert observations[0].met is None
    assert "unavailable" in observations[0].evidence.lower()
    assert ("UnicodeDecodeError" if failure == "invalid_utf8" else "PermissionError") in observations[0].evidence
    assert observations[1].met is True
