"""What a request is about (docs/DESIGN_ONE_WORKER.md sections 1 and 3).

A request keeps the owner's own words (never pasted text) and the `targets` the supervisor model understood from
them: apps, sites, paths, recipients. Targets are informational — they feed the environment line and the data-flow
rule — never a reason to refuse. The one thing that lifts an ask is the owner's own message naming the target
(`named`): then a send to it is auto-granted for the rest of the request.

`within` holds a scheduled run to the targets recorded when the owner scheduled it.
"""

import json
import re
from dataclasses import dataclass, field, replace
from typing import Any

from ferb.policy.services import CATALOGUE

GENERIC_LABELS = frozenset({"www", "app", "web", "mail", "m", "com", "co", "in", "org", "net", "io", "so", "be",
                            "ai", "desktop", "client"})


@dataclass(frozen=True)
class Targets:
    apps: frozenset[str] = frozenset()         # bundle ids or app names, as the model wrote them
    sites: frozenset[str] = frozenset()        # domains
    paths: frozenset[str] = frozenset()        # path prefixes (normalized)
    recipients: frozenset[str] = frozenset()   # people / addresses, as the owner named them

    @classmethod
    def of(cls, apps: Any = (), sites: Any = (), paths: Any = (), recipients: Any = ()) -> "Targets":
        return cls(frozenset(str(x) for x in apps), frozenset(str(x) for x in sites),
                   frozenset(str(x) for x in paths), frozenset(str(x) for x in recipients))

    @property
    def destinations(self) -> frozenset[str]:
        return self.recipients | self.sites | self.apps

    def to_json(self) -> dict[str, Any]:
        return {"apps": sorted(self.apps), "sites": sorted(self.sites), "paths": sorted(self.paths),
                "recipients": sorted(self.recipients)}

    @classmethod
    def from_json(cls, d: dict[str, Any] | str | None) -> "Targets":
        if isinstance(d, str):
            d = json.loads(d)
        d = d or {}

        def fs(k: str) -> frozenset[str]:
            v = d.get(k) or []
            return frozenset(str(x) for x in v) if isinstance(v, (list, tuple, set, frozenset)) else frozenset()
        return cls(fs("apps"), fs("sites"), fs("paths"), fs("recipients"))


@dataclass(frozen=True)
class Request:
    """The facts the gateway keeps about one request: the owner's own words, the targets, and what the owner has
    approved in an ask during it (a destination for owner data)."""
    request_id: str
    words: str = ""                                   # the owner's own words (pasted text removed)
    targets: Targets = field(default_factory=Targets)
    owner_email: str | None = None                    # "email me": the owner's own address counts as named
    approved: frozenset[str] = frozenset()            # destinations the owner said yes to in this request


class Requests:
    """Per-request facts, recorded by the supervisor when a task starts and forgotten when it ends."""

    def __init__(self) -> None:
        self._by_id: dict[str, Request] = {}

    def record(self, r: Request) -> None:
        self._by_id[r.request_id] = r

    def get(self, request_id: str) -> Request:
        return self._by_id.get(request_id) or Request(request_id)

    def approve(self, request_id: str, destination: str) -> None:
        cur = self.get(request_id)
        self._by_id[request_id] = replace(cur, approved=cur.approved | {destination.lower()})

    def forget(self, request_id: str) -> None:
        self._by_id.pop(request_id, None)


def _words(text: str) -> set[str]:
    return set(re.findall(r"[a-z0-9]+", text.lower()))


def site_matches(host: str, sites: frozenset[str] | set[str]) -> bool:
    """"example.com" covers example.com and its subdomains (accounts.example.com), nothing else."""
    host = host.lower().rstrip(".")
    for s in sites:
        s = s.lower().split("://")[-1].split("/")[0].split(":")[0].removeprefix("www.").rstrip(".")
        if s and (host == s or host.endswith("." + s) or host == "www." + s):
            return True
    return False


def _labels(target: str) -> list[str]:
    """The distinctive words of a domain or bundle id: "whatsapp" for net.whatsapp.WhatsApp / web.whatsapp.com."""
    return [x for x in re.split(r"[.\-_]", target.lower()) if x and x not in GENERIC_LABELS and len(x) >= 3]


def named(target: str, words: str) -> bool:
    """The owner's own words name this target: the target itself, a distinctive label of a domain / bundle id, or
    a catalogue name / alias for that service ("Gmail" names mail.google.com; "WhatsApp" the WhatsApp app)."""
    t = target.strip().lower()
    if not t or not words:
        return False
    lw = " ".join(words.lower().split())
    if t in lw:
        return True
    toks = _words(lw)
    if any(x in toks for x in _labels(t)):
        return True
    for s in CATALOGUE:
        if t in {b.lower() for b in s.bundles} or site_matches(t, frozenset(s.domains)) or t == s.name.lower():
            for alias in (s.name.lower(), *s.aliases):
                if (alias in toks) if " " not in alias else (alias in lw):
                    return True
    return False


def owner_named(target: Any, req: Request, people_only: bool = False) -> bool:
    """The owner's own message names where this action goes: every recipient (if any), else the site / app. A
    message or call (`people_only`) with nobody named goes to whoever is on screen: never auto-granted."""
    recipients = [r for r in target.recipients if r]
    if recipients:
        return all(r.strip().lower() == (req.owner_email or "").lower() or named(r, req.words) for r in recipients)
    if people_only:
        return False
    if target.site:
        return named(target.site, req.words)
    return False


def involves(req: Request, destination: str) -> bool:
    """Owner data may go here: a target of the request, something the owner named, or a destination they approved."""
    d = destination.strip().lower()
    if not d:
        return True
    if d in req.approved or d in {x.lower() for x in req.targets.destinations}:
        return True
    if site_matches(d, req.targets.sites | req.targets.apps):
        return True
    return named(d, req.words)


def within(run: Targets, recorded: Targets) -> list[str]:
    """A scheduled run's targets must stay inside those recorded from the owner's words (build step 15)."""
    p = []
    extra_people = {r.lower() for r in run.recipients} - {r.lower() for r in recorded.recipients}
    if extra_people:
        p.append("recipients " + ", ".join(sorted(extra_people)))
    extra_sites = [x for x in run.sites if not site_matches(x, recorded.sites)]
    if extra_sites:
        p.append("sites " + ", ".join(sorted(extra_sites)))
    extra_apps = {a.lower() for a in run.apps} - {a.lower() for a in recorded.apps}
    if extra_apps:
        p.append("apps " + ", ".join(sorted(extra_apps)))
    extra_paths = [x for x in run.paths if not any(x == r or x.startswith(r.rstrip("/") + "/") for r in recorded.paths)]
    if extra_paths:
        p.append("paths " + ", ".join(sorted(extra_paths)))
    return p
