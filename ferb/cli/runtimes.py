"""`ferb runtimes`: the capability table, what each role uses, and switching a role's runtime or model (D32).
Reads and writes config.toml directly (the daemon reads it on every call); no daemon needed."""

from pathlib import Path
from typing import TextIO

from ferb.config import Config
from ferb.runtimes.catalog import CATALOG, ROLES


def show(home: Path, out: TextIO) -> int:
    cfg = Config(home)
    print(f"{'runtime':<10}{'usable':<8}{'model':<20}{'tools':<9}{'cache':<9}{'key':<5}{'typed':<8}{'effort':<8}"
          f"{'images':<8}billing", file=out)
    for name, info in CATALOG.items():
        model = str(cfg.get("models", name) or info.default_model or "(default)")
        ok, fix = info.available(home)
        c = info.capabilities(model)
        print(f"{name:<10}{'yes' if ok else 'no':<8}{model:<20}{c.tools:<9}{c.cache:<9}{'yes' if c.cache_key else 'no':<5}"
              f"{c.structured_output:<8}{'yes' if c.reasoning_effort else 'no':<8}{'yes' if c.image_input else 'no':<8}"
              f"{c.billing}" + (f"   ({fix})" if fix else ""), file=out)
    worker = str(cfg.get("runtimes", "worker", "codex"))
    uses = {"orchestrator": str(cfg.get("runtimes", "orchestrator", "codex")), "worker": worker,
            "checker": str(cfg.get("runtimes", "checker", worker))}
    print("\n" + "\n".join(f"{role}: {uses[role]}" for role in ROLES), file=out)
    return 0


def use(home: Path, role: str, runtime: str, model: str | None, out: TextIO) -> int:
    if role not in ROLES:
        print(f"Unknown role {role!r}; roles: {', '.join(ROLES)}.", file=out)
        return 1
    if runtime not in CATALOG:
        print(f"Unknown runtime {runtime!r}; runtimes: {', '.join(CATALOG)}.", file=out)
        return 1
    cfg = Config(home)
    cfg.set("runtimes", role, runtime)
    if model:
        cfg.set("models", runtime, model)
    print(f"The {role} now uses {runtime}" + (f" with {model}" if model else "") + " from its next call.", file=out)
    return 0
