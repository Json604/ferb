"""The sink-rule corpus (tests/corpus/README.md): every scenario replayed through FERB's real gateway.

Pins today's safety floor: a must-ask scenario FERB asks about today must keep asking (zero new misses). Misses and false
questions known today are listed in tests/corpus/baseline.json; a false question is recorded, never a failure, so the
labels redesign can shrink that list. FERB_CORPUS_REPORT=<path.json> writes every result there (and a .md beside it)."""
import json
import os
import re
import warnings
from pathlib import Path

import pytest

from tests.corpus.generate import generate
from tests.corpus.model import from_json, to_json
from tests.corpus.replay import markdown, replay, summarize

HERE = Path(__file__).parent / "corpus"
BASELINE = json.loads((HERE / "baseline.json").read_text())


def test_the_corpus_file_is_the_generators_output() -> None:
    """The generator is deterministic, and scenarios.json is what it makes (regenerate: python -m tests.corpus.generate)."""
    made = generate()
    assert to_json(made) == (HERE / "scenarios.json").read_text()
    assert to_json(from_json(to_json(made))) == to_json(made)
    assert len(made) >= 300


RESERVED = re.compile(r"(^|\.)(example|test|example\.com|example\.org|example\.net)$")


def test_every_host_email_and_phone_in_the_corpus_is_reserved_or_fictional() -> None:
    """No real person's data and no real site (AGENTS.md: nothing personal in tracked files)."""
    text = (HERE / "scenarios.json").read_text()
    hosts = set(re.findall(r"https?://([^/?#\s\"\\]+)", text)) | set(re.findall(r"@([a-z0-9.-]+\.[a-z]+)", text))
    assert hosts and not [h for h in hosts if not RESERVED.search(h.split(":")[0].lower())]
    phones = re.findall(r"\+\d[\d\s()-]{8,}\d", text)
    assert phones and all(re.sub(r"\D", "", p).startswith(("447700900", "1555")) for p in phones), phones


@pytest.mark.ferb_timeout(900)
def test_replay_keeps_todays_safety_floor(monkeypatch: pytest.MonkeyPatch, tmp_path: Path) -> None:
    monkeypatch.setattr("ferb.sanitize.images.recognize_text", lambda _image: [])   # as the daemon fixture does
    scenarios = generate()
    only = os.environ.get("FERB_CORPUS_ONLY", "")                  # a prefix, for working on one family
    if only:
        scenarios = [s for s in scenarios if s.id.startswith(only)]
    results = [replay(s) for s in scenarios]
    if target := os.environ.get("FERB_CORPUS_REPORT"):
        out = Path(target)
        out.write_text(json.dumps({"summary": summarize(results), "results": [r.row() for r in results]}, indent=1))
        out.with_suffix(".md").write_text(markdown(results, {s.id: s for s in scenarios}))
    misses = sorted(r.id for r in results if r.verdict == "miss")
    new_misses = [m for m in misses if m not in BASELINE["known_misses"]]
    assert not new_misses, f"must-ask scenarios FERB no longer asks about: {new_misses}"
    false_q = sorted(r.id for r in results if r.verdict == "false_question")
    new_false = [f for f in false_q if f not in BASELINE["false_questions"]]
    if new_false:                                                  # recorded, never failed: the baseline to tighten
        warnings.warn(f"false questions not in the baseline: {new_false}")
    fixed = [f for f in BASELINE["false_questions"] if f not in false_q and any(r.id == f for r in results)]
    if fixed:
        warnings.warn(f"baseline false questions FERB no longer asks (tighten baseline.json): {fixed}")
