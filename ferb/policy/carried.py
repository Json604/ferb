"""Stored provenance of what FERB writes from what it was shown (T-1007 design B, GATE-519): a model-written row
keeps its sources' kinds and the owner's values its own text holds, never the union of every label its writer saw.

`derive(text, shown)`: for each whole value of `text` (confirm.whole_values), the classes of the shown records that
hold it: `owner` (the owner's words), `private` (the owner's data), `site:<host>` (a signed-in page of that host). A
shown record FERB wrote itself from what it was shown (a derived row with stored values) gives the classes of its own
stored values only. A value found, as written, only in records that are neither the owner's words nor data (FERB's own
code-written facts such as the clock, outside content) or, not as one of its values, in a derived record, is not the
owner's and isn't carried. A value found nowhere as written, when the writer saw the owner's words or data, is carried
as `private`: a reformatted value is still the owner's (fail-closed). Words are never values; rule 9 for paraphrased instructions rides on the kinds. Shape and provenance
only (rule 4)."""
import re
from typing import Any, Iterable, Sequence

from ferb.core import origin
from ferb.policy import confirm, provenance

PROVENANCE = "provenance"        # the JSON field a derived row keeps it in (journal data)
OWNERS = frozenset({origin.PRIVATE, origin.OWNER})
_JOINED = re.compile(r"(?<=\d)[\s.,+\-]+(?=\d)")       # digit groups written together: +44 7700 → +447700

Shown = Sequence[Any]            # (labels, text) or (labels, text, values | None)


def _forms(text: str) -> tuple[str, str]:
    folded = text.casefold()
    return folded, _JOINED.sub("", folded)


def _holds(forms: tuple[str, str], spelled: set[str]) -> bool:
    return any(provenance._contains(t, v) for t in forms for v in spelled if v)


def _sites(labels: set[str]) -> set[str]:
    """The site a record was read from, when it is one site's page read in the owner's signed-in browser (GATE-506's
    shape: web pages and the private label, nothing else): never the hosts a mixed or derived record inherited."""
    web = {o.split(":", 2)[2] for o in labels if o.startswith(origin.EXTERNAL + ":web:") and o.count(":") >= 2}
    if not web or any(o != origin.PRIVATE and not o.startswith(origin.EXTERNAL + ":web:") for o in labels):
        return set()
    return {"site:" + host for host in web}


def derive(text: str, shown: Iterable[Shown], made: Iterable[str] = ()) -> dict[str, Any]:
    """`{"kinds": [...], "values": {value: [classes]}}` for a row FERB writes with `text`, its writer shown `shown`.
    `made`: the row's own labels; when they hold the owner's words or data, a value found nowhere is theirs even if
    what was shown wasn't kept (fail-closed)."""
    made = set(made)
    records: list[tuple[set[str], tuple[str, str], dict[str, list[str]] | None]] = []
    for item in shown:
        labels, body = set(item[0]), str(item[1] or "")
        stored = item[2] if len(item) > 2 else None
        if body:
            records.append((labels, _forms(body), stored if isinstance(stored, dict) else None))
    kinds = sorted(origin.kinds({o for labels, _, _ in records for o in labels} | made))
    saw_private = bool(made & OWNERS) or any(labels & OWNERS for labels, _, _ in records)
    values: dict[str, list[str]] = {}
    for value in confirm.whole_values(text):
        spelled = confirm.spellings(value)
        classes: set[str] = set()
        accounted = False
        for labels, forms, stored in records:
            if stored is not None:                     # FERB wrote it: only its own stored values are the owner's
                held = [c for v, cs in stored.items() if _holds(_forms(v), spelled) for c in cs]
                classes |= set(held)
                accounted = accounted or (not held and _holds(forms, spelled))
            elif labels & OWNERS:
                if _holds(forms, spelled):
                    # one site's signed-in page gives only that site (GATE-524: the value may go back to it); any
                    # other private record, the owner's data at large
                    sites = _sites(labels) if origin.PRIVATE in labels else set()
                    classes |= ({origin.OWNER} & labels) | (sites or ({origin.PRIVATE} & labels))
            elif _holds(forms, spelled):
                accounted = True                       # FERB's own code-written facts (the clock), or outside content
        if not classes and saw_private and not accounted:
            classes = {origin.PRIVATE}                 # not found as written: still the owner's (fail-closed)
        if classes:
            values[value] = sorted(classes)
    return {"kinds": kinds, "values": values}


def shown_records(outputs: Iterable[Any]) -> list[tuple[set[str], str, Any]]:
    """A task's recorded reads (`SpanStore.payload_outputs`) as `derive` takes them: a context record holding
    FERB's history is split into its entries (`segments`: each entry's kinds, length and stored values, or `whole`
    when its whole text counts), each entry with its own labels."""
    out: list[tuple[set[str], str, Any]] = []
    for result in outputs:
        segments = getattr(result, "segments", None)
        if not segments:
            out.append((set(result.origins), result.text, None))
            continue
        at = 0
        for seg in segments:
            size = int(seg.get("len", 0))
            labels = {str(k) for k in seg.get("kinds", [])}
            out.append((labels, result.text[at:at + size], None if seg.get("whole") else seg.get("values") or {}))
            at += size + 2                             # entries are joined by a blank line (`_context_exposure`)
    return out


def sink_parts(payload: str, segments: Iterable[dict[str, Any]]) -> tuple[str, dict[frozenset[str], str]]:
    """What of a stored record of FERB's conversation is the owner's data for the sink (GATE-522): each entry whose
    whole text counts (the owner's own messages, notices, rows from before stored provenance) when it is the owner's
    words or data, and of each entry FERB wrote only the values it stored. FERB's own prose around them (a date its
    clock wrote, a page's words it repeated) is not the owner's data. Values stored as read only on one or more
    sites' own signed-in pages come apart, by those sites (GATE-524: they may go back to them)."""
    kept: list[str] = []
    by_site: dict[frozenset[str], list[str]] = {}
    at = 0
    for seg in segments:
        size = int(seg.get("len", 0))
        piece = payload[at:at + size]
        at += size + 2                                 # entries are joined by a blank line
        if seg.get("whole"):
            if set(seg.get("kinds", [])) & OWNERS:
                kept.append(piece)
        elif isinstance(seg.get("values"), dict):
            for value, classes in seg["values"].items():
                sites = frozenset(str(c)[5:] for c in classes if str(c).startswith("site:"))
                if sites and len(sites) == len(classes):
                    by_site.setdefault(sites, []).append(str(value))
                else:
                    kept.append(str(value))
    return "\n".join(kept), {sites: "\n".join(values) for sites, values in by_site.items()}


def sink_text(payload: str, segments: Iterable[dict[str, Any]]) -> str:
    """`sink_parts`, every value together (for a reader that needs no sites)."""
    main, by_site = sink_parts(payload, segments)
    return "\n".join([main, *by_site.values()]).strip("\n")
