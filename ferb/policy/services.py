"""Describe installed apps and paired profiles as facts; never infer a requested service or route."""

import json
from dataclasses import dataclass, field


@dataclass
class Environment:
    apps: list[tuple[str, str]]
    paired: list[tuple[str, str, str]] = field(default_factory=list)


def summary(env: Environment, macos: str, running: set[str]) -> tuple[str, str]:
    installed = [{'bundle_id': bundle, 'name': name} for bundle, name in env.apps]
    profiles = [{'browser': browser, 'profile': folder, 'name': name} for browser, folder, name in env.paired]
    stable = (f'This Mac: macOS {macos}. Installed apps (inventory, not route selection): ' +
              json.dumps(installed, ensure_ascii=False) + '\nPaired owner profiles: ' +
              json.dumps(profiles, ensure_ascii=False))
    live = 'Running bundle IDs: ' + json.dumps(sorted(running))
    return stable, live
