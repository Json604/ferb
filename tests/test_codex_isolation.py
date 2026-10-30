"""Codex runs with its own HOME so the owner's ~/.agents/skills never load (D32 note; verified live 2026-10-07)."""
import asyncio
import stat
from pathlib import Path

import pytest

import ferb.runtimes.codex_exec as cx
from ferb.core.types import Identity, ModelRequest, Task

# Save the original before the no_real_models fixture patches it (module code runs before fixtures)
_original_codex_start = cx.CodexRuntime.start


def test_skill_dependency_install_is_disabled() -> None:
    assert "skill_mcp_dependency_install" in cx.DISABLED


@pytest.mark.anyio
async def test_codex_process_gets_ferbs_own_home(tmp_path: Path, monkeypatch) -> None:
    seen: dict = {}

    async def fake_exec(*cmd, **kw):
        seen.update(kw)
        raise RuntimeError("stop before running anything")
    async def no_version(binary):
        return None
    monkeypatch.setattr(cx, "check_version", no_version)
    monkeypatch.setattr(asyncio, "create_subprocess_exec", fake_exec)
    # Restore the real start method before fixture patches it (test's monkeypatch takes precedence)
    monkeypatch.setattr(cx.CodexRuntime, "start", _original_codex_start)
    (tmp_path / "codex-home").mkdir()
    (tmp_path / "codex-home" / "auth.json").write_text("{}")
    rt = cx.CodexRuntime(Path("/x/codex"), "supervisor", tmp_path, output_schema={"type": "object"})
    task = Task("t", None, "chat", "dev", Identity.OWNER, set(), "mixed", "r", [], None)
    with pytest.raises(RuntimeError):
        await rt.start(task, ModelRequest("sys", ("ctx",), 0), "")
    home = Path(seen["env"]["HOME"])
    assert home == tmp_path / cx.CODEX_USER and home.is_dir()
    assert stat.S_IMODE(home.stat().st_mode) == 0o700 and not any(home.iterdir())
