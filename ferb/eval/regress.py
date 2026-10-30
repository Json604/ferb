"""Read a data-defined task set (the security catalogues in sets/). Must not infer a verdict: the owner's
`ferb verdict` is acceptance."""

import tomllib
from dataclasses import dataclass
from pathlib import Path


@dataclass(frozen=True)
class RegressTask:
    id: str
    prompt: str
    surface: str  # "disposable" | "owner"
    expect: str   # free text shown to the owner when asking for a verdict


def load_set(path: Path) -> list[RegressTask]:
    data = tomllib.loads(path.read_text())
    return [RegressTask(t["id"], t["prompt"], t.get("surface", "disposable"), t.get("expect", ""))
            for t in data.get("task", [])]
