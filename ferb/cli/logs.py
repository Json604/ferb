"""`ferb logs [--tasks --trail --tools --browsers]`: live stream from the daemon's bus."""

import json
from datetime import datetime
from typing import Any, TextIO

from ferb.cli.client import Client


def format_event(ev: dict[str, Any]) -> str:
    at = datetime.fromisoformat(ev["at"]).strftime("%H:%M:%S")
    marker = "!" if ev["class"] == "critical" else ("*" if ev["class"] == "notice" else " ")
    data = json.dumps(ev["data"], separators=(",", ":"), ensure_ascii=False)
    return f"{at} {marker} {ev['topic']:<24} {data}"


def run(client: Client, filters: list[str], out: TextIO, limit: int | None = None) -> int:
    n = 0
    for ev in client.stream("logs.subscribe", {"filters": filters}):
        print(format_event(ev), file=out, flush=True)
        n += 1
        if limit is not None and n >= limit:
            break
    return 0


def voice(client: Client, out: TextIO, limit: int | None = None) -> int:
    """`ferb voice`: push-to-talk as a conversation — what FERB heard you say, and what it said back (section 20.1)."""
    print("Push-to-talk, live — hold Right Option to talk. Ctrl-C to leave (voice keeps working).", file=out, flush=True)
    n = 0
    for ev in client.stream("logs.subscribe", {"filters": ["voice"]}):
        text = str(ev.get("data", {}).get("text", ""))
        if ev.get("topic") == "voice.heard":
            print(f"\nyou (voice)> {text or '(nothing heard)'}", file=out, flush=True)
        elif ev.get("topic") == "voice.said":
            print(f"ferb> {text}", file=out, flush=True)
        n += 1
        if limit is not None and n >= limit:
            break
    return 0
