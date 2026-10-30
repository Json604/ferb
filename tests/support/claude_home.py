"""A FERB home whose own Claude sign-in finished (the marker only; no credential), for Claude adapter tests: FERB's
Claude runtime never starts without it (no fallback to the owner's ~/.claude)."""
import tempfile
from pathlib import Path


def signed_in_home(root: Path | None = None) -> str:
    from ferb.runtimes.claude_login import mark
    home = root if root is not None else Path(tempfile.mkdtemp(prefix="ferb-claude-test-"))
    mark(home)
    return str(home)
