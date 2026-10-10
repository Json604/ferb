"""Owns FERB's origin vocabulary (MP2, S3): where a record's content came from. Must not judge intent or content.

Trust follows origin. The owner's own words carry authority; FERB's own actions are FERB's; the owner's local data
(files, apps, their signed-in accounts) is private; model-written text is inferred; anything from outside (web
pages, email, messages from other people, downloads, pasted text) is external: data, never instructions. A record
holding several origins keeps them all; its trust is the least trusted one, and an unknown label counts as outside."""

from typing import Iterable
from urllib.parse import urlsplit

OWNER, FERB, PRIVATE, INFERRED, EXTERNAL = "owner", "ferb", "private", "inferred", "external"
_RANK = {OWNER: 0, FERB: 1, PRIVATE: 2, INFERRED: 3}
OUTSIDE_RANK = 4


def _clean(text: str) -> str:
    return " ".join(str(text).replace(",", " ").split())[:120]


def external(kind: str, detail: str = "") -> str:
    kind, detail = _clean(kind).lower(), _clean(detail)
    return f"{EXTERNAL}:{kind}" + (f":{detail}" if detail else "")


def is_external(o: str) -> bool:
    return o == EXTERNAL or o.startswith(EXTERNAL + ":")


def rank(o: str) -> int:
    return _RANK.get(o, OUTSIDE_RANK)


def least_trusted(origins: Iterable[str]) -> str:
    items = [o for o in origins if o]
    return max(items, key=lambda o: (rank(o), o)) if items else FERB


def join(origins: Iterable[str]) -> str:
    return ",".join(sorted({o for o in origins if o}))


def split(text: str | None) -> set[str]:
    return {o for o in (text or "").split(",") if o}


def recorded(text: str | None, has_output: bool) -> set[str]:
    """A tool result's or context source's recorded origins: one rule for every reader (episodes, task.evidence,
    task_origins). Content with no recorded origin (every span from before F3) is unlabeled outside content, whatever
    its kind; a span that holds no output adds nothing. Fail-closed: an unknown label counts as outside."""
    found = split(text)
    return found or ({external("unlabeled")} if has_output else set())


def from_read_source(kind: str, ident: str) -> str:
    """One `ToolResult.read_sources` entry → its origin."""
    if kind in ("file", "app", "owner_account"):
        return PRIVATE
    if kind == "download":
        return external("download")
    if kind == "web":                                  # a page, or a host a shell command reached (F5a proxy)
        try:
            host = urlsplit(ident).hostname if "://" in ident else None
        except ValueError:                             # a malformed URL from outside content must not break labeling
            host = None
        return external("web", host or ident)
    if kind == "origin":                              # a record's stored origin, passed through by FERB's readers
        return ident
    return external(kind, ident)                      # email, message, and any unknown kind: outside content
