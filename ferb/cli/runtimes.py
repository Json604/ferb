"""`ferb runtimes`: the capability table, what each role uses, and switching a role's runtime or model (D32).
Reads and writes config.toml directly (the daemon reads it on every call); no daemon needed."""

from pathlib import Path
from typing import Any, Iterable, TextIO

from ferb.config import Config
from ferb.runtimes.catalog import CATALOG, ROLES


def roles(cfg: Config) -> dict[str, str]:
    """Which runtime each role uses; the checker follows the worker unless set (D32)."""
    worker = str(cfg.get("runtimes", "worker", "codex"))
    return {"orchestrator": str(cfg.get("runtimes", "orchestrator", "codex")), "worker": worker,
            "checker": str(cfg.get("runtimes", "checker", worker))}


def _signed_in(name: str, home: Path, usable: bool) -> bool:
    """FERB's own Codex or Claude sign-in finished, or (a direct API) the provider's key is in the Keychain."""
    if name == "codex":
        from ferb.runtimes.codex_exec import codex_home
        return codex_home(home) is not None
    if name == "claude":
        from ferb.runtimes.claude_login import claude_home
        return claude_home(home) is not None
    return usable


def status(home: Path) -> dict[str, Any]:
    """Each runtime with whether it can run on this Mac (and the fix when not), and what each role uses: the setup
    window's Choose your AI (L34 C) and `ferb runtimes`. Presence checks only: no key is read."""
    cfg = Config(home)
    rows = []
    for name, info in CATALOG.items():
        ok, fix = info.available(home)
        rows.append({"name": name, "usable": ok, "fix": fix, "signed_in": _signed_in(name, home, ok),
                     "model": str(cfg.get("models", name) or info.default_model or "")})
    return {"roles": roles(cfg), "runtimes": rows}


def assign(home: Path, runtime: str, chosen: Iterable[str] = ROLES, model: str | None = None) -> dict[str, str]:
    """Point the chosen roles at `runtime` (and its model, when given) from their next call. ValueError names an
    unknown role or runtime; nothing is written then."""
    chosen = list(chosen)
    if not chosen:
        raise ValueError(f"Name at least one role; roles: {', '.join(ROLES)}.")
    unknown = [role for role in chosen if role not in ROLES]
    if unknown:
        raise ValueError(f"Unknown role {unknown[0]!r}; roles: {', '.join(ROLES)}.")
    if runtime not in CATALOG:
        raise ValueError(f"Unknown runtime {runtime!r}; runtimes: {', '.join(CATALOG)}.")
    cfg = Config(home)
    for role in chosen:
        cfg.set("runtimes", role, runtime)
    if model:
        cfg.set("models", runtime, model)
    return roles(cfg)


def show(home: Path, out: TextIO) -> int:
    cfg = Config(home)
    print(f"{'runtime':<10}{'usable':<8}{'model':<20}{'tools':<9}{'cache':<9}{'cache key':<11}{'typed':<8}{'effort':<8}"
          f"{'images':<8}billing", file=out)
    for name, info in CATALOG.items():
        model = str(cfg.get("models", name) or info.default_model or "(default)")
        ok, fix = info.available(home)
        c = info.capabilities(model)
        print(f"{name:<10}{'yes' if ok else 'no':<8}{model:<20}{c.tools:<9}{c.cache:<9}{'yes' if c.cache_key else 'no':<11}"
              f"{c.structured_output:<8}{'yes' if c.reasoning_effort else 'no':<8}{'yes' if c.image_input else 'no':<8}"
              f"{c.billing}" + (f"   ({fix})" if fix else ""), file=out)
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
