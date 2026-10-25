"""Explicit clipboard writes, without focus or keyboard input (D36). Capture owns subsequent attribution."""
from typing import Callable

from pydantic import BaseModel

from ferb.core.types import ActionSpec, Identity, RiskLevel, ToolContext, ToolResult
from ferb.policy.rules import Kind


def write_general(text: str) -> int:
    from AppKit import NSPasteboard, NSPasteboardTypeString
    board = NSPasteboard.generalPasteboard()
    board.clearContents()
    if not board.setString_forType_(text, NSPasteboardTypeString):
        raise RuntimeError("the pasteboard did not accept the text")
    return int(board.changeCount())


CONCEALED = frozenset({"org.nspasteboard.ConcealedType", "org.nspasteboard.TransientType"})


def read_general() -> tuple[int, str | None, bool]:
    """The general pasteboard now: (change count, its text or None, whether an app marked the item secret). Read for
    the owner's question before a paste into a terminal (D31); a concealed item's text is not returned."""
    from AppKit import NSPasteboard, NSPasteboardTypeString
    board = NSPasteboard.generalPasteboard()
    types = {str(t) for t in (board.types() or ())}
    concealed = bool(CONCEALED & types)
    text = None if concealed else board.stringForType_(NSPasteboardTypeString)
    return int(board.changeCount()), (str(text) if text is not None else None), concealed


class MacClipboard:
    name = "mac.clipboard"
    description = "Write text to the Mac clipboard without changing focus. Reports the clipboard change count."
    identities = {Identity.OWNER, Identity.FERB}
    scopes = {"mac:act"}
    risk, reversible, on_screen = RiskLevel.MEDIUM, False, False

    class Input(BaseModel):
        text: str

    def __init__(self, write: Callable[[str], int] = write_general,
                 on_write: Callable[[int, str], None] | None = None) -> None:
        self.write, self.on_write = write, on_write

    def describe(self, args: BaseModel) -> ActionSpec:
        return ActionSpec(Kind.MAC_INPUT, content=getattr(args, "text"))

    async def run(self, args: BaseModel, ctx: ToolContext) -> ToolResult:
        value = getattr(args, "text")
        count = self.write(value)
        if self.on_write is not None:
            self.on_write(count, ctx.task_id)
        return ToolResult({"characters": len(value), "change_count": count})
