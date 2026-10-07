# tests/test_api_keys.py
"""Provider API keys live in the Keychain (service ferb, account api/<provider>) and nowhere else (D32, rule 14)."""
import os
import stat
from pathlib import Path

import pytest

from ferb.runtimes import keys


@pytest.fixture
def fake_security(tmp_path: Path, monkeypatch):
    script = tmp_path / "security"
    script.write_text("#!/bin/sh\n"
                      'if [ "$5" = "api/openai" ]; then [ "$6" = "-w" ] && echo "sk-test-123"; exit 0; fi\n'
                      "exit 44\n")
    script.chmod(script.stat().st_mode | stat.S_IEXEC)
    monkeypatch.setenv("PATH", f"{tmp_path}{os.pathsep}{os.environ['PATH']}")


@pytest.mark.anyio
async def test_reads_the_key_from_the_keychain_account(fake_security) -> None:
    assert await keys.api_key("openai") == "sk-test-123"
    assert await keys.api_key("anthropic") is None


def test_presence_check_never_reveals(fake_security) -> None:
    assert keys.has_api_key("openai") and not keys.has_api_key("google")


def test_unknown_provider_is_refused() -> None:
    with pytest.raises(ValueError):
        keys.account("someone")
