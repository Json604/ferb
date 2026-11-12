"""Owns wait.for: a worker waits in code, with no model calls, until one of its conditions holds or its time limit
passes, then gets one result saying what code saw. Must not read meaning into what it watches, act on a page, or wait
past the task's own time limit.

Owner, 2026-11-12 (ORCH-631): "lets just mkae tools the ai can use. and set an adequetate time lmits, maybe
workers/planner can set them too". Each condition is a structural fact code can check:
- process: one of the task's own shell commands exits, or its output (from `since`) holds `match`;
- file: a path exists, is gone, or exists and stopped changing (size and modification time) for `settle_s`;
- page: a task tab's text holds `match`, or (no match) its text differs from the first read; read every `every_s`
  (PAGE_FLOOR_S, ORCH-506's 30 s floor), optionally reloading it first (its own address again; nothing else is done
  to the page);
- url: a task tab's address holds `match`, or (no match) differs from when the wait began;
- mail: a new line naming `sender` / `subject` / `text` (each given one, case aside) appears in a task tab showing a
  mailbox (FERB's own or the owner's webmail, as the task opened it), read as a page is;
- download: a task tab's downloads record has a file the worker hasn't seen;
- time: a time (in_minutes from now, or at HH:MM, the next one);
- script: a command the worker writes runs every `every_s` (SCRIPT_FLOOR_S) in FERB's shell sandbox exactly as
  shell.run does (the call is a run_shell call for the gateway: snapshot, ledger, certificate and terminal rules);
  exit 0 means met; its last output is the evidence.
Any one met wakes the worker. A correction from the owner wakes it too ("correction"), and the time limit
("time_limit"): DEFAULT_LIMIT_MIN unless the worker sets one, never past the task's own limit. A stop or pause ends the
call as any call ends (cancelled); a restart ends it and the next run of the task is told (`wait.ended`)."""

import asyncio
import os
import re
import time as _time
from dataclasses import dataclass, field
from datetime import datetime, timedelta
from pathlib import Path, PurePosixPath
from typing import Any, Awaitable, Callable, Literal

from pydantic import BaseModel, Field, model_validator

from ferb.clock.when import When, WhenError, resolve
from ferb.core.errors import FerbError, Refused
from ferb.core.types import ActionSpec, Identity, RiskLevel, Target, ToolContext, ToolResult
from ferb.policy import urls
from ferb.policy.rules import Kind
from ferb.sanitize.terminal import printable

# Agent rulings (ORCH-632): the default time limit, the intervals' floors and how much a result carries
DEFAULT_LIMIT_MIN = 30.0
PAGE_FLOOR_S = 30.0               # ORCH-506: a page is read (or reloaded) at most every 30 s
PAGE_DEFAULT_S = 60.0
SCRIPT_FLOOR_S = 5.0
SCRIPT_DEFAULT_S = 30.0
SCRIPT_RUN_S = 60.0               # one run of a check script; past it the run is stopped and counts as not met
FILE_EVERY_S = 2.0                # a file, an address or a downloads record is looked at every 2 s
TICK_S = 1.0                      # the loop's beat: a process's new output, a correction
SETTLE_DEFAULT_S = 10.0
OUTPUT_TAIL = 2000                # the end of a command's or a script's output in a result
MATCH_LINES = 5                   # matching lines a result quotes
LINE_CHARS = 300
PAGE_ERRORS = 3                   # page reads in a row that fail before the wait ends with the error
QUOTE_CHARS = 40                  # model-written match text in the live line
NOT_FOUND = (126, 127)            # the shell's own codes: the command couldn't run or wasn't found
ORIGIN_OUTPUT = "external:unlabeled"

KINDS = ("process", "file", "page", "url", "mail", "download", "time", "script")

