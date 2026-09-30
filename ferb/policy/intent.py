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
    "email": frozenset({Kind.DRAFT, Kind.SEND, Kind.MAILBOX_SEND}),   # MAILBOX_SEND: from FERB's own mailbox
    "reply": frozenset({Kind.READ, Kind.DRAFT, Kind.REPLY}),
    "post": frozenset({Kind.DRAFT, Kind.POST}),
    "summarize": frozenset({Kind.READ, Kind.NAVIGATE, Kind.SEARCH, Kind.OPEN_TAB}),
    "research": frozenset({Kind.READ, Kind.NAVIGATE, Kind.SEARCH, Kind.OPEN_TAB}),
    "fill_form": frozenset({Kind.NAVIGATE, Kind.FILL_FORM, Kind.SUBMIT}),
    "edit_file": frozenset({Kind.READ, Kind.WRITE_FILE}),
    "run_command": frozenset({Kind.RUN_SHELL}),
    "sign_in": frozenset({Kind.NAVIGATE, Kind.OPEN_TAB, Kind.FILL_FORM, Kind.SUBMIT, Kind.FILL_SECRET}),
    "use_app": frozenset({Kind.READ, Kind.MAC_INPUT, Kind.APPLESCRIPT}),
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
    irreversible: frozenset[str] = frozenset()   # kinds the model marked irreversible for this request (ask each time)

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


# Optional fuzzy checker (Jev action.matches_request, build step 13): "yes" | "no" | "unsure". It can only make things
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



def within(run: RequestIntent, recorded: RequestIntent) -> list[str]:
    """A scheduled run's intent must stay inside the one recorded from the owner's words (build step 15)."""
    p = []
    if run.kinds - recorded.kinds:
        p.append("action types " + ", ".join(sorted(run.kinds - recorded.kinds)))
    if {r.lower() for r in run.recipients} - {r.lower() for r in recorded.recipients}:
        p.append("recipients " + ", ".join(sorted(run.recipients - recorded.recipients)))
    extra_sites = [x for x in run.sites if not site_matches(x, recorded.sites)]
    if extra_sites:
        p.append("sites " + ", ".join(sorted(extra_sites)))
    if run.accounts - recorded.accounts:
        p.append("accounts " + ", ".join(sorted(run.accounts - recorded.accounts)))
    extra_paths = [x for x in run.paths if not any(x == r or x.startswith(r.rstrip("/") + "/") for r in recorded.paths)]
    if extra_paths:
        p.append("paths " + ", ".join(sorted(extra_paths)))
    if run.max_amount is not None and (recorded.max_amount is None or run.max_amount > recorded.max_amount):
        p.append("a larger amount")
    return p


def to_json(i: RequestIntent) -> dict[str, object]:
    return {"kinds": sorted(i.kinds), "recipients": sorted(i.recipients), "sites": sorted(i.sites),
            "accounts": sorted(i.accounts), "paths": sorted(i.paths), "max_amount": i.max_amount,
            "irreversible": sorted(i.irreversible)}


def from_json(request_id: str, d: dict[str, object]) -> RequestIntent:
    def fs(k: str) -> frozenset[str]:
        v = d.get(k) or []
        return frozenset(str(x) for x in v) if isinstance(v, (list, tuple, set, frozenset)) else frozenset()
    amount = d.get("max_amount")
    return RequestIntent(request_id, fs("kinds"), fs("recipients"), fs("sites"), fs("accounts"), fs("paths"),
                         float(amount) if isinstance(amount, (int, float)) else None, fs("irreversible"))
