"""Extract D*/MP* decision IDs from the 0.27 plan."""

import re
from pathlib import Path

# Matches bullets such as "- D12." or "- D28 (2026-10-07)."; struck-through
# items ("- ~~D3~~") and prose such as "- D23's ..." do not match.
_DECIDED = re.compile(r"^- (D\d+|MP\d+)[.:) ]", re.M)


def plan_decision_ids(plan_path: Path) -> set[str]:
    return set(_DECIDED.findall(plan_path.read_text()))