DESCRIPTION = (
    "Wait in code, with no AI calls, until any one of your conditions holds or the time limit passes; you get one "
    "result saying which condition woke you and what FERB saw (matching output lines, a file's size, the page lines "
    "that matched, a script's output). Use it instead of calling shell.wait, a page read or a sleep again and again. "
    "Conditions (any_of, any one wakes you): process {id, match?: wake when it exits or its output holds match}; "
    "file {path, state: exists | gone | settled (exists and stopped changing for settle_s)}; page {tab, match?: wake "
    "when its text holds match, or without match when it changes; every_s >= 30; reload: load its address again "
    "before each read}; url {tab, match?: its address holds match, or changes}; mail {tab showing a mailbox, sender / "
    "subject / text: a new line naming them appears}; download {tab: it downloads a file}; time {in_minutes or at "
    "\"HH:MM\"}; script {command, cwd?, every_s >= 5: runs in FERB's shell sandbox as shell.run does; exit 0 wakes "
    "you}. match is plain text, case aside (regex: true for a regular expression). limit_minutes: how long at most "
    "(default 30, longer or shorter as the work needs, up to the task's own limit). A message from the owner wakes "
    "you too. While you wait, the task's status shows what you wait for.")


class Condition(BaseModel):
    kind: Literal["process", "file", "page", "url", "mail", "download", "time", "script"]
    id: str = Field(default="", description="process: the command's id from shell.start")
    path: str = Field(default="", description="file: the path (absolute, ~, or relative to FERB's workspace)")
    state: Literal["exists", "gone", "settled", ""] = Field(default="", description="file: exists | gone | settled")
    settle_s: float = Field(default=SETTLE_DEFAULT_S, ge=2, le=3600)
    tab: str = Field(default="", description="page, url, mail, download: the task tab's id")
    match: str = Field(default="", description="process, page, url: the text to wait for")
    regex: bool = False
    since: int = Field(default=0, ge=0, description="process: output byte offset to look from (default: all of it)")
    reload: bool = False
    every_s: float = Field(default=0, ge=0, description="page, mail: >= 30 (default 60); script: >= 5 (default 30)")
    sender: str = ""
    subject: str = ""
    text: str = ""
    in_minutes: float = Field(default=0, ge=0)
    at: str = Field(default="", description="time: HH:MM, 24-hour local, the next one")
    command: str = Field(default="", description="script: the shell command; exit 0 means the condition holds")
    cwd: str = ""

    @model_validator(mode="after")
    def _fields(self) -> "Condition":
        need = {"process": self.id, "file": self.path and self.state, "page": self.tab, "url": self.tab,
                "mail": self.tab and (self.sender or self.subject or self.text), "download": self.tab,
                "time": self.in_minutes or self.at, "script": self.command.strip()}[self.kind]
        if not need:
            raise ValueError({"process": "a process condition needs id", "file": "a file condition needs path and "
                              "state", "page": "a page condition needs tab", "url": "a url condition needs tab",
                              "mail": "a mail condition needs tab and one of sender, subject, text",
                              "download": "a download condition needs tab",
                              "time": "a time condition needs in_minutes or at",
                              "script": "a script condition needs command"}[self.kind])
        if self.regex and self.match:
            try:
                re.compile(self.match)
            except re.error as e:
                raise ValueError(f"match isn't a valid regular expression: {e}") from None
        return self


class WaitInput(BaseModel):
    any_of: list[Condition] = Field(min_length=1, max_length=8)
    limit_minutes: float | None = Field(default=None, gt=0, description="default 30; at most the task's own limit")

    @model_validator(mode="after")
    def _one_script(self) -> "WaitInput":
        if sum(c.kind == "script" for c in self.any_of) > 1:
            raise ValueError("one script condition per wait: put both checks in one command")
        return self


@dataclass
class WaitDeps:
    """What the daemon hands the tool. `status(worker, task, until, line)`: the task's live line while it waits
    (line None clears it); `steered(task)`: how many corrections its tree has had; `ending()`: "restart" while FERB
    shuts down; `ended(task, data)`: journals a wait a restart cut; `monotonic` and `pause(seconds, wake)`: the
    loop's clock and rest (a test steps them, TEST-508)."""
    now: Callable[[], datetime]
    limit: Callable[[str], datetime | None]
    shells: Callable[[], Any]
    browsers: Callable[[Identity], Any]
    status: Callable[[str, str, datetime | None, str | None], None]
    steered: Callable[[str], int]
    ending: Callable[[], str | None]
    ended: Callable[[str, dict[str, Any]], None]
    monotonic: Callable[[], float] = _time.monotonic
    pause: Callable[[float, asyncio.Event], Awaitable[None]] = field(default=None)  # type: ignore[assignment]

    def __post_init__(self) -> None:
        if self.pause is None:
            self.pause = _pause


