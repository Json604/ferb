"""Data labels for the data-flow rule (section 10.2, safety test 9).

Reading owner-account data (an owner browser profile's page, a mail app's window) labels the worker that read it.
One worker reads and acts, so what it types or sends after that read may carry the data: the gateway puts the
worker's labels on every outbound action with content, and `policy/dataflow.py` decides whether the destination is
one the request involves (a target, something the owner named, or something they approved in an ask).
"""

from dataclasses import dataclass, field

from ferb.core.types import DataLabel

OWNER_SOURCE = "owner_account"


@dataclass
class Taint:
    worker_labels: dict[str, set[DataLabel]] = field(default_factory=dict)

    def read(self, worker: str, kind: str, source: str) -> None:
        if kind != OWNER_SOURCE:
            return
        site, _, account = source.partition("|")
        self.worker_labels.setdefault(worker, set()).add(DataLabel(OWNER_SOURCE, account or None, site or None))

    def labels_of(self, worker: str) -> frozenset[DataLabel]:
        return frozenset(self.worker_labels.get(worker, set()))

    def forget(self, worker: str) -> None:
        self.worker_labels.pop(worker, None)
