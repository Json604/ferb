"""Intent match (section 10.2): does this action match what the owner asked for?

The typed intent is derived only from the owner's message at plan time — before any untrusted content is read —
recorded once per request, and never changeable by workers (IntentRegistry refuses a second write).
"""

from dataclasses import dataclass
from typing import Callable, Literal

from ferb.policy.rules import REVERSIBLE, ActionRequest, Kind

# What a request clearly implies ("book a table" -> fill a form and submit it). Plan-time expansion only.
IMPLIES: dict[str, frozenset[str]] = {
    "book": frozenset({Kind.NAVIGATE, Kind.SEARCH, Kind.OPEN_TAB, Kind.FILL_FORM, Kind.SUBMIT}),
    "buy": frozenset({Kind.NAVIGATE, Kind.SEARCH, Kind.OPEN_TAB, Kind.FILL_FORM, Kind.SUBMIT, Kind.PAY}),
    "email": frozenset({Kind.DRAFT, Kind.SEND}),
    "reply": frozenset({Kind.READ, Kind.DRAFT, Kind.REPLY}),
    "post": frozenset({Kind.DRAFT, Kind.POST}),
    "summarize": frozenset({Kind.READ, Kind.NAVIGATE, Kind.SEARCH, Kind.OPEN_TAB}),
    "research": frozenset({Kind.READ, Kind.NAVIGATE, Kind.SEARCH, Kind.OPEN_TAB}),
    "fill_form": frozenset({Kind.NAVIGATE, Kind.FILL_FORM, Kind.SUBMIT}),
    "edit_file": frozenset({Kind.READ, Kind.WRITE_FILE}),
    "run_command": frozenset({Kind.RUN_SHELL}),
    "sign_in": frozenset({Kind.NAVIGATE, Kind.OPEN_TAB, Kind.FILL_FORM, Kind.SUBMIT, Kind.FILL_SECRET}),
}


@dataclass(frozen=True)
class RequestIntent:
    request_id: str
    kinds: frozenset[str]                    # explicit + implied action kinds
    recipients: frozenset[str] = frozenset()
    sites: frozenset[str] = frozenset()
    accounts: frozenset[str] = frozenset()
    paths: frozenset[str] = frozenset()      # path prefixes
    max_amount: float | None = None

    @classmethod
    def from_plan(cls, request_id: str, verbs: set[str], extra_kinds: set[str] | None = None,
                  **targets: object) -> "RequestIntent":
        kinds: set[str] = set(extra_kinds or ())
        for v in verbs:
            kinds |= IMPLIES[v]
        return cls(request_id, frozenset(kinds), **targets)  # type: ignore[arg-type]

    @property
    def destinations(self) -> frozenset[str]:
        return self.recipients | self.sites | self.accounts


class IntentRegistry:
    """Write-once per request. The supervisor records it at plan time; nothing can replace it afterwards."""

    def __init__(self) -> None:
        self._by_request: dict[str, RequestIntent] = {}

    def record(self, intent: RequestIntent) -> None:
        if intent.request_id in self._by_request:
            raise PermissionError(f"intent for request {intent.request_id} is already recorded and immutable")
        self._by_request[intent.request_id] = intent

    def get(self, request_id: str) -> RequestIntent | None:
        return self._by_request.get(request_id)


@dataclass(frozen=True)
class IntentResult:
    decision: Literal["match", "ask", "block"]
    reason: str = ""


# Optional fuzzy checker (Jev action.matches_request, step 12): "yes" | "no" | "unsure". It can only make things
# stricter: it's consulted only after a deterministic match and can turn it into block/ask, never the reverse.
Fuzzy = Callable[[ActionRequest, RequestIntent], Literal["yes", "no", "unsure"]]


def site_matches(host: str, sites: frozenset[str]) -> bool:
    """"example.com" in the request covers example.com and its subdomains (accounts.example.com), nothing else."""
    host = host.lower().rstrip(".")
    for s in sites:
        s = s.lower().split("://")[-1].split("/")[0].split(":")[0].removeprefix("www.").rstrip(".")
        if s and (host == s or host.endswith("." + s) or host == "www." + s):
            return True
    return False


def check(a: ActionRequest, intent: RequestIntent | None, fuzzy: Fuzzy | None = None) -> IntentResult:
    if a.kind in REVERSIBLE:
        return IntentResult("match")
    if intent is None:
        return IntentResult("block", "no recorded intent for this request")
    if a.kind not in intent.kinds:
        return IntentResult("block", f"'{a.kind}' was not requested")
    t = a.target
    if t.recipients and not set(t.recipients) <= intent.recipients:
        new = sorted(set(t.recipients) - intent.recipients)
        return IntentResult("ask", f"surprising recipient(s): {', '.join(new)}")
    if t.site and intent.sites and not site_matches(t.site, intent.sites):
        return IntentResult("ask", f"surprising site: {t.site}")
    if t.account and intent.accounts and t.account not in intent.accounts:
        return IntentResult("ask", f"surprising account: {t.account}")
    if t.path and intent.paths and not any(t.path == p or t.path.startswith(p.rstrip("/") + "/")
                                           for p in intent.paths):
        return IntentResult("ask", f"surprising path: {t.path}")
    if t.amount is not None and intent.max_amount is not None and t.amount > intent.max_amount:
        return IntentResult("ask", f"amount {t.amount} above requested {intent.max_amount}")
    if fuzzy is not None:
        verdict = fuzzy(a, intent)
        if verdict == "no":
            return IntentResult("block", "fuzzy check: doesn't match the request")
        if verdict == "unsure":
            return IntentResult("ask", "fuzzy check: unsure it matches the request")
    return IntentResult("match")
