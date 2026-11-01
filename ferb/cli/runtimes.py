"""`ferb runtimes`: FERB's runtime table, what each role uses, and switching a role's runtime or model (D32).
Reads and writes config.toml directly (the daemon reads it on every call); no daemon needed."""

from pathlib import Path
from typing import Any, Iterable, TextIO

from ferb.config import Config
from ferb.runtimes.catalog import CATALOG, ROLES, model_for


def roles(cfg: Config) -> dict[str, str]:
    """Which runtime each role uses; the checker follows the worker unless set (D32)."""
    worker = str(cfg.get("runtimes", "worker", "codex"))
    return {"orchestrator": str(cfg.get("runtimes", "orchestrator", "codex")), "worker": worker,
            "checker": str(cfg.get("runtimes", "checker", worker))}


def status(home: Path) -> dict[str, Any]:
    """Each runtime with whether it can run on this Mac (and the fix when not), and what each role uses: the setup
    window's Choose your AI (L34 C) and `ferb runtimes`. Presence checks only: no key is read."""
    cfg = Config(home)
    rows = []
    for name, entry in CATALOG.items():
        model = model_for(cfg, name)
        signed, _ = entry.signed_in(home)
        ok, fix = entry.available(home, model)
        rows.append({"name": name, "usable": ok, "fix": fix, "signed_in": signed, "model": model})
    return {"roles": roles(cfg), "runtimes": rows}


def check_model(home: Path, runtime: str, model: str) -> None:
    """ValueError when FERB knows no documented limits for `model` on `runtime` (L28: it couldn't size a call for
    it), naming the runtime's own reason."""
    from ferb.core.errors import ModelLimitsUnknown
    if runtime == "codex":                  # Codex's windows come from its catalog: dumped once per pinned version
        from ferb.runtimes.codex_exec import bundled_catalog
        bundled_catalog(home)
    try:
        CATALOG[runtime].limits(model, home)
    except ModelLimitsUnknown as e:
        raise ValueError(f"FERB can't use {model!r} with {runtime}: {e}") from None


def assign(home: Path, runtime: str, chosen: Iterable[str] = ROLES, model: str | None = None) -> dict[str, str]:
    """Point the chosen roles at `runtime` (and its model, when given) from their next call. ValueError names an
    unknown role or runtime, or a model the runtime documents no limits for; nothing is written then."""
    chosen = list(chosen)
    if not chosen:
        raise ValueError(f"Name at least one role; roles: {', '.join(ROLES)}.")
    unknown = [role for role in chosen if role not in ROLES]
    if unknown:
        raise ValueError(f"Unknown role {unknown[0]!r}; roles: {', '.join(ROLES)}.")
    if runtime not in CATALOG:
        raise ValueError(f"Unknown runtime {runtime!r}; runtimes: {', '.join(CATALOG)}.")
    if model:
        check_model(home, runtime, model)
    cfg = Config(home)
    for role in chosen:
        cfg.set("runtimes", role, runtime)
    if model:
        cfg.set("models", runtime, model)
    return roles(cfg)


def show(home: Path, out: TextIO) -> int:
    from ferb.core.errors import ModelLimitsUnknown
    cfg = Config(home)
    print(f"{'runtime':<11}{'usable':<8}{'model':<22}{'billing':<14}{'sign-in':<9}window", file=out)
    for name, entry in CATALOG.items():
        model = model_for(cfg, name)
        ok, fix = entry.available(home, model)
        try:
            window = f"{entry.limits(model, home).context_window:,}"
        except ModelLimitsUnknown:
            window = "unknown"
        print(f"{name:<11}{'yes' if ok else 'no':<8}{model or '(none)':<22}{entry.billing:<14}{entry.sign_in:<9}"
              f"{window}" + (f"   ({fix})" if fix else ""), file=out)
    uses = roles(cfg)
    print("\n" + "\n".join(f"{role}: {uses[role]}" for role in ROLES), file=out)
    return 0


