"""One local P28 proposal shared by setup and chat; only owner review creates memory.

Its question ("Getting to know you") is asked once (L3): when the scan finishes, or when FERB starts with a scan still
unreviewed. `status()` (polled by `chat.status`) only reads it; the question never expires, so nothing re-asks it."""
import asyncio
import inspect
from typing import Any, Callable

from ferb.core.types import Identity
from ferb.device.persona import confirm_persona
from ferb.policy.rules import ActionRequest


class PersonaReview:
    def __init__(self, config: Any, approvals: Any, collect: Callable[[], Any] | None,
                 *, remember: Callable[[str], None], publish: Callable[[str, dict[str, Any]], None],
                 record: Callable[[str, dict[str, Any]], None], redact: Callable[[Any], Any] = lambda x: x) -> None:
        self.config, self.approvals, self.collect = config, approvals, collect
        self.remember, self.publish, self.record = remember, publish, record
        self.redact = redact
        self.task: asyncio.Task[None] | None = None
        self.approval_id: str | None = None
        self.error: str | None = None
        self.closed = False

    def start(self, *, refresh: bool = False) -> dict[str, Any]:
        if self.closed:
            return {'state': 'closed'}
        if self.task is not None and not self.task.done():
            return self.status()
        if self.collect is None:
            return {'state': 'unavailable'}
        if refresh:
            self.approvals.forget_request('setup-persona')
            self.approval_id = None
            self.config.set('setup', 'persona_reviewed', False)
            self.config.set('setup', 'persona_pending', {})
        if not self.config.get('setup', 'persona_reviewed', False):
            if not self._proposal():
                self.error = None
                self.task = asyncio.create_task(self._discover())
            elif self.approval_id is None:              # a scan this daemon hasn't asked about yet (a restart)
                self._ask()
        return self.status()

    async def _discover(self) -> None:
        try:
            assert self.collect is not None
            # Production collection is async and owns its subprocess. Quick synchronous fixture collectors
            # remain injectable without a thread that could outlive daemon shutdown.
            collected = self.collect()
            proposal = self.redact(await collected if inspect.isawaitable(collected) else collected)
            self.config.set('setup', 'persona_pending', proposal)
            self.record('setup.persona_scanned', {k: proposal[k] for k in ('scanned_at', 'apps', 'used_apps')})
            self._ask()
        except Exception as exc:
            self.error = str(self.redact(f'{type(exc).__name__}: {exc}'))
            self.record('setup.persona_failed', {'error': self.error})

    def _proposal(self) -> dict[str, Any]:
        return dict(self.config.get('setup', 'persona_pending', {}))

    def _ask(self) -> None:
        """The one question for this proposal, published once."""
        proposal = self._proposal()
        options = (['Save app-use profile'] if proposal['used_apps'] else []) + ['Skip']
        action = ActionRequest('setup.persona', 'owner_question', Identity.OWNER, frozenset(),
                               '', '', 'setup-persona', frozenset())
        ap = self.approvals.question(action, proposal['reason'], options)
        self.approval_id = ap.id
        self.publish('approval.requested', self.describe(ap))

    def describe(self, ap: Any) -> dict[str, Any]:
        return {'approval_id': ap.id, 'capability': ap.capability, 'kind': ap.kind,
                'reason': ap.reason, 'task_id': None, 'options': list(ap.options),
                'evidence': self._proposal().get('evidence', []), 'title': 'Getting to know you',
                'proposal': self._proposal().get('summary', '')}

    def status(self) -> dict[str, Any]:
        if self.closed:
            return {'state': 'closed'}
        if self.config.get('setup', 'persona_reviewed', False):
            return {'state': 'reviewed'}
        if self._proposal():
            ap = self.approvals.get(self.approval_id) if self.approval_id else None
            return {'state': 'pending', **({'approval': self.describe(ap)} if ap is not None else {})}
        if self.error:
            return {'state': 'failed', 'error': self.error}
        return {'state': 'scanning' if self.task is not None and not self.task.done() else 'not_started'}

    def resolve(self, ap: Any) -> None:
        if ap.capability != 'setup.persona':
            return
        proposal = self._proposal()
        answer = ('yes' if ap.choice == 'Save app-use profile' else 'skip' if ap.choice == 'Skip'
                  else ap.answer or 'yes')
        if ap.state == 'granted' and answer.strip().lower() in ('yes', 'y') and not proposal['used_apps']:
            raise ValueError('There is no app-use evidence to confirm. Describe how you use this Mac, or skip.')
        confirmed = confirm_persona(proposal['summary'], answer) if ap.state == 'granted' else None
        if confirmed is not None:
            confirmed = str(self.redact(confirmed))
            self.remember(confirmed)
            self.config.set('setup', 'persona', confirmed)
        self.config.set('setup', 'persona_reviewed', True)
        self.record('setup.persona_reviewed', {'confirmed': confirmed is not None,
                                            'scanned_at': proposal['scanned_at']})

    async def close(self) -> None:
        self.closed = True
        if self.task is not None and not self.task.done():
            self.task.cancel()
            try:
                await self.task
            except asyncio.CancelledError:
                pass
