"""Render persisted daemon log events for inspection. Must not change their recorded meaning.

`ferb logs [--tasks --trail --tools --browsers]`: live stream from the daemon's bus."""

import json
from datetime import datetime
from typing import Any, TextIO

from ferb.cli.client import Client
from ferb.trace import lines


def format_event(ev: dict[str, Any]) -> str:
    if ev["topic"] == "tool.finished":
        d = ev["data"]
        return lines.step_line(ev["at"], d.get("tool", "?"), d.get("args") or {}, d.get("why") or "",
                               d.get("summary") or "", d.get("seconds"), d.get("error"))
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
    """`ferb voice`: push-to-talk as a conversation — what FERB heard you say, and what it said back."""
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


def voice_switch(client: Client, on: bool, out: TextIO) -> int:
    """`ferb voice on|off`: push-to-talk on or off without restarting FERB (the daemon starts or quits FERB.app)."""
    r = client.call("voice.switch", {"on": on})
    if r.get("needs_setup"):
        print("Push-to-talk isn't set up yet: run `ferb setup --voice` once.", file=out)
        return 1
    print("Push-to-talk is on: hold Right Option, speak, let go." if r.get("push_to_talk") else
          "Push-to-talk is off: FERB.app has quit and FERB won't listen or speak replies. `ferb voice on` turns it "
          "back on.", file=out)
    return 0