def use(home: Path, role: str, runtime: str, model: str | None, out: TextIO) -> int:
    try:
        assign(home, runtime, [role], model)
    except ValueError as e:
        print(str(e), file=out)
        return 1
    print(f"The {role} now uses {runtime}" + (f" with {model}" if model else "") + " from its next call.", file=out)
    ok, fix = CATALOG[runtime].available(home)
    if not ok:
        print(f"{runtime} isn't usable on this Mac yet: {fix}. The {role} will fail until then.", file=out)
    return 0


# `ferb runtimes pins` (developer toolbox): FERB's Codex and Claude Agent SDK pins against what npm and PyPI publish.
# It informs only: it never changes a pin (moving one is the checklist in RUN-411, "Moving the Codex and Claude
# pins"), and it reaches the network only when a developer runs it.
NPM_TAGS = "https://registry.npmjs.org/-/package/@openai/codex/dist-tags"
NPM_SEARCH = "https://registry.npmjs.org/-/v1/search?text=%40openai%2Fcodex&size=5"
PYPI = "https://pypi.org/pypi/claude-agent-sdk/json"


def _release(version: str) -> tuple[int, ...] | None:
    """A plain release's numbers ("0.166.0"); None for a pre-release or anything else."""
    parts = version.split(".")
    return tuple(int(p) for p in parts) if parts and all(p.isdigit() for p in parts) else None


def _age(published: str | None, now: Any) -> str:
    from datetime import datetime
    if not published:
        return ""
    try:
        when = datetime.fromisoformat(published.replace("Z", "+00:00"))
    except ValueError:
        return ""
    days = (now - when).total_seconds() / 86400
    return f", published {when.date().isoformat()} ({days:.0f} days ago)" if days >= 1 else \
        f", published {when.date().isoformat()} (under a day ago)"


def pin_report(get: Any, now: Any) -> list[str]:
    """One line per pin: pinned, newest published release and its age, and whether FERB is behind. `get(url)` returns
    the parsed JSON (tests pass a stand-in)."""
    from ferb.runtimes.claude_sdk import CLAUDE_CODE_VERSION, SDK_VERSION
    from ferb.runtimes.codex_exec import CODEX_VERSION
    lines = []
    latest = str(get(NPM_TAGS).get("latest", ""))
    date = next((o["package"].get("date") for o in get(NPM_SEARCH).get("objects", [])
                 if o.get("package", {}).get("name") == "@openai/codex" and o["package"].get("version") == latest), None)
    lines.append(_line("Codex (@openai/codex)", CODEX_VERSION, latest, _age(date, now)))
    pypi = get(PYPI)
    newest = str(pypi.get("info", {}).get("version", ""))
    files = pypi.get("releases", {}).get(newest) or [{}]
    behind = sorted(v for v in pypi.get("releases", {}) if (_release(v) or ()) > (_release(SDK_VERSION) or ()))
    lines.append(_line(f"Claude Agent SDK (claude-agent-sdk; bundles Claude Code {CLAUDE_CODE_VERSION})", SDK_VERSION,
                       newest, _age(files[0].get("upload_time_iso_8601"), now))
                 + (f"; {len(behind)} newer releases" if len(behind) > 1 else ""))
    return lines


def _line(name: str, pinned: str, latest: str, age: str) -> str:
    if not latest:
        return f"{name}: pinned {pinned}; the registry named no newest release"
    if _release(latest) is not None and (_release(latest) or ()) > (_release(pinned) or ()):
        return (f"{name}: pinned {pinned}, newest {latest}{age}: behind (see notes/runtimes.md \"Moving the "
                f"Codex/Claude pins\")")
    return f"{name}: pinned {pinned}, newest {latest}{age}: up to date"


def pins(out: TextIO) -> int:
    from datetime import datetime, timezone
    import httpx

    def get(url: str) -> Any:
        reply = httpx.get(url, timeout=20, follow_redirects=True)
        reply.raise_for_status()
        return reply.json()
    try:
        lines = pin_report(get, datetime.now(timezone.utc))
    except (httpx.HTTPError, ValueError, KeyError, TypeError) as e:
        print(f"Couldn't read npm or PyPI: {type(e).__name__}: {e}", file=out)
        return 1
    print("\n".join(lines), file=out)
    return 0
