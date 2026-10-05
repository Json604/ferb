"""Apply owner-configured observation exclusions. Must not infer app/task preferences beyond those settings.

The exclusion list, applied before anything is written, published or tagged. An excluded
observation is dropped completely: no row, no bus event, no count (owner decision).

Lists come from config `[privacy]`: `exclude_apps` (bundle ids or names; also what Mac control refuses),
`exclude_domains` (a domain covers its subdomains), `exclude_paths` (skipped even inside a watched folder).
Private / incognito browsing is never recorded."""

from dataclasses import dataclass, field
from pathlib import Path
from typing import Any
from urllib.parse import urlsplit

DEFAULT_EXCLUDE_PATHS = ["~/.ssh", "~/Library/Keychains", "~/.ferb"]


@dataclass(frozen=True)
class Exclusions:
    apps: frozenset[str] = frozenset()
    domains: frozenset[str] = frozenset()
    paths: tuple[Path, ...] = field(default_factory=tuple)

    @classmethod
    def build(cls, apps: Any = (), domains: Any = (), paths: Any = DEFAULT_EXCLUDE_PATHS) -> "Exclusions":
        return cls(frozenset(str(a).lower() for a in apps),
                   frozenset(str(d).lower().lstrip(".") for d in domains),
                   tuple(Path(str(p)).expanduser().resolve() for p in paths))

    def app(self, bundle_id: str | None, name: str | None = None) -> bool:
        return any(v and v.lower() in self.apps for v in (bundle_id, name))

    def host(self, host: str | None) -> bool:
        h = (host or "").lower().rstrip(".")
        return bool(h) and any(h == d or h.endswith("." + d) for d in self.domains)

    def url(self, url: str | None) -> bool:
        try:
            return self.host(urlsplit(url or "").hostname)
        except ValueError:
            return False

    def path(self, path: str | Path | None) -> bool:
        if not path:
            return False
        p = Path(path).expanduser()
        try:
            p = p.resolve()
        except OSError:
            pass
        return any(p == x or x in p.parents for x in self.paths)

    def excluded(self, source: str, data: dict[str, Any]) -> bool:
        """Every rule applies to every source: an event naming an excluded app, host or path is dropped."""
        if data.get("incognito"):
            return True
        if self.app(data.get("bundle_id"), data.get("app")):
            return True
        if self.host(data.get("host")) or self.url(data.get("url")):
            return True
        return self.path(data.get("path")) or self.path(data.get("from_path"))
