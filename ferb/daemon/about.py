"""Own FERB's read-only self-description (notes/daemon.md decision 6): built from code and config each time, so it never
goes stale. Must not change anything, or carry a secret or a value the redactor would hide.

Three readers, one source:
- every role's prompt gets `summary` (in "Facts about this FERB", the shared path every role takes): small and stable
  (version, the models per role, where settings live, the lookup tool), never a live number, so the cacheable prefix
  stays the same between calls until the owner changes a model or FERB is updated;
- a worker reads the whole document with the `about.read` tool (`AboutRead`, an observation through the gateway);
- FERB.app's Settings → About shows the same document (`about.get`).

What it holds (`document`, assembled by the daemon's About area): version, models per role, FERB's own account,
permissions, the settings the owner changes in FERB.app (values from `settings.read`, places from the app's guide),
what FERB keeps about the owner, where and for how long (`retention.describe`: the rule table the hourly pass runs),
health (the database's size and clean-up state, FERB's CPU and memory from `resource_samples`), cost (the priced
model calls in traces) and FERB.app's how-tos and shortcuts (`APP_GUIDE`: the list FERB.app keeps with its screens and
hands to the daemon with `about.guide`)."""

from typing import Any, Callable

from pydantic import BaseModel, ConfigDict, Field

from ferb.core.types import ActionSpec, Identity, RiskLevel, ToolContext, ToolResult

SECTIONS = ("version", "models", "account", "permissions", "settings", "records", "health", "cost", "app")
# The app guide FERB.app sends (`about.guide`): at most this many entries of each list, each string at most this long
GUIDE_ITEMS, GUIDE_CHARS = 80, 400


def summary(version: str, models: dict[str, str]) -> str:
    """The always-on lines: no live numbers (prompt cache). `models`: role -> "runtime model"."""
    shown = "; ".join(f"{role} {model}" for role, model in models.items())
    return (f"- About FERB: version {version}; models: {shown}. FERB's settings (AI models, permissions, browsers, "
            "notifications, privacy and how long records are kept) are the owner's to change in FERB.app's Settings; "
            "FERB never changes them and tells the owner where they are. A worker reads FERB's full self-description "
            "(what it records about the owner, where and for how long, its permissions and settings, health, cost and "
            "how to do things in FERB.app) with the about.read tool.")


def clean_guide(value: Any) -> dict[str, Any]:
    """The app guide as FERB.app sent it, kept only in its shape: `howtos` [{title, steps}], `shortcuts` [{action,
    keys}], `places` {setting key: where in the app}; anything else is dropped and long strings cut."""
    def text(x: Any) -> str:
        return " ".join(x.split())[:GUIDE_CHARS] if isinstance(x, str) else ""

    data = value if isinstance(value, dict) else {}
    howtos = [{"title": text(h.get("title")), "steps": text(h.get("steps"))}
              for h in data.get("howtos", []) if isinstance(h, dict)][:GUIDE_ITEMS]
    shortcuts = [{"action": text(k.get("action")), "keys": text(k.get("keys"))}
                 for k in data.get("shortcuts", []) if isinstance(k, dict)][:GUIDE_ITEMS]
    raw: dict[Any, Any] = data["places"] if isinstance(data.get("places"), dict) else {}
    places = {text(k): text(v) for k, v in list(raw.items())[:GUIDE_ITEMS]}
    return {"howtos": [h for h in howtos if h["title"] and h["steps"]],
            "shortcuts": [k for k in shortcuts if k["action"] and k["keys"]],
            "places": {k: v for k, v in places.items() if k and v}}


class AboutRead:
    """FERB's self-description for a worker (decision 6): read-only, redacted, the same document FERB.app shows."""
    name = "about.read"
    description = ("Read FERB's own self-description: version and models, FERB's own account, permissions, the owner's "
                   "current settings and where each is changed in FERB.app, what FERB records about the owner (where "
                   "and for how long), health, cost, and how-tos and shortcuts for FERB.app. Read-only: settings are "
                   "the owner's to change in FERB.app; tell them where. `section` narrows it to one part.")
    scopes: set[str] = set()
    identities = {Identity.OWNER, Identity.FERB}
    risk, reversible, observation, origin = RiskLevel.LOW, True, True, "ferb"

    class Input(BaseModel):
        model_config = ConfigDict(extra="forbid")
        section: str | None = Field(default=None, description="One of: " + ", ".join(SECTIONS) + "; omit for all")

    def __init__(self, document: Callable[[], Any]) -> None:
        self.document = document          # the daemon's About area: an awaitable of the redacted document

    def describe(self, args: BaseModel) -> ActionSpec:
        return ActionSpec("read")

    async def run(self, args: BaseModel, ctx: ToolContext) -> ToolResult:
        a = self.Input.model_validate(args)
        if a.section is not None and a.section not in SECTIONS:
            return ToolResult({"error": "section must be one of: " + ", ".join(SECTIONS)}, is_error=True)
        doc = await self.document()
        return ToolResult({a.section: doc.get(a.section)} if a.section else doc)
