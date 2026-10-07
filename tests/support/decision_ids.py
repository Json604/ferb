"""Extract decision IDs (D*, MP*, P*, S*, W*) from the 0.27 plan."""

import re
from pathlib import Path

# Matches bullets such as "- D12.", "- D28 (2026-10-07).", "- P11 (...)" or "- S1 Recovery:"; struck-through
# items ("- ~~P13"), prose such as "- D23's ...", lists such as "- P7, P9 ..." and "SQ"/"Q" items do not match.
# Every P*/S*/W* item is decided per the plan's "Decision status" table (2026-10-07).
_DECIDED = re.compile(r"^- (D\d+|MP\d+|P\d+|S\d+|W\d+)[.:) ]", re.M)


def plan_decision_ids(plan_path: Path) -> set[str]:
    return set(_DECIDED.findall(plan_path.read_text()))
