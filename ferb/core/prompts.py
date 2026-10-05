"""Owns versioned prompt loading. Must not infer intent, choose a target or claim semantic success from transport facts.

Load versioned prompt files from the package; never compose behavioral instructions in Python."""

from pathlib import Path

PACKAGE = Path(__file__).resolve().parents[1]


def load_prompt(relative_path: str, replacements: dict[str, str] | None = None) -> str:
    path = (PACKAGE / relative_path).resolve()
    if PACKAGE not in path.parents or path.suffix != ".md":
        raise ValueError("prompt must be a Markdown file inside the FERB package")
    text = path.read_text(encoding="utf-8").strip()
    for key, value in (replacements or {}).items():
        text = text.replace("{" + key + "}", value)
    return text
