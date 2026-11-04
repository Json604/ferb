"""What FERB saw on this Mac (MEM-13), shared by the app's setup step and chat. It replaces the P28
persona question: nothing waits for the owner's yes.

`start()` gathers the facts (`collect`: code only, ferb/device/persona.py), has a model interpret them and stores the
conclusions as observed memory (`build`: the daemon's, ferb/memory/mac_profile.py), once; `status()` (polled by
`setup.persona` and `chat.status`) only reads it. The summary is presented once: the setup step shows it when it asked
for it; otherwise (an older install, chat opening first) it is posted in chat once (`tell`). A refresh (`refresh`, the
weekly memory pass) gathers again and re-interprets only when the gathered facts changed (`facts_fingerprint`), quietly."""
import asyncio
import inspect
from typing import Any, Awaitable, Callable

from ferb.device.persona import PersonaState, facts_fingerprint


class PersonaReview:
    def __init__(self, config: Any, collect: Callable[[], Any] | None, *,
                 build: Callable[[dict[str, Any]], Awaitable[dict[str, Any]]],
                 record: Callable[[str, dict[str, Any]], None],
                 tell: Callable[[str], Awaitable[None]] | None = None,
                 tell_joined: Callable[[list[str]], Awaitable[None]] | None = None,
                 redact: Callable[[Any], Any] = lambda x: x, clock: Callable[[], str] = lambda: "") -> None:
        self.config, self.collect, self.build, self.record, self.tell = config, collect, build, record, tell
        self.tell_joined = tell_joined
        self.state = PersonaState(config)          # persona.json in FERB's home, not config.toml
        self.redact, self.clock = redact, clock
        self.task: asyncio.Task[None] | None = None
        self.error: str | None = None
        self.closed = False

    def start(self, *, refresh: bool = False, shown_by: str = "chat") -> dict[str, Any]:
        """Build what FERB saw once (or again with `refresh`); `shown_by` "setup": the setup step presents it, so chat
        doesn't."""
        if self.closed:
            return {'state': 'closed'}
        if self.task is not None and not self.task.done():
            return self.status()
        if self.collect is None:
            return {'state': 'unavailable'}
        if refresh or not self.state.seen():
            self.error = None
            self.task = asyncio.create_task(self._discover(shown_by, quiet=False))
        return self.status()

    async def refresh(self) -> bool:
        """The weekly re-check: gather again; interpret and store again only when the facts changed. Quiet (the memory
        page shows it). True when it changed what FERB remembers."""
        if self.closed or self.collect is None or (self.task is not None and not self.task.done()):
            return False
        seen = self.state.seen()
        if not seen:
            return False
        facts = await self._gather()
        if facts_fingerprint(facts) == seen.get("fingerprint"):
            self.state.set_seen(seen | {"checked_at": self.clock(), "profiles_read": facts.get("profiles_read", True)})
            return False
        await self._store(facts, "quiet", quiet=True)
        return True

    async def _gather(self) -> dict[str, Any]:
        assert self.collect is not None
        collected = self.collect()
        facts = await collected if inspect.isawaitable(collected) else collected
        return dict(self.redact(facts))

    async def _discover(self, shown_by: str, *, quiet: bool) -> None:
        try:
            facts = await self._gather()
            self.record('setup.persona_scanned', {k: facts.get(k) for k in ('scanned_at', 'installed_apps',
                                                                          'apps_with_use')})
            await self._store(facts, shown_by, quiet=quiet)
        except asyncio.CancelledError:
            raise
        except Exception as exc:
            self.error = str(self.redact(f'{type(exc).__name__}: {exc}'))
            self.record('setup.persona_failed', {'error': self.error})

    async def _store(self, facts: dict[str, Any], shown_by: str, *, quiet: bool) -> None:
        stored = await self.build(facts)
        seen = {'summary': stored.get('summary', ''), 'shown': stored.get('shown', ''), 'facts': stored.get('facts', []),
                'at': self.clock(), 'checked_at': self.clock(), 'fingerprint': facts_fingerprint(facts),
                'profiles_read': facts.get('profiles_read', True),
                'shown_by': shown_by if not quiet else self.state.seen().get('shown_by', shown_by)}
        self.state.set_seen(seen)
        self.state.set_reviewed(True)
        self.record('memory.mac_profile_seen', {'facts': len(seen['facts']), 'quiet': quiet, 'shown_by': shown_by})
        if not quiet and shown_by != 'setup' and self.tell is not None and seen['shown']:
            await self.tell(str(seen['shown']))
        # MEM-427: the facts newly always on are shown by "here's what I saw" (the setup step or the chat notice);
        # when nothing shows them (a quiet re-check, or an empty summary), one notice tells them, after the rest
        shown = not quiet and (shown_by == 'setup' or bool(seen['shown']))
        if stored.get('joined') and not shown and self.tell_joined is not None:
            await self.tell_joined([str(i) for i in stored['joined']])

    def needs_browser_part(self) -> bool:
        """DMN-507: what FERB saw was gathered before it had Full Disk Access, so the browsers' profiles were left out;
        a `refresh` once access is there adds them (re-interpreted only when they change what FERB saw)."""
        seen = self.state.seen()
        return bool(seen) and seen.get("profiles_read", True) is False

    def status(self) -> dict[str, Any]:
        if self.closed:
            return {'state': 'closed'}
        if self.task is not None and not self.task.done():
            return {'state': 'scanning'}
        seen = self.state.seen()
        if seen:
            return {'state': 'seen', 'summary': seen.get('shown') or seen.get('summary', ''),
                    'facts': seen.get('facts', []), 'at': seen.get('at', '')}
        if self.error:
            return {'state': 'failed', 'error': self.error}
        return {'state': 'not_started'}

    async def close(self) -> None:
        self.closed = True
        if self.task is not None and not self.task.done():
            self.task.cancel()
            try:
                await self.task
            except asyncio.CancelledError:
                pass
