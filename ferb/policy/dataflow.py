"""Data flow (section 10.2): data read from owner accounts may only go to a target of the request, somewhere the
owner named or approved in an ask, or back where it came from. Crossing into FERB's identity is blocked.
Model-provider calls aren't actions and aren't checked (inherent to the design)."""

from ferb.core.types import Identity
from ferb.policy.rules import OUTBOUND, ActionRequest
from ferb.policy.targets import Request, involves


def check(a: ActionRequest, req: Request) -> str | None:
    owner_data = [label for label in a.data_labels if label.source == "owner_account"]
    if not owner_data or a.kind not in OUTBOUND:
        return None
    if a.identity is Identity.FERB:
        return "owner account data may not cross into FERB's identity"
    destinations = {d for d in (a.target.site, a.target.account, *a.target.recipients) if d}
    for label in owner_data:
        home = {h for h in (label.site, label.account) if h}
        stray = sorted(d for d in destinations - home if not involves(req, d))
        if stray:
            return f"owner data from {label.account or label.site} may not go to {', '.join(stray)}"
    return None
