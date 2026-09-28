"""Exposure (section 10.2): a worker that read untrusted content loses shell, AppleScript and file-write for the
rest of its run. Owner-marked trusted sources don't expose; an injection signal exposes immediately."""

from dataclasses import dataclass, field

from ferb.policy.rules import ActionRequest

LOST_WHEN_EXPOSED = frozenset({"shell", "applescript", "fs:write"})
UNTRUSTED_KINDS = frozenset({"web", "email", "message", "download", "third_party"})


@dataclass
class Exposure:
    exposed: bool = False
    reasons: list[str] = field(default_factory=list)


class ExposureTracker:
    def __init__(self, trusted_sources: set[str] | None = None) -> None:
        self.trusted_sources = set(trusted_sources or ())    # owner-marked: own notes, chosen senders
        self._workers: dict[str, Exposure] = {}

    def state(self, worker_id: str) -> Exposure:
        return self._workers.setdefault(worker_id, Exposure())

    def read(self, worker_id: str, source_kind: str, source_id: str) -> bool:
        """Record a read. Returns True if this read newly exposed the worker."""
        if source_kind not in UNTRUSTED_KINDS or source_id in self.trusted_sources:
            return False
        return self._expose(worker_id, f"read {source_kind}: {source_id}")

    def injection_signal(self, worker_id: str, detail: str) -> bool:
        return self._expose(worker_id, f"injection signal: {detail}")

    def _expose(self, worker_id: str, reason: str) -> bool:
        st = self.state(worker_id)
        newly = not st.exposed
        st.exposed = True
        st.reasons.append(reason)
        return newly

    def check(self, a: ActionRequest) -> str | None:
        st = self._workers.get(a.worker_id)
        if st and st.exposed and a.scopes & LOST_WHEN_EXPOSED:
            return (f"worker is exposed ({st.reasons[0]}); it has lost "
                    f"{', '.join(sorted(a.scopes & LOST_WHEN_EXPOSED))}")
        return None