async def _pause(seconds: float, wake: asyncio.Event) -> None:
    """The loop's rest: until `seconds` pass or something wakes it (a process exited)."""
    try:
        await asyncio.wait_for(wake.wait(), seconds)
    except asyncio.TimeoutError:
        pass


def _err(msg: str) -> ToolResult:
    return ToolResult({"error": msg}, is_error=True)


def _clip(text: str, n: int = LINE_CHARS) -> str:
    one = " ".join(printable(text).split())
    return one if len(one) <= n else one[:n - 1].rstrip() + "…"


def _quoted(text: str) -> str:
    """Model-written match text in the live line: one printable line, cut, quoted as the task's words (ORCH-598)."""
    return f"“{_clip(text, QUOTE_CHARS).replace('“', '').replace('”', '')}”"


def _every(seconds: float) -> str:
    return f"every {int(seconds)} s" if seconds < 60 else f"every {round(seconds / 60):g} min"


def _host(url: str | None) -> str:
    u = urls.parse(url) if url else None
    return (u.host or "") if u is not None else ""


def _matcher(c: Condition) -> Callable[[str], bool]:
    if c.regex:
        pattern = re.compile(c.match)
        return lambda line: pattern.search(line) is not None
    want = c.match.casefold()
    return lambda line: want in line.casefold()


class _Watch:
    """One condition while it is watched: when it is next due, and what it last saw."""
    def __init__(self, index: int, c: Condition) -> None:
        self.index, self.c = index, c
        self.due = 0.0                       # monotonic time it is next looked at
        self.seen: dict[str, Any] = {}       # what it last saw (for a time-limit result)
        self.base: Any = None                # what the first look saw (page text, url, mail lines)
        self.errors = 0
        self.handle: Any = None              # process: its handle; script: none
        self.exited: asyncio.Task[Any] | None = None
        self.settled_since: float | None = None
        self.at: datetime | None = None      # time: when


