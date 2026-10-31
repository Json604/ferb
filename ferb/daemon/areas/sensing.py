"""Own the watchers and trail wiring, sensing pause, perception's attention considerations and notification rules. Must
not record what the owner excluded."""

import asyncio
import json
from datetime import datetime
from pathlib import Path
from typing import Any

from ferb.core import origin as origin_
from ferb.core.types import Identity
from ferb.daemon.server import Session
from ferb.sense.exclusions import DEFAULT_EXCLUDE_PATHS, Exclusions
from ferb.sense.manager import Sense
from ferb.sense.trail import Trail


class Sensing:
    """A mixin of the Daemon (ferb/daemon/main.py): its methods use the daemon's state."""

    def _exclusions(self) -> Exclusions:
        """[privacy] lists, re-read when config.toml changes (the owner may edit it while FERB runs)."""
        try:
            mtime = self.config.path.stat().st_mtime
        except OSError:
            mtime = 0.0
        if self._excl is None or self._excl[0] != mtime:
            self._excl = (mtime, Exclusions.build(self.config.exclude_apps(),
                                                  self.config.get("privacy", "exclude_domains", []),
                                                  self.config.get("privacy", "exclude_paths", DEFAULT_EXCLUDE_PATHS)))
        return self._excl[1]

    def _watch_folders(self) -> list[Path]:
        from ferb.sense.watchers.files import valid_folders
        ok, self.watch_problems = valid_folders(list(self.config.get("sense", "watch_folders", [])), self._exclusions())
        for problem in self.watch_problems:
            self._journal().append("sense.folder_skipped", {"why": problem})
        return ok

    async def _build_sense(self) -> None:
        from ferb.sense.watchers.app_focus import AppFocusWatcher
        from ferb.sense.watchers.app_installs import AppInstallsWatcher
        from ferb.sense.watchers.browser_tabs import ScriptedTabsWatcher
        from ferb.sense.watchers.files import FilesWatcher
        from ferb.sense.watchers.idle import IdleWatcher
        from ferb.sense.watchers.windows import WindowTitleWatcher
        assert self.db is not None
        a = self.attribution
        a.shell_cwds = lambda: [(p.cwd, p.owner_task) for p in (self.shells.list() if self.shells else [])
                                if p.running]
        self.trail = Trail(self.db, self.clock, self.ids, self.redact, self._exclusions, self._publish,
                           lambda: self.device_id,
                           lambda: self.sense is not None and self.sense.paused, a)
        if self.mac is not None:
            self.mac.on_activate = a.ferb_activated
            self.mac.acting = a.acting
        if self.hub is not None:
            self.hub.on_tab = self._on_tab
            self.hub.paused = lambda: self.sense is not None and self.sense.paused
            self.hub.owned = lambda br, tab_id: self._tab_task(br, tab_id) is not None
            self.hub.on_sound = self._tab_sound            # hands decision 5: playback changes as disturbance facts
        from ferb.sense.perception import Perception
        self.perception = Perception(self)
        self.perception.bind_clipboard()
        self.perception.purge_text()          # trail.tag stays a no-op unless Jev is switched on (_wire_jev)
        watchers: list[Any] = [self.perception]
        if self.run_watchers:
            watchers += [AppFocusWatcher(), WindowTitleWatcher(lambda: bool(self.config.get("sense", "window_titles",
                                                                                           True))),
                        IdleWatcher(lambda: self.owner_input.last_owner_wall, lambda: self.owner_input.running,
                                    float(self.config.get("sense", "idle_after_s", 120))),
                        FilesWatcher(self._watch_folders), AppInstallsWatcher(self._app_changed),
                        ScriptedTabsWatcher(self._automation_granted, self._any_extension_connected)]
            watchers += self.perception.native_watchers()
        self.sense = Sense(self.home, self.trail, watchers, self.owner_input if self.watch_owner_input else None,
                           on_pause=self._on_pause, on_fail=self._watcher_failed,
                           presence=self.presence if self.watch_owner_input else None)
        await self.sense.start()
        if self.sense.paused:
            self._journal().append("sense.still_paused", {})
        elif self.watch_owner_input and not self.owner_input.running:
            self._journal().append("owner_input.unavailable", {"why": "no event tap (Accessibility / Input Monitoring)"})

    def _watcher_failed(self, source: str, why: str) -> None:
        self._journal().append("sense.watcher_failed", {"source": source, "error": why})

    def _tab_task(self, br: Any, tab_id: int) -> str | None:
        for driver in (self.router, self.ferb_identity):
            owner = driver.owner_of(br, tab_id) if driver is not None else None
            if owner is not None:
                return owner
        return None

    def _tab_sound(self, br: Any, e: Any) -> None:
        """Hands decision 5: a tab that isn't FERB's started or stopped sound, or was muted or unmuted (the browser's
        own facts): a disturbance fact when one of FERB's calls was near (`DisturbanceSentinel.sound_changed`). FERB's
        own tabs (opened by it, or in its window) are left out, and so is a mute the owner changed themselves (the
        browser's `mutedInfo.reason` "user", protocol 21: hands decision 11). Nothing about the page is kept."""
        if e.ferb:
            return
        owners_mute = getattr(e, "muted_reason", None) == "user"
        change = {k: {"before": not bool(getattr(e, k)), "after": bool(getattr(e, k))}
                  for k in ("audible", "muted") if k in e.changed and getattr(e, k) is not None
                  and not (k == "muted" and owners_mute)}
        tab = {"browser": br.name, "tab_id": e.tab_id} | ({"task": task} if (task := self._tab_task(br, e.tab_id))
                                                          else {})
        self.disturbance.sound_changed(tab, change)

    def _on_tab(self, br: Any, e: Any) -> None:
        from ferb.sense.watchers.browser_tabs import tab_observation
        if self.trail is None:
            return
        o = tab_observation(br.binding.browser, br.binding.folder, br.binding.identity is Identity.FERB, e.event,
                            e.url, e.title, e.incognito, self._tab_task(br, e.tab_id))
        if o is not None:
            self.trail.record(o)

    def _on_pause(self, paused: bool) -> None:
        if paused and self.hub is not None:
            self.hub.forget_owner_tabs()
        self._journal().append("sense.paused" if paused else "sense.resumed", {})
        self._publish("sense.paused" if paused else "sense.resumed", {})

    def _any_extension_connected(self, browser: str) -> bool:
        return self.hub is not None and any(br.conn is not None and br.binding.browser == browser
                                            for br in self.hub.bridges.values())

    def signals(self) -> tuple[datetime | None, datetime | None]:
        """For the time header: owner last active (owner-input tap), last trail event."""
        last = self.owner_input.last_owner_wall
        owner = datetime.fromtimestamp(last).astimezone() if last else None
        return owner, self.trail.last_event_at if self.trail else None

    async def _sense_pause(self, session: Session, params: dict[str, Any]) -> dict[str, Any]:
        assert self.sense is not None
        await self.sense.pause()
        return {"paused": True}

    async def _sense_resume(self, session: Session, params: dict[str, Any]) -> dict[str, Any]:
        assert self.sense is not None
        await self.sense.resume()
        return {"paused": False, "watchers": self.sense.running}

    async def _sense_status(self, session: Session, params: dict[str, Any]) -> dict[str, Any]:
        s = self.sense
        return {"paused": bool(s and s.paused), "watchers": s.running if s else [],
                "failed": dict(s.failed) if s else {},
                "owner_input": self.owner_input.running, "folder_problems": self.watch_problems,
                "perception": self.perception.status() if self.perception is not None else {}}

    def _attention_effect(self, decision: str, effect: str) -> None:
        """Q4/C3: what Jev's live trail.attention yes led to, on its decision row (`ferb decide stats`)."""
        if self.jev is not None:
            self.jev.mark_effect('trail.attention', decision, effect)

    def _attention_app(self, event: dict[str, Any]) -> str:
        """The app a Jev yes batch is about: that of its first observation row (a batch never spans apps)."""
        from ferb.sense.batches import app_of
        for ref in event.get('source_refs', []):
            if self.trail is not None and isinstance(ref, str) and ref.startswith('trail:'):
                row = self.trail.row(ref[6:])
                if row is not None and row['source'] != 'notification':
                    return app_of(json.loads(row['data_json']))
        return ''

    def _attention_arrived(self, event: dict[str, Any]) -> None:
        """L10, one moment one message: at most one consideration runs per app. A yes batch for an app whose
        consideration is running waits and merges into that app's next consideration; there is no timer."""
        app = self._attention_app(event)
        waiting = self._attention_waiting.get(app)
        if waiting is not None:
            waiting.append(event)
            return
        self._attention_waiting[app] = [event]
        job = asyncio.create_task(self._attention_turns(app))
        self._runs.add(job)
        job.add_done_callback(self._runs.discard)

    async def _attention_turns(self, app: str) -> None:
        """Consider an app's waiting batches together, then whatever arrived meanwhile, until none waits."""
        try:
            while self._attention_waiting[app]:
                events, self._attention_waiting[app] = self._attention_waiting[app], []
                await self._consider_attention(*events)
        finally:
            del self._attention_waiting[app]

    async def _consider_attention(self, *events: dict[str, Any]) -> None:
        """One consideration of one or more yes batches about one app (merged, rows in the order they happened). Every
        batch's decision records the outcome. Already told is scoped to the rows' app (L10)."""
        from ferb.sense.batches import app_of
        decisions = list(dict.fromkeys('batch:' + str(event.get('batch_id')) for event in events))

        def effect(text: str) -> None:
            together = f' (one consideration for {len(decisions)} batches)' if len(decisions) > 1 else ''
            for decision in decisions:
                self._attention_effect(decision, text + together)
        if self.supervisor is None or self.db is None or self.attention_notices is None:
            effect('not considered: the orchestrator is not running')
            return
        try:
            found = []
            for ref in dict.fromkeys(ref for event in events for ref in event.get('source_refs', [])):
                if not isinstance(ref, str) or not ref.startswith('trail:'):
                    continue
                row = self.trail.row(ref[6:]) if self.trail is not None else None
                if row is None or row['source'] == 'notification':
                    continue                 # notification decisions remain hard shadow, including mixed batches
                found.append((row['seq'], ref, {'id': row['id'], 'at': row['at'], 'source': row['source'],
                                                'actor': row['actor'], 'task_id': row['task_id'],
                                                'data': json.loads(row['data_json']), 'origin': 'external:observation'}))
            found.sort(key=lambda item: item[0])                  # merged batches read in the order they happened
            refs, rows = [ref for _, ref, _ in found], [row for _, _, row in found]
            if not rows:
                effect('not considered: no observation rows left (notifications stay shadow)')
                return
            told = self.attention_notices.told({app_of(row['data']) for row in rows})
            result = await self.supervisor.attention(refs, rows, told)
            if result.get('tell') is not True or not str(result.get('text', '')).strip():
                effect('considered; nothing told')
                return
            notice = self.attention_notices.add(decisions[0], result['source_refs'], str(result['text']),
                                                result.get('quotes'), refs,
                                                origin_.least_trusted(row['origin'] for row in rows))
            if notice is not None:                    # posted once: a replayed batch never posts again
                await self._post_notice(notice)
                effect('told in chat: notice ' + notice['id'])
        except Exception as exc:
            effect('consideration failed: ' + type(exc).__name__)
            self._journal().append('attention.failed', {'batch_ids': [str(e.get('batch_id')) for e in events],
                                                        'error': type(exc).__name__})
