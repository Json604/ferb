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
