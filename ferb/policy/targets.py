"""Request context. Targets describe intent without enforcing permission."""

import json
from dataclasses import dataclass, field
from typing import Any


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
    """The owner's words and observed targets for one request; agents interpret them as task context."""
    request_id: str
    words: str = ""                                   # the owner's own words (pasted text removed)
    targets: Targets = field(default_factory=Targets)
    owner_email: str | None = None                    # "email me": the owner's own address counts as named


class Requests:
    """Per-request facts, recorded by the supervisor when a task starts and forgotten when it ends."""

    def __init__(self) -> None:
        self._by_id: dict[str, Request] = {}

    def record(self, r: Request) -> None:
        self._by_id[r.request_id] = r

    def get(self, request_id: str) -> Request:
        return self._by_id.get(request_id) or Request(request_id)

    def forget(self, request_id: str) -> None:
        self._by_id.pop(request_id, None)
