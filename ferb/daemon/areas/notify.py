"""Own telling the owner outside a chat turn: banners (FERB.app or osascript), held banners, results told, notices in
chat. Must not decide what is worth telling: callers do."""

import asyncio
import json
from typing import Any, AsyncIterator

from ferb.core import origin as origin_
from ferb.core.errors import FerbError
from ferb.daemon import banners
from ferb.daemon.areas.shared import _no_notify
from ferb.daemon.banners import Banner
from ferb.daemon.server import Session


# `osascript -e 'display notification …'` posts under Script Editor's bundle id (Notification Center's app identifier).
OSASCRIPT_POSTER = "com.apple.ScriptEditor2"


class Notify:
    """A mixin of the Daemon (ferb/daemon/main.py): its methods use the daemon's state."""

    async def _osascript_notify(self, title: str, text: str) -> None:            # posts as OSASCRIPT_POSTER
        """A macOS notification (the OS's own, not FERB UI). Text is redacted and quoted for AppleScript."""
        def q(v: str) -> str:
            return '"' + str(self._redact_text(v)).replace("\\", "\\\\").replace('"', '\\"') + '"'
        proc = await asyncio.create_subprocess_exec("osascript", "-e", f"display notification {q(text)} with title "
                                                    f"{q(title)}", stdout=asyncio.subprocess.DEVNULL,
                                                    stderr=asyncio.subprocess.DEVNULL)
        await asyncio.wait_for(proc.wait(), 10)

    async def _task_result(self, ev: dict[str, Any], shown: bool, alert: bool = False, publish: bool = True) -> None:
        """What FERB told (or will tell) the owner about a task; a macOS notification only when chat was closed and no
        connected FERB client receives `task.result` to show it (L9). An `alert` (a watch's condition met) is told the
        way a reminder is: a notification now, whether or not a client shows it, even while the owner is away. Without
        `publish` (a watch check with nothing to tell) it is only recorded: no client hears of it (fix wave 6c, I3)."""
        assert self.db is not None
        ev = self.redact(ev)
        answer = str(ev.get("answer") or ev.get("summary") or "")
        shown = shown or ev.get("status") == "cancelled"          # stopped by the owner: they know
        at = str(ev.get("at") or self.clock.now().isoformat())    # the outcome's own time when a chat turn sent it
        assert self.task_results is not None
        cards = [c for c in ev.get("cards") or [] if isinstance(c, dict)]
        self.task_results.record(ev["task_id"], at, ev.get("status", ""), str(ev.get("goal", "")), answer, shown,
                                 cards)
        if not publish:
            return
        task = {"kind": "task", "task_id": str(ev["task_id"])}
        if alert:
            await self._notify("FERB alert", answer, urgent=True, category="alert", target=task)
        elif ev.get("status") != "cancelled" and (self.posters.connected or (
                not shown and not self._client_shows("task.result"))):
            # L34 D: with FERB.app as the poster it decides by the notch: a banner while the notch isn't showing the
            # answer, whichever chat showed it; without it, as before (L9)
            await self._notify("FERB finished" if ev.get("status") == "done" else "FERB", answer,
                               category="task_finished", target=task, when_hidden=self.posters.connected,
                               ident=f"task:{ev['task_id']}")
        if "request_id" not in ev:                  # a resumed plan or scheduled run: the request its task ran for
            ev = {**ev, "request_id": self.tasks.column(str(ev["task_id"]), "request_id")}
        self._publish('task.result', {**ev, 'at': at, 'shown': shown})

    def _approval_notice(self, data: dict[str, Any]) -> None:
        """An approval is waiting and no chat is open for that task: tell the owner (notification), only when no
        connected FERB client receives `approval.requested` to show it (L9, spec 8.4; final review M2). The banner
        names the question, never a CLI command (spec 4.4). The setup profile question is never a banner (L3): an open
        client (`chat.attach`, `ferb setup`) started it and shows it inline."""
        tid = str(data.get("task_id") or "")
        if data.get("capability") == "setup.persona":
            return
        text = f"FERB needs your OK: {str(data.get('summary') or data.get('reason', ''))}"   # the short line first
        if self.posters.connected:          # L34 D: FERB.app shows it unless the notch already does
            approval = str(data.get("approval_id") or "")
            self._spawn(self._notify("FERB needs you", text, urgent=True, category="question",
                                     target={"kind": "question", "approval_id": approval,
                                             "task_id": tid.split(".", 1)[0]},
                                     when_hidden=True, ident=f"question:{approval}"), "approval banner")
            return
        if tid.split(".", 1)[0] in self._attached_trees or self._client_shows("approval.requested"):
            return
        self._spawn(self._notify("FERB needs you", text, urgent=True, category="question"), "approval banner")

    async def _notify_safely(self, title: str, text: str) -> None:
        banner = None
        if self.perception is not None and self.notify is not _no_notify:
            # FERB knows its own banners (L10): the Notification Center row they become is FERB's, not an arrival.
            shown = {"title": self._redact_text(title), "body": self._redact_text(text)}     # as osascript shows it
            banner = self.perception.banner_posted(self.redact(shown), OSASCRIPT_POSTER)
        try:
            await self.notify(title, text)
        except Exception:
            if banner is not None:                    # never posted: no row of it will come back
                self.perception.banner_failed(banner)

    def owner_idle(self) -> bool:
        """Away from the Mac longer than [sense] idle_after_s (the owner-input tap); unknown counts as present."""
        last = self.owner_input.last_owner_wall
        if not self.owner_input.running or last is None:
            return False
        import time as _t
        return _t.time() - last > float(self.config.get("sense", "idle_after_s", 120))

    async def _notify(self, title: str, text: str, urgent: bool = False, *, category: str = "",
                      target: dict[str, str] | None = None, when_hidden: bool = False, ident: str = "") -> None:
        """Time awareness in code (13): while the owner is idle, non-urgent banners wait until they're back. A category
        the owner switched off in Settings posts nothing (L34 D); chat, logs and the journal still have it."""
        if not banners.enabled(self.config.get, category):
            return
        banner = Banner(title, text, category, dict(target or {}), when_hidden, ident)
        if not urgent and self.owner_idle():
            self._held.append(banner)
            return
        await self._deliver(banner)

    async def _flush_held(self) -> None:
        while True:
            await asyncio.sleep(10)
            if self._held and not self.owner_idle():
                held, self._held = self._held, []
                for banner in held:
                    await self._deliver(banner)

    async def _deliver(self, banner: Banner) -> None:
        """FERB.app posts it while it is the poster (its name and icon; a click opens the notch at the item), else
        osascript does. The title and body are redacted here, from the redacted data callers pass (rule 14)."""
        import uuid
        if self.posters.connected and self.posters.deliver({
                "id": banner.ident or f"banner:{uuid.uuid4().hex[:12]}", "category": banner.category,
                "title": self._redact_text(banner.title), "body": self._redact_text(banner.text),
                "target": dict(banner.target), "when_hidden": banner.when_hidden}):
            return
        await self._notify_safely(banner.title, banner.text)

    async def _notifications_subscribe(self, session: Session, params: dict[str, Any]) -> AsyncIterator[dict[str, Any]]:
        """L34 D: FERB.app, allowed by macOS to post notifications, takes FERB's banners while this stream is open; the
        daemon posts no osascript banner meanwhile. FERB.app leaving ends it, and osascript is back at once."""
        if params.get("authorized") is not True:
            raise FerbError("FERB.app may not post notifications yet; FERB keeps posting them through osascript")
        queue = self.posters.open()
        try:
            while True:
                yield await queue.get()
        finally:
            self.posters.close(queue)

    async def _notifications_categories(self, session: Session, params: dict[str, Any]) -> dict[str, Any]:
        """L34 D: which banner categories post (Settings → Notifications); `set` switches some (config [banners])."""
        if "set" in params:
            switched = banners.set_categories(self.config, params["set"])
            self._journal().append("banners.switched", {"set": params["set"]})
            return {"categories": switched}
        return {"categories": banners.categories(self.config.get)}

    async def _attention_ack(self, session: Session, params: dict[str, Any]) -> dict[str, Any]:
        """`attention.ack {ids}`: a client showed these notices (they leave `chat.status`). They were told when posted.
        `{id}` is the single-notice form older CLIs send."""
        if self.attention_notices is None:
            raise ValueError('Owner notices are unavailable')
        ids = params['ids'] if 'ids' in params else [params['id']]
        if not isinstance(ids, list) or not ids or not all(isinstance(i, str) and i for i in ids):
            raise ValueError('ids must be a nonempty list of notice ids')
        self.attention_notices.acknowledge(ids)
        return {'ok': True}

    def _client_shows(self, topic: str) -> bool:
        """A connected FERB client receives this topic (L9): an authenticated `logs.subscribe` stream with no filter
        (the notch UI, `ferb chat`) or a filter that includes it. Then the client shows it and the macOS banner stays
        off; a subscription filtered to other topics (`ferb logs --timers`) shows nothing."""
        return any(sub.wants(topic) for sub in self._log_subscriptions)

    async def _loop_notice(self, tree_id: str, step_id: str, text: str, notice_origin: str = origin_.FERB) -> None:
        """L31: a loop's back-off or pause notice, posted to the owner's chat like a notice (listed in `chat.status`
        until a client shows it; a banner when no client is connected). Its origin follows its text: a back-off is
        FERB's own; a pause quotes the round's words, so it also carries INFERRED."""
        import uuid
        await self._tell_owner(f'loop-{step_id}-{uuid.uuid4().hex[:12]}', [f'task:{tree_id}'], text, notice_origin,
                               category="loop", task_id=tree_id)

    async def _tell_owner(self, ident: str, refs: list[str], text: str, notice_origin: str, *, category: str = "",
                          task_id: str = "") -> None:
        """One notice line in the owner's chat, posted once per `ident` (`AttentionNotices.add`, `_post_notice`): a
        loop's notice (L31), or a fact distillation retired or replaced on the owner's words (L36 §6, re-review ruling
        (c), `Distiller.tell`). `refs`: what it cites."""
        if self.attention_notices is None:
            return
        notice = self.attention_notices.add(ident, refs, text, notice_origin=notice_origin)
        if notice is not None:
            await self._post_notice(notice, category=category, task_id=task_id)

    async def _post_notice(self, notice: dict[str, Any], *, category: str = "", task_id: str = "") -> None:
        """L9: a notice is a FERB message in the owner's chat: journaled `chat.notice`, published `attention.notice`, and
        listed in `chat.status` until a client acknowledges it. Posting it is telling: a notification it cites gets its
        told receipt now. A macOS banner (its text, never its outside quotes) only when no connected FERB client
        receives `attention.notice`."""
        assert self.attention_notices is not None and self.db is not None
        view = self.attention_notices.view(notice)
        self._journal().append('chat.notice', {k: view[k] for k in ('id', 'text', 'quotes')}, origin=notice['origin'])
        if self.perception is not None:
            for ref in json.loads(notice['source_refs_json']):
                row = self.trail.row(ref.removeprefix('trail:')) if self.trail is not None else None
                if row and row['source'] == 'notification':
                    data = json.loads(row['data_json'])
                    self.perception.policy.told(data.get('bundle_id', ''), data.get('conversation', ''),
                                               ref.removeprefix('trail:'), view['text'])
        self._publish('attention.notice', view)
        if self.posters.connected:
            # L34 D: FERB.app shows notices in the notch; a loop's pause or back-off is also a banner while the notch
            # isn't showing it
            if category == "loop":
                await self._notify("FERB loop", view['text'], category="loop",
                                   target={"kind": "task", "task_id": task_id}, when_hidden=True,
                                   ident=f"loop:{view['id']}")
        elif not self._client_shows('attention.notice'):
            await self._notify('FERB noticed', view['text'], category=category)

    async def _notices_list(self, session: Session, params: dict[str, Any]) -> dict[str, Any]:
        """Noticed: every notice FERB posted, newest first, as clients render them (AttentionNotices.view), and whether
        a client has shown it. Reading acknowledges nothing, so the notch still shows a new one in chat."""
        if self.attention_notices is None:
            return {"notices": []}
        rows = self.attention_notices.list(pending=True) + self.attention_notices.list(pending=False)
        rows.sort(key=lambda r: str(r["created_at"]), reverse=True)
        return {"notices": [self.attention_notices.view(r) | {"shown": r["delivered_at"] is not None} for r in rows]}
