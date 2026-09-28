"""Emergency stop (section 10.5): revoke all run tokens, block new actions at the gateway, cancel held sends,
and run every registered stop hook (workers, debuggers, task-owned processes register as they are built)."""

from dataclasses import dataclass, field
from typing import Any, Callable

StopHook = Callable[[], Any]


@dataclass
class StopControl:
    revoke_all_tokens: Callable[[], int]
    set_blocked: Callable[[bool], None]
    undo_all_holds: Callable[[], list[Any]]
    hooks: list[tuple[str, StopHook]] = field(default_factory=list)
    engaged: bool = False

    def register(self, name: str, hook: StopHook) -> None:
        self.hooks.append((name, hook))

    def engage(self) -> dict[str, Any]:
        """Order matters: block first (nothing new starts), then revoke, then cancel what's in flight."""
        self.set_blocked(True)
        self.engaged = True
        revoked = self.revoke_all_tokens()
        undone = self.undo_all_holds()
        hook_errors = {}
        for name, hook in self.hooks:
            try:
                hook()
            except Exception as e:           # one failing hook never stops the rest
                hook_errors[name] = type(e).__name__
        return {"tokens_revoked": revoked, "holds_undone": len(undone), "hooks": len(self.hooks),
                "hook_errors": hook_errors}

    def lift(self) -> None:
        self.set_blocked(False)
        self.engaged = False
