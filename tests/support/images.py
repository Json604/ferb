"""Synthetic pictures for masking tests, made with Quartz (the screenshot path's image library; FERB has no Pillow), and
a probe tool that returns one."""

from typing import Any

from pydantic import BaseModel

from ferb.core.types import ActionSpec, Identity, RiskLevel, Target, ToolResult
from ferb.policy.rules import Kind


def solid_jpeg(w: int, h: int) -> bytes:
    """An all-white JPEG of this size."""
    import Quartz
    from Foundation import NSMutableData
    ctx = Quartz.CGBitmapContextCreate(None, w, h, 8, 0, Quartz.CGColorSpaceCreateDeviceRGB(),
                                       Quartz.kCGImageAlphaNoneSkipLast)
    Quartz.CGContextSetRGBFillColor(ctx, 1.0, 1.0, 1.0, 1.0)
    Quartz.CGContextFillRect(ctx, Quartz.CGRectMake(0, 0, w, h))
    data = NSMutableData.data()
    dest = Quartz.CGImageDestinationCreateWithData(data, "public.jpeg", 1, None)
    Quartz.CGImageDestinationAddImage(dest, Quartz.CGBitmapContextCreateImage(ctx),
                                      {Quartz.kCGImageDestinationLossyCompressionQuality: 0.95})
    assert Quartz.CGImageDestinationFinalize(dest)
    return bytes(data)


def black(px: tuple[int, int, int]) -> bool:
    """Painted over (JPEG: within a few levels of black)."""
    return max(px) <= 8


def white(px: tuple[int, int, int]) -> bool:
    """Untouched white."""
    return min(px) >= 247


class ImageTool:
    """A reading tool that returns an image, as mac.screenshot / browser.screenshot / mac.snapshot do."""
    identities = {Identity.OWNER, Identity.FERB}
    risk, reversible = RiskLevel.LOW, True

    class Input(BaseModel):
        pass

    def __init__(self, name: str, image: bytes, content: dict[str, Any],
                 reads: tuple[tuple[str, str], ...] = (("app", "com.example.App"),)) -> None:
        self.name, self.description, self.scopes = name, name, {"probe"}
        self.image, self.content, self.reads = image, content, reads

    def describe(self, args: Any) -> ActionSpec:
        return ActionSpec(Kind.READ, Target(site="x.example"))

    async def run(self, args: Any, ctx: Any) -> ToolResult:
        return ToolResult(dict(self.content), read_sources=self.reads, image=self.image)
