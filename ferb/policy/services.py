"""Describe installed apps and paired profiles as facts; never infer a requested service or route."""

import json
from dataclasses import dataclass, field


@dataclass
class Environment:
    apps: list[tuple[str, str]]
    # (browser, folder, name) or (browser, folder, name, account): the account the profile is signed in to (HAND-424)
    paired: list[tuple[str, ...]] = field(default_factory=list)


def summary(env: Environment, macos: str, running: set[str]) -> tuple[str, str]:
    """The planner's device facts. The installed-apps list isn't among them (owner, 2026-11-03, ORCH-514): a task's
    worker looks apps up with its Mac tools when a request involves one."""
    profiles = [{'browser': p[0], 'profile': p[1], 'name': p[2]} | ({'account': p[3]} if len(p) > 3 and p[3] else {})
                for p in env.paired]
    stable = f'This Mac: macOS {macos}.\nPaired owner profiles: ' + json.dumps(profiles, ensure_ascii=False)
    live = 'Running bundle IDs: ' + json.dumps(sorted(running))
    return stable, live
