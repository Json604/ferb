"""Data labels for the data-flow rule (section 10.2, safety test 9).

Reading owner-account data (an owner browser profile's page, a mail app's window) labels the worker that read it.
When a reader finishes, the typed fields it returns carry all its labels. When an actor's action content contains
one of those values, the action carries the label and `policy/dataflow.py` decides whether the destination is one
the request involves. Substring matching of values >= MIN_CHARS: it catches copies, not paraphrases (section 19).
"""

from dataclasses import dataclass, field

from ferb.core.types import DataLabel

MIN_CHARS = 8
OWNER_SOURCE = "owner_account"


@dataclass
class Taint:
    worker_labels: dict[str, set[DataLabel]] = field(default_factory=dict)
    values: dict[str, list[tuple[str, frozenset[DataLabel]]]] = field(default_factory=dict)   # tree -> labeled values

    def read(self, worker: str, kind: str, source: str) -> None:
        if kind != OWNER_SOURCE:
            return
        site, _, account = source.partition("|")
        self.worker_labels.setdefault(worker, set()).add(DataLabel(OWNER_SOURCE, account or None, site or None))

    def labels_of(self, worker: str) -> frozenset[DataLabel]:
        return frozenset(self.worker_labels.get(worker, set()))

    def add_values(self, tree: str, values: list[str], labels: frozenset[DataLabel]) -> None:
        if not labels:
            return
        bucket = self.values.setdefault(tree, [])
        for v in values:
            if len(v.strip()) >= MIN_CHARS:
                bucket.append((v.strip().lower(), labels))

    def labels_for(self, tree: str | None, content: str | None) -> frozenset[DataLabel]:
        if not tree or not content:
            return frozenset()
        c = content.lower()
        out: set[DataLabel] = set()
        for value, labels in self.values.get(tree, []):
            if value in c:
                out |= labels
        return frozenset(out)

    def forget_tree(self, tree: str) -> None:
        self.values.pop(tree, None)
