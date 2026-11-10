"""A GatewayDeps of inert stand-ins, for tests that build a Gateway by hand (the daemon builds the real one)."""
from types import SimpleNamespace
from typing import Any

from ferb.daemon.gateway import GatewayDeps


def stub_deps(**overrides: Any) -> GatewayDeps:
    async def nothing(*args: Any, **kwargs: Any) -> None:
        return None
    store = SimpleNamespace()
    values: dict[str, Any] = dict(
        evidence=None, recovery=store, snapshots=None, changes=None, confirmations=store, destinations=store,
        proc_task=lambda proc_id: None, app_window=None,
        journal_problem=lambda kind, data: None, profile_name=lambda browser, folder: folder,
        profile_account=lambda browser, folder: "", disturbance=None,
        username_sources=lambda: None, owner_contacts=lambda: frozenset(), on_path=lambda path, task_id: None, route_question=None,
        shadow_action=None)
    values.update(overrides)
    return GatewayDeps(**values)
