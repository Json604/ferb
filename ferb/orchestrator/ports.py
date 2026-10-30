"""Own what the supervisor needs from the rest of FERB, as a few typed ports handed over at construction. Must not
decide anything: each field is a daemon service the supervisor calls.

Every field is required: a port built without one fails where it is built (TypeError), so missing wiring is found at
start, never as a silent default that quietly does nothing. The supervisor copies the fields onto itself in
`__init__` (its own code reads `self.<name>`), so a test may still replace one afterwards."""

from dataclasses import dataclass
from datetime import datetime
from typing import Any, Awaitable, Callable


@dataclass(frozen=True)
class MemoryPort:
    """Memory and learning (L28, L29, L36)."""
    # the planner's ranked items for (its query, the target words of the tasks it is shown), or an awaitable of them
    memory_candidates: Callable[[str, str], Any]
    # a worker's or plan step's ranked memory for (its query, its task's target words); an awaitable
    memory_push: Callable[[str, str], Any]
    # chat capture (I1): (items, own words, source, sites=, elsewhere=) -> per item receipt or refusal (may await)
    remember: Callable[..., Any]
    forget_request: Callable[[str, str], str]           # a forget asked in chat (I2): (item id, request id) -> a line
    question_answered: Callable[[Any], str]             # FERB's own follow-up once the owner answered its question
    settle: Callable[[], Awaitable[None]]               # a forget still being finished completes first (I2)
    procedure_context: Callable[[str], str]
    procedure_details: Callable[[str, Any, dict[str, Any] | None], list[Any]]   # (procedure id, task, slot values)
    procedure_problem: Callable[[str], str | None]      # why a planner's procedure_id can't be used (I5)
    identity_context: Callable[[], str]
    on_episode: Callable[[str], Any]                    # a finished task's episode card (may return an awaitable)
    procedure_outcome: Callable[[str, str, dict[str, Any] | None], None]   # a procedure run's receipt
    slot_values: Callable[[str, list[dict[str, Any]]], dict[str, Any] | None]
    slot_problem: Callable[[str, list[dict[str, Any]]], str | None]
    typed_behind_memory: Callable[[str], list[str]]     # what the owner typed behind their remembered items (S3)


@dataclass(frozen=True)
class SchedulePort:
    """Durable timers and schedules."""
    timers: Any                                         # ferb.clock.timers.Timers
    # (spec, owner's words, request id, what the turn saw, what the request itself carried) -> a line
    on_schedule: Callable[[Any, str, str, set[str], set[str]], str]
    on_schedule_change: Callable[[Any, str], str]
    on_reminder_cancel: Callable[[Any, str], str]
    schedule_live: Callable[[str], bool]                # whether a schedule's runs may still hold its scope (D30)


@dataclass(frozen=True)
class EnvironmentPort:
    """What the Mac, FERB's tools and the owner's activity look like now."""
    environment: Callable[[str], Any]                   # installed apps and connections (may return an awaitable)
    availability: Callable[[], Any]                     # what the planner may use right now
    apps: Callable[[], list[tuple[str, str]]]           # (bundle id, name) from the device profile
    running_apps: Callable[[], list[tuple[str, str, int]]]   # (bundle id, name, pid) running now
    pasted_is_data: Callable[[], bool]                  # `[policy] pasted_text_is_data`
    relevant_context: Callable[[str], Any]              # (text, origin, payload, source refs), run off the loop
    worker_types: Callable[[], list[str]]
    signals: Callable[[], tuple[datetime | None, datetime | None]]   # (owner last active, last trail event)
    skills_catalog: Callable[[], list[dict[str, str]]]
    abilities_catalog: Callable[[], list[dict[str, Any]]]


@dataclass(frozen=True)
class DesktopPort:
    """The desktop observer and the change record around a task tree (P26, L39)."""
    on_task_start: Callable[[str], Awaitable[Any]]
    on_task_finish: Callable[[str], Awaitable[Any]]
    disturbance: Any                                    # the desktop sentinel
    disturbance_context: Callable[[str], str]
    changes: Any                                        # ferb.act.fs.changes.Changes


@dataclass(frozen=True)
class BrowserPort:
    """The browser drivers, as a task's tabs."""
    browser_context: Callable[[str], str]
    task_tabs: Callable[[str], list[dict[str, Any]]]    # the tabs a task holds now, redacted
    on_loop_rest: Callable[[str, bool], Awaitable[None]]   # a loop's round ends (True) or starts (False) (L31)


@dataclass(frozen=True)
class NotifyPort:
    """Telling the owner outside a chat turn."""
    on_loop_notice: Callable[..., Awaitable[None]]      # (tree id, step id, text, origin): a loop's notice (L31)
    on_resumed_outcome: Callable[[dict[str, Any]], Awaitable[None]]
    on_conversation_resume: Callable[[], None]
    notification_rule: Callable[[Any, str], str]


class QuestionRouter:
    """A worker's question on its way to the orchestrator: the gateway holds this from its construction and the
    supervisor binds its `route_question` when it is built, so neither needs the other to exist first."""

    def __init__(self) -> None:
        self._route: Callable[[str, str, list[str]], Awaitable[dict[str, Any]]] | None = None

    def bind(self, route: Callable[[str, str, list[str]], Awaitable[dict[str, Any]]]) -> None:
        self._route = route

    async def __call__(self, task_id: str, question: str, options: list[str]) -> dict[str, Any]:
        if self._route is None:
            raise RuntimeError("no orchestrator is bound to route worker questions")
        return await self._route(task_id, question, options)