class WaitFor:
    name = "wait.for"
    description = DESCRIPTION
    scopes = {"time"}
    identities = {Identity.OWNER, Identity.FERB}
    risk, reversible = RiskLevel.LOW, True
    origin = "ferb"                                    # FERB's own clock and records, unless a result says it read more
    Input = WaitInput

    def __init__(self, deps: WaitDeps) -> None:
        self.deps = deps

    # ---------- what the gateway checks ----------

    @staticmethod
    def _script(args: Any) -> Condition | None:
        return next((c for c in args.any_of if c.kind == "script"), None)

    def describe(self, args: BaseModel) -> ActionSpec:
        """A wait with a script is a shell command for the gateway, as shell.run is (ledger, snapshot, certificate
        and terminal rules); any other wait only reads."""
        script = self._script(args)
        if script is not None:
            return ActionSpec(Kind.RUN_SHELL, Target(path=script.cwd or "."), content=script.command)
        return ActionSpec(Kind.READ, Target())

    def file_paths(self, args: Any) -> list[tuple[str, str]]:
        """Each watched file, for the gateway's protection lists as an fs read is (FERB's own files refused)."""
        return [(c.path, "read") for c in args.any_of if c.kind == "file"]

    # ---------- the wait ----------

    async def run(self, args: BaseModel, ctx: ToolContext) -> ToolResult:
        a: Any = args
        try:
            watches, until = self._prepare(a, ctx)
        except (Refused, WhenError, FerbError) as e:
            return _err(str(e))
        line = self._line(watches, until, ctx)
        started, began = self.deps.monotonic(), self.deps.now()
        steered = self.deps.steered(ctx.task_id)
        wake = asyncio.Event()
        for w in watches:
            if w.handle is not None and w.c.kind == "process":
                w.exited = asyncio.ensure_future(self._exit_of(w, wake, until))
        self.deps.status(ctx.worker_id, ctx.task_id, until, line)
        sources: set[tuple[str, str]] = set()
        try:
            while True:
                now = self.deps.monotonic()
                for w in watches:
                    if now < w.due:
                        continue
                    hit = await self._look(w, ctx, sources)
                    if hit is not None:
                        return self._woke(w.c.kind, started, {"condition": w.index} | hit, sources)
                if self.deps.steered(ctx.task_id) > steered:
                    return self._woke("correction", started, {"note": "the owner sent a message to this task; read "
                                                              "it, then wait again if it still applies"}, sources)
                if self.deps.now() >= until:
                    return self._woke("time_limit", started, {
                        "limit": f"{until.astimezone():%H:%M}", "began": f"{began.astimezone():%H:%M}",
                        "last_seen": [{"condition": w.index, "kind": w.c.kind, **w.seen} for w in watches]},
                        sources)
                nxt = min([w.due for w in watches] + [self.deps.monotonic() + TICK_S])
                left = max(0.0, (until - self.deps.now()).total_seconds())
                wake.clear()
                await self.deps.pause(max(0.01, min(nxt - self.deps.monotonic(), left)), wake)
        except asyncio.CancelledError:
            if self.deps.ending() == "restart":
                try:
                    self.deps.ended(ctx.task_id, {"line": line, "began": began.isoformat(), "why": "restart"})
                except Exception:
                    pass
            raise
        finally:
            for w in watches:
                if w.exited is not None:
                    w.exited.cancel()
            self.deps.status(ctx.worker_id, ctx.task_id, None, None)

    def _woke(self, kind: str, started: float, evidence: dict[str, Any],
              sources: set[tuple[str, str]]) -> ToolResult:
        after = round(self.deps.monotonic() - started)
        return ToolResult({"woke": kind, "after_s": after, "evidence": evidence}, is_error="error" in evidence,
                          read_sources=tuple(sorted(sources)))

    # ---------- before the wait: every condition checked, its first look due now ----------

    def _prepare(self, a: Any, ctx: ToolContext) -> tuple[list[_Watch], datetime]:
        now = self.deps.now()
        limit = self.deps.limit(ctx.task_id)
        if a.limit_minutes is not None:
            until = now + timedelta(minutes=float(a.limit_minutes))
            if limit is not None and until > limit:
                raise Refused(f"that's past this task's time limit ({limit.astimezone():%H:%M}); the longest wait now "
                              f"is {max(0, int((limit - now).total_seconds() // 60))} minutes")
        else:
            until = now + timedelta(minutes=DEFAULT_LIMIT_MIN)
            if limit is not None:
                until = min(until, limit)
        watches = []
        for i, c in enumerate(a.any_of):
            w = _Watch(i, c)
            self._admit(w, ctx, now)
            watches.append(w)
            if c.kind == "time" and w.at is not None and w.at > until:
                raise Refused(f"condition {i}'s time ({w.at.astimezone():%H:%M}) is after this wait's limit "
                              f"({until.astimezone():%H:%M}); set limit_minutes to cover it")
        return watches, until

    def _scope(self, ctx: ToolContext, kind: str) -> None:
        need = {"process": "shell", "script": "shell", "file": "fs:read"}.get(kind)
        if need is None and kind in ("page", "url", "mail", "download"):
            need = "identity:browse" if ctx.identity is Identity.FERB else "browser:read"
        if need is not None and need not in ctx.scopes:
            raise Refused(f"this task can't use a {kind} condition (no {need})")

    def _admit(self, w: _Watch, ctx: ToolContext, now: datetime) -> None:
        c = w.c
        self._scope(ctx, c.kind)
        if c.kind == "process":
            shells = self.deps.shells()
            if shells is None:
                raise Refused("FERB's shell isn't available")
            w.handle = shells.handle(c.id, ctx.task_id)
            shells.info(w.handle)                       # Refused: not one of this task's commands
        elif c.kind in ("page", "url", "mail", "download"):
            driver = self.deps.browsers(ctx.identity)
            if driver is None:
                raise Refused("no browser is connected for this task")
            w.handle = driver.owned_by(c.tab, ctx.task_id)   # Refused: not this task's tab
            if c.kind in ("page", "mail"):
                if c.every_s and c.every_s < PAGE_FLOOR_S:
                    raise Refused(f"a page is read at most every {PAGE_FLOOR_S:g} s (every_s >= {PAGE_FLOOR_S:g})")
        elif c.kind == "script":
            from ferb.act.shell import guard
            if self.deps.shells() is None:
                raise Refused("FERB's shell isn't available")
            if c.every_s and c.every_s < SCRIPT_FLOOR_S:
                raise Refused(f"a script runs at most every {SCRIPT_FLOOR_S:g} s (every_s >= {SCRIPT_FLOOR_S:g})")
            guard.working_directory(c.cwd or None, ctx)
        elif c.kind == "time":
            w.at = resolve(When(in_minutes=c.in_minutes or None, time=c.at or None), now)
        elif c.kind == "file":
            w.handle = self._path(c.path, ctx)

    @staticmethod
    def _path(given: str, ctx: ToolContext) -> Path:
        p = Path(given).expanduser()
        return p if p.is_absolute() else Path(ctx.workspace) / p

    def _interval(self, c: Condition) -> float:
        if c.kind in ("page", "mail"):
            return max(PAGE_FLOOR_S, c.every_s or PAGE_DEFAULT_S)
        if c.kind == "script":
            return max(SCRIPT_FLOOR_S, c.every_s or SCRIPT_DEFAULT_S)
        if c.kind in ("file", "url", "download"):
            return FILE_EVERY_S
        return TICK_S

    # ---------- the live line ----------

    def _line(self, watches: list[_Watch], until: datetime, ctx: ToolContext) -> str:
        phrases = [self._phrase(w, ctx) for w in watches]
        first = phrases[0]
        if len(phrases) > 1:
            n = len(phrases) - 1
            first += f", or {n} other {'thing' if n == 1 else 'things'}"
        only_time = len(watches) == 1 and watches[0].c.kind == "time"
        return first if only_time else f"{first} (up to {until.astimezone():%H:%M})"

    def _phrase(self, w: _Watch, ctx: ToolContext) -> str:
        from ferb.orchestrator.progress import command_said
        c = w.c
        if c.kind == "process":
            shells = self.deps.shells()
            cmd = command_said(shells.info(w.handle).command) or "a command"
            return f"Waiting for {cmd} to print {_quoted(c.match)}" if c.match else f"Waiting for {cmd} to finish"
        if c.kind == "file":
            name = _clip(PurePosixPath(c.path.rstrip("/")).name or c.path, QUOTE_CHARS)
            if c.state == "exists":
                return f"Waiting for {name} to appear"
            if c.state == "gone":
                return f"Waiting for {name} to go away"
            downloads = any(part in ("Downloads", "downloads") for part in Path(w.handle).parts)
            return f"Waiting for {name} to finish {'downloading' if downloads else 'saving'}"
        if c.kind == "time":
            assert w.at is not None
            return f"Waiting until {w.at.astimezone():%H:%M}"
        if c.kind == "script":
            return f"Checking {command_said(c.command) or 'a command'} {_every(self._interval(c))}"
        host = _host(self.deps.browsers(ctx.identity).tab_url(c.tab)) or "a page"
        if c.kind == "page":
            what = _quoted(c.match) if c.match else "a change"
            return f"Watching {host} for {what} {_every(self._interval(c))}"
        if c.kind == "url":
            return (f"Waiting for the {host} tab's address to show {_quoted(c.match)}" if c.match
                    else f"Waiting for the {host} tab to go to another page")
        if c.kind == "mail":
            who = (f"from {_quoted(c.sender)}" if c.sender else f"about {_quoted(c.subject)}" if c.subject
                   else f"mentioning {_quoted(c.text)}")
            return f"Watching {host} for a new message {who} {_every(self._interval(c))}"
        return f"Waiting for a download in the {host} tab"

    # ---------- one look ----------

    async def _exit_of(self, w: _Watch, wake: asyncio.Event, until: datetime) -> None:
        """A process's exit, as it happens (the process's own wait), so the wait wakes at once."""
        shells = self.deps.shells()
        left = max(0.0, (until - self.deps.now()).total_seconds()) + 1
        await shells.wait(w.handle, left)
        w.due = 0.0                                      # looked at on this wake, not a beat later
        wake.set()

    async def _look(self, w: _Watch, ctx: ToolContext, sources: set[tuple[str, str]]) -> dict[str, Any] | None:
        """What this condition sees now: its evidence when it holds, else None (it is due again after its interval)."""
        c = w.c
        w.due = self.deps.monotonic() + self._interval(c)
        if c.kind == "time":
            assert w.at is not None
            w.due = self.deps.monotonic() + max(0.01, min(TICK_S, (w.at - self.deps.now()).total_seconds()))
            return {"at": f"{w.at.astimezone():%H:%M}"} if self.deps.now() >= w.at else None
        if c.kind == "process":
            return await self._process(w, sources)
        if c.kind == "file":
            return self._file(w)
        if c.kind == "script":
            return await self._run_script(w, ctx, sources)
        return await self._tab(w, ctx, sources)

    async def _process(self, w: _Watch, sources: set[tuple[str, str]]) -> dict[str, Any] | None:
        shells, c = self.deps.shells(), w.c
        try:
            info = shells.info(w.handle)
        except Refused as e:
            return {"error": str(e)}
        if c.match:
            start = w.base if w.base is not None else c.since
            text, offset, total = await shells.read(w.handle, start, None)
            w.base = offset
            if text:
                sources.add(("origin", ORIGIN_OUTPUT))
                matches = _matcher(c)
                hits = [_clip(line) for line in text.splitlines() if matches(line)]
                if hits:
                    return {"matched": hits[:MATCH_LINES], "running": info.running, "exit_code": info.exit_code,
                            "output_offset": offset}
        w.seen = {"running": info.running}
        if not info.running:
            text, offset, total = await shells.tail(w.handle, OUTPUT_TAIL)
            if text:
                sources.add(("origin", ORIGIN_OUTPUT))
            return {"exited": True, "exit_code": info.exit_code, "output_tail": text, "output_total": total,
                    **({"stopped_by_ferb": info.stopped} if info.stopped else {})}
        return None

    def _file(self, w: _Watch) -> dict[str, Any] | None:
        c, path = w.c, Path(w.handle)
        try:
            st = os.stat(path, follow_symlinks=False)
        except FileNotFoundError:
            st = None
        except OSError as e:
            return {"error": f"{e.strerror or e}: {path}"}
        facts = {"path": str(path), "exists": st is not None}
        if st is not None:
            facts |= {"bytes": st.st_size,
                      "modified": datetime.fromtimestamp(st.st_mtime).astimezone().isoformat(timespec="seconds")}
        w.seen = facts
        if c.state == "exists":
            return facts if st is not None else None
        if c.state == "gone":
            return facts if st is None else None
        key = (st.st_size, st.st_mtime_ns) if st is not None else None
        if key is None or key != w.base:
            w.base, w.settled_since = key, (self.deps.monotonic() if key is not None else None)
            return None
        held = self.deps.monotonic() - (w.settled_since or self.deps.monotonic())
        return facts | {"unchanged_s": round(held)} if held >= c.settle_s else None

    async def _run_script(self, w: _Watch, ctx: ToolContext, sources: set[tuple[str, str]]) -> dict[str, Any] | None:
        """One run of the worker's check, exactly as shell.run starts a command (the task's sandbox, its folders,
        FERB's proxy and download watch); its record goes once it has ended, so a long wait leaves no pile of them."""
        from ferb.act.shell import guard
        shells, c = self.deps.shells(), w.c
        h = await shells.start(c.command, guard.working_directory(c.cwd or None, ctx), ctx.task_id, False,
                               folders=ctx.folders)
        try:
            code = await shells.wait(h, SCRIPT_RUN_S)
            if code is None:
                await shells.kill(h)
            text, _, total = await shells.tail(h, OUTPUT_TAIL)
            network = shells.network(h)
            for d in network.get("destinations") or []:
                sources.add(("web", f"https://{d['host']}"))
            if text:
                sources.add(("origin", ORIGIN_OUTPUT))
            info = shells.info(h)
        finally:
            w.due = self.deps.monotonic() + self._interval(c)          # the next run counts from this one's end
            try:
                if shells.info(h).running:                  # the wait was stopped mid-run: the run goes with it
                    await shells.kill(h)
            except Refused:
                pass
            await shells.release(h)
        w.seen = {"runs": int(w.seen.get("runs", 0)) + 1, "exit_code": code, "output_tail": text}
        if code == 0:
            return {"exit_code": 0, "output_tail": text, "runs": w.seen["runs"]}
        if code in NOT_FOUND and w.seen["runs"] == 1:
            return {"error": "the script couldn't run (the shell's exit code "
                             f"{code}: not found or not executable)", "exit_code": code, "output_tail": text}
        if info.stopped:
            return {"error": "FERB stopped the script", "stopped_by_ferb": info.stopped, "output_tail": text}
        if code is None:
            w.seen["timed_out"] = f"a run took over {SCRIPT_RUN_S:g} s and was stopped"
        return None

    async def _tab(self, w: _Watch, ctx: ToolContext, sources: set[tuple[str, str]]) -> dict[str, Any] | None:
        c, driver = w.c, self.deps.browsers(ctx.identity)
        try:
            ref = driver.owned_by(c.tab, ctx.task_id)
            if c.kind == "url":
                url = driver.tab_url(c.tab) or ""
                w.seen = {"url": url}
                if w.base is None:
                    w.base = url
                if (c.match and _matcher(c)(url)) or (not c.match and url != w.base):
                    sources.add(("web", url))
                    return {"url": url}
                return None
            if c.kind == "download":
                fetch = getattr(driver, "downloads_for", None)
                found = list(await fetch(ref, 0.0)) if fetch is not None else []
                if found:
                    for f in found:
                        sources.add(("download", str(f.get("path") or f.get("source_url") or "")))
                    return {"downloads": found}
                return None
            if c.reload:
                url = driver.tab_url(c.tab)
                if url:
                    await driver.navigate(ref, url)
            snap = await driver.snapshot(ref)
        except Refused as e:
            return {"error": str(e)}
        except asyncio.CancelledError:
            raise
        except Exception as e:                           # a read that failed (a hung page): tried again next time
            w.errors += 1
            w.seen = {"error": f"{type(e).__name__}: {e}"}
            return {"error": f"the page couldn't be read {PAGE_ERRORS} times in a row: {e}"} \
                if w.errors >= PAGE_ERRORS else None
        w.errors = 0
        sources.add(("web", snap.url))
        if ctx.identity is Identity.OWNER:
            sources.add(("owner_account", f"{_host(snap.url)}|"))
        lines = [line for line in str(snap.text or "").splitlines() if line.strip()]
        w.seen = {"url": snap.url, "title": _clip(snap.title or "")}
        if c.kind == "page":
            if c.match:
                matches = _matcher(c)
                hits = [_clip(line) for line in lines if matches(line)]
                return {"url": snap.url, "matched": hits[:MATCH_LINES]} if hits else None
            if w.base is None:
                w.base = lines
                return None
            if lines != w.base:
                new = [_clip(line) for line in lines if line not in set(w.base)]
                return {"url": snap.url, "changed": True, "new_lines": new[:MATCH_LINES]}
            return None
        wanted = [s.casefold() for s in (c.sender, c.subject, c.text) if s]
        naming = [line for line in lines if all(s in line.casefold() for s in wanted)]
        if w.base is None:
            w.base = naming
            return None
        before = list(w.base)
        new = []
        for line in naming:                              # lines beyond those the first read had: new messages
            if line in before:
                before.remove(line)
            else:
                new.append(_clip(line))
        return {"url": snap.url, "new_lines": new[:MATCH_LINES]} if new else None
