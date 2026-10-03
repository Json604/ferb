"""Legacy policy evaluator, retained for compatibility tests and request/question/stop state.

The 0.22 execution gateway does NOT call evaluate. Do not reconnect legacy identity, exposure, effect or undo
decisions: authorization and recovery belong to the orchestrator/worker contract."""

from dataclasses import dataclass, field
from typing import Any, Callable, Literal

from ferb.core.interfaces import Clock
from ferb.policy import dataflow
from ferb.policy.approvals import Approvals, Hold, Holds
from ferb.policy.autonomy import Level, ask_reason
from ferb.policy.exposure import ExposureTracker
from ferb.policy.rules import UNDO_WINDOW, ActionRequest, check_identity, check_scopes
from ferb.policy.targets import Requests


@dataclass
class Verdict:
    decision: Literal["allow", "hold", "ask", "block"]
    reason: str = ""
    step: str = ""                                 # which check decided
    approval_id: str | None = None
    hold_id: str | None = None
    events: list[tuple[str, dict[str, Any]]] = field(default_factory=list)   # (bus topic, data) to publish


class PolicyEngine:
    def __init__(self, clock: Clock, ferb_workspace: str, level: Callable[[], Level] = lambda: Level.BALANCED,
                 trusted_kinds: Callable[[], frozenset[str]] = lambda: frozenset(),
                 undo_window_s: float = 10.0,
                 trusted_accounts: Callable[[], frozenset[str]] = lambda: frozenset(),
                 remember_account: Callable[[str], None] = lambda key: None) -> None:
        self.clock = clock
        self.ferb_workspace = ferb_workspace
        self.level = level
        self.trusted_kinds = trusted_kinds
        self.trusted_accounts, self.remember_account = trusted_accounts, remember_account
        self.requests = Requests()
        self.exposure = ExposureTracker()
        self.approvals = Approvals(clock)
        self.holds = Holds(clock, undo_window_s)
        self.stopped = False

    def evaluate(self, a: ActionRequest) -> Verdict:
        if self.stopped:
            return self._block("stop", "FERB is stopped (`ferb resume-tasks` lifts it)", a)
        reason = check_identity(a, self.ferb_workspace) or check_scopes(a)
        if reason:
            return self._block("identity", reason, a)
        reason = self.exposure.check(a)
        if reason:
            return self._block("exposure", reason, a)
        if a.effect == "navigate":
            return Verdict("allow", step="navigate")          # opens, scrolls, searches, reads
        req = self.requests.get(a.request_id)
        reason = dataflow.check(a, req)
        if reason:
            return self._block("dataflow", reason, a)
        st = self.exposure.state(a.worker_id)
        injected = any(r.startswith("injection signal") for r in st.reasons)
        reason = ask_reason(a, self.level(), injected, self.trusted_kinds())
        account_q = None
        if a.unverified_account and a.unverified_account not in self.trusted_accounts():
            account_q = (f"FERB can't check which account is signed in on {a.unverified_account}; approve once "
                         "and FERB won't ask again for this site and profile")
            reason = reason or account_q
        consumed = self.approvals.consume(a) if reason else None
        if consumed is not None and account_q is not None and consumed.reason == account_q:
            self.remember_account(a.unverified_account or "")        # learned trust (10.4), revocable
        if reason and consumed is None:
            ap = self.approvals.request(a, reason)
            return Verdict("ask", reason, "approval", approval_id=ap.id,
                           events=[("approval.requested", {"approval_id": ap.id, "capability": a.capability,
                                                           "kind": a.kind, "effect": a.effect, "reason": reason,
                                                           "task_id": a.task_id})])
        if a.kind in UNDO_WINDOW:
            h = self.holds.hold(a)
            return Verdict("hold", f"held {self.holds.window_s:.0f} s for undo", "undo", hold_id=h.id,
                           events=[("action.held", {"hold_id": h.id, "capability": a.capability, "kind": a.kind,
                                                    "seconds": self.holds.window_s, "task_id": a.task_id,
                                                    "to": a.target.contact})])
        return Verdict("allow", step="allow")

    def release_due(self) -> list[tuple[Hold, Verdict]]:
        """Holds whose undo window passed. Re-checks stop: a stopped FERB never releases."""
        if self.stopped:
            return []
        return [(h, Verdict("allow", step="released", hold_id=h.id,
                            events=[("action.released", {"hold_id": h.id, "capability": h.action.capability,
                                                         "task_id": h.action.task_id})]))
                for h in self.holds.due()]

    def undo(self, hold_id: str) -> bool:
        return self.holds.undo(hold_id)

    def _block(self, step: str, reason: str, a: ActionRequest) -> Verdict:
        return Verdict("block", reason, step, events=[("policy.blocked", {
            "step": step, "reason": reason, "capability": a.capability, "kind": a.kind,
            "task_id": a.task_id, "worker_id": a.worker_id})])
