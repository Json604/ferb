"""Owns the uniform after-action read (V2): what an acting tool observed changed, and whether a procedure's expect
matched. Must not claim a change it did not read: a tool that read nothing back reports changed=None (unknown), never
a false pass."""

from typing import Any


def facts_of(content: dict[str, Any]) -> dict[str, Any]:
    """Where an acting tool keeps its per-action facts: mac tools nest them under `facts`; browser.act spreads them
    into the result itself."""
    nested = content.get("facts")
    return nested if isinstance(nested, dict) else content


def after_of(facts: dict[str, Any]) -> dict[str, Any]:
    """The acting tool's per-action facts -> a uniform after block. Only keys the tools really return are read."""
    note = str(facts.get("note", ""))
    # set_value returns no value read-back today (only value_matches), so after_value is unknown for it.
    if "value_matches" in facts or facts.get("method") == "ax_value":
        return {"read": "value", "changed": bool(facts.get("value_matches", facts.get("accepted"))),
                "value": facts.get("value"), "note": note}
    if "observed_change" in facts:        # False is "not confirmed changed" (a signature diff can miss a change): unknown
        return {"read": "ax", "changed": True if facts["observed_change"] else None, "value": facts.get("value"),
                "note": note}
    if isinstance(facts.get("bytes_match"), bool):       # browser upload: the page's selected file compared by sha256
        return {"read": "value", "changed": facts["bytes_match"], "value": None, "note": note}
    return {"read": "none", "changed": None, "value": None, "note": note}


def compare(expect: dict[str, Any], after: dict[str, Any]) -> dict[str, Any]:
    if "value_contains" in expect:
        if after.get("value") is None:
            return {"met": None, "evidence": "no value was read back after the action"}
        ok = str(expect["value_contains"]).casefold() in str(after["value"]).casefold()
        return {"met": ok, "evidence": f"after value {'contains' if ok else 'does not contain'} the expected text"}
    if "changed" in expect:
        if after.get("changed") is None:
            return {"met": None, "evidence": "nothing was read back after the action"}
        ok = bool(after["changed"]) == bool(expect["changed"])
        return {"met": ok, "evidence": f"the screen {'changed' if after['changed'] else 'did not change'} as "
                                       f"{'expected' if ok else 'not expected'}"}
    return {"met": None, "evidence": "no expectation to compare"}
