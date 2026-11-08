"""Owns what Settings → AI's Check sends (T-1069, owner 2026-11-08: "yes lets run a few same tasks on codex, claude
code, anthropic api key, openai api key(i have credits), and a free gemini key"): the call shapes FERB really makes,
through the real runtime adapters, each kept tiny: a planner call with the real planner schema and its lookup tools
offered, a worker call that calls a tool with no arguments and one with arguments and answers in the worker's schema, a
checker call in the checker's schema, and a call carrying a small generated picture. Must not reach anything but the
model: the two check tools answer from memory and touch nothing.

Each line is a fact of the run: passed or not, the provider's words when not (redacted by the caller), the model, the
typed answer's form (RUN-427), time, tokens and cost."""

import struct
import zlib
from dataclasses import dataclass, field
from typing import Any

from pydantic import BaseModel, ConfigDict

from ferb.core.types import ActionSpec, Identity, RiskLevel, ToolContext, ToolResult

PING, ECHO = "check.ping", "check.echo"
PROMPT_PLANNER = ("This is FERB's connection check, not a real request. Use no tool. Answer with kind \"reply\", text "
                  "\"ok\", every object field null and every list empty, and every other field empty or false.")
PROMPT_WORKER = ("This is FERB's connection check. First call the tool check_ping (check.ping) with no arguments. Then call "
                 "the tool check_echo (check.echo) with text \"hello\". Then report: status \"done\", reason \"\", "
                 "question null, answer \"ok\", artifacts [], how \"\".")
PROMPT_CHECKER = ("This is FERB's connection check. Condition 0: the answer says ok. The answer: \"ok\". Judge it: results "
                  "[{index 0, met true, evidence \"the answer says ok\", how null}], next \"accept\", findings [], "
                  "and every other field empty or null.")
PROMPT_IMAGE = "This is FERB's connection check. What single colour fills the picture? Answer {\"colour\": <word>}."
IMAGE_SCHEMA = {"type": "object", "properties": {"colour": {"type": "string"}}, "required": ["colour"],
                "additionalProperties": False}


class CheckPing:
    """A tool for the check only: no arguments, answers "pong", touches nothing."""
    name = PING
    description = "FERB's connection check: call it with no arguments; it answers pong."
    scopes: set[str] = set()
    identities = {Identity.OWNER, Identity.FERB}
    risk, reversible, observation, origin = RiskLevel.LOW, True, True, "ferb"

    class Input(BaseModel):
        model_config = ConfigDict(extra="forbid")

    def __init__(self) -> None:
        self.calls = 0

    def describe(self, args: BaseModel) -> ActionSpec:
        return ActionSpec("read")

    async def run(self, args: BaseModel, ctx: ToolContext) -> ToolResult:
        self.calls += 1
        return ToolResult({"pong": True})


class CheckEcho:
    """A tool for the check only: echoes its text, touches nothing."""
    name = ECHO
    description = "FERB's connection check: call it with text; it answers with the same text."
    scopes: set[str] = set()
    identities = {Identity.OWNER, Identity.FERB}
    risk, reversible, observation, origin = RiskLevel.LOW, True, True, "ferb"

    class Input(BaseModel):
        model_config = ConfigDict(extra="forbid")
        text: str

    def __init__(self) -> None:
        self.texts: list[str] = []

    def describe(self, args: BaseModel) -> ActionSpec:
        return ActionSpec("read")

    async def run(self, args: BaseModel, ctx: ToolContext) -> ToolResult:
        self.texts.append(str(getattr(args, "text")))
        return ToolResult({"echo": str(getattr(args, "text"))})


def red_png(size: int = 16) -> bytes:
    """A small solid red PNG, made here: no screen is captured for the check."""
    def chunk(kind: bytes, data: bytes) -> bytes:
        return struct.pack(">I", len(data)) + kind + data + struct.pack(">I", zlib.crc32(kind + data) & 0xFFFFFFFF)
    rows = b"".join(b"\x00" + b"\xff\x00\x00" * size for _ in range(size))
    return (b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", size, size, 8, 2, 0, 0, 0))
            + chunk(b"IDAT", zlib.compress(rows)) + chunk(b"IEND", b""))


@dataclass
class Line:
    """One check's result as Settings shows it."""
    name: str
    ok: bool = False
    detail: str = ""
    model: str = ""
    typed: str = ""
    seconds: float = 0.0
    input_tokens: int = 0
    output_tokens: int = 0
    cost_usd: float | None = None
    extra: dict[str, Any] = field(default_factory=dict)

    def add_usage(self, data: dict[str, Any]) -> None:
        self.model = str(data.get("model") or self.model)
        self.typed = str(data.get("typed") or self.typed)
        self.input_tokens += int(data.get("input_tokens") or 0)
        self.output_tokens += int(data.get("output_tokens") or 0)
        cost = data.get("cost_usd")
        if isinstance(cost, (int, float)):
            self.cost_usd = (self.cost_usd or 0.0) + float(cost)

    def row(self) -> dict[str, Any]:
        return {"name": self.name, "ok": self.ok, "detail": self.detail, "model": self.model,
                "typed": self.typed or "none", "seconds": round(self.seconds, 1), "input_tokens": self.input_tokens,
                "output_tokens": self.output_tokens, "cost_usd": self.cost_usd}
