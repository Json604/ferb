"""Own the list of sites a task tree contacted, built from FERB's own records (L2). Must not take a host from a
model's words, and must not judge a site.

`sites_contacted(spans, tree_id)` reads two records:
- each finished shell process's `shell.network` journal row (what FERB's egress proxy recorded it reaching, the row
  `ferb timeline` shows), for the processes of the tree's tasks;
- the browser calls that asked to load an address in one of the tree's own tabs (an open or a navigate that ran, or
  whose outcome is unknown, so it may have reached the site): the http(s) address asked for and the address the tab
  reported. A borrowed owner tab whose address FERB didn't ask for is not counted.

The answer's "Sites contacted" line is built from it by code. A host on the owner's exclusion list is already recorded
as excluded (the proxy's host and the stored address both), so it never appears. A process still running is journaled
when it ends."""

import json
from typing import Any

from ferb.policy import urls
from ferb.policy.destinations import KnownDestinations
from ferb.policy.rules import Kind
from ferb.store.spans import SpanStore, tree_where

EXCLUDED = "excluded"                        # the proxy's record of a host on the owner's exclusion list
WEB = ("http", "https")


def sites_contacted(spans: SpanStore, tree_id: str) -> list[str]:
    """The hosts this task tree reached, each once (normalized: lowercase, no trailing dot), sorted."""
    where, params = tree_where(tree_id)
    hosts: set[str] = set()
    for row in spans.db.conn.execute(
            "SELECT data_json FROM (SELECT data_json, json_extract(data_json, '$.task_id') AS task_id FROM journal "
            f"WHERE kind = 'shell.network') WHERE {where}", params):
        hosts.update(str(d.get("host") or "") for d in json.loads(row["data_json"]).get("destinations") or [])
    for row in spans.db.conn.execute(
            "SELECT input_json, output_json FROM spans WHERE kind = 'tool.call' AND ended_at IS NOT NULL "
            "AND (error IS NULL OR json_extract(input_json, '$.outcome') = 'unknown') "
            f"AND json_extract(input_json, '$.kind') IN (?, ?) AND {where}", (Kind.NAVIGATE, Kind.OPEN_TAB, *params)):
        asked = (json.loads(row["input_json"]).get("args") or {}).get("url")
        if not _web_host(asked):                         # no address asked for (a borrowed tab): nothing FERB loaded
            continue
        shown = json.loads(row["output_json"] or "{}").get("url")
        hosts.update(_web_host(u) for u in (asked, shown))
    return sorted({h for h in (KnownDestinations.normalize("host", h) for h in hosts) if h and h != EXCLUDED})


def _web_host(url: Any) -> str:
    """The host an http(s) address names, parsed as the browser does; "" for anything else."""
    parsed = urls.parse(url) if isinstance(url, str) and url else None
    return parsed.host if parsed is not None and parsed.scheme in WEB else ""
