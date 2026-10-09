"""Owns making text from models, pages and apps safe to print on the owner's terminal. Must not change what the text
says: only control characters go, and nothing judges the text."""

import re

_CONTROL = re.compile(r"[\x00-\x08\x0b-\x1f\x7f-\x9f]")      # C0 but newline and tab, DEL, C1


def printable(text: str) -> str:
    """The text without C0/C1 control characters (and DEL); newlines and tabs stay. A worker's or a page's text then
    can't move the cursor, clear a line or overwrite FERB's own words on the owner's terminal (ESC, CR, CSI...)."""
    return _CONTROL.sub("", text)
