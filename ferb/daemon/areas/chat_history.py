"""Own the chat history RPCs (APP-805): FERB.app writes each chat entry as it changes and reads a day a page at a time;
its old saved chat comes over once. FERB.app only (auth.APP_ONLY). Must not interpret an entry (ferb/store/
chat_history.py keeps it as the app sent it, redacted)."""

from typing import Any

from ferb.core.errors import FerbError
from ferb.daemon.server import Session
from ferb.store.chat_history import PAGE, ChatHistory


class ChatHistoryRpc:
    """A mixin of the Daemon (ferb/daemon/main.py): its methods use the daemon's state."""

    def _chat_history(self) -> ChatHistory:
        if self.db is None:
            raise FerbError("daemon not started")
        return ChatHistory(self.db, self.redact, lambda: self.clock.now().isoformat())

    async def _chat_history_put(self, session: Session, params: dict[str, Any]) -> dict[str, Any]:
        """`chat.history.put {entries}`: entries that changed in the app (each `{id, kind, at, day, open, ref?,
        body}`) replace the ones kept."""
        return self._chat_history().write(params.get("entries"), replace=True)

    async def _chat_history_import(self, session: Session, params: dict[str, Any]) -> dict[str, Any]:
        """`chat.history.import {entries}`: the chat an older FERB.app kept in its preferences, sent once; an entry
        already kept stays as it is. `held` counts the given ids now kept, so the app knows the import is complete."""
        return self._chat_history().write(params.get("entries"), replace=False)

    async def _chat_history_page(self, session: Session, params: dict[str, Any]) -> dict[str, Any]:
        """`chat.history.page {day, before?, limit?, open?}`: a day's newest entries before `before`, oldest first, with
        `more`, `cursor` (the next `before`) and `context`; with `open`, every turn still waiting for FERB's answer."""
        store = self._chat_history()
        out = store.page(str(params.get("day") or ""), params.get("before") or None, params.get("limit", PAGE))
        if params.get("open") is True:
            out["open"] = store.open()
        return out

    async def _chat_history_days(self, session: Session, params: dict[str, Any]) -> dict[str, Any]:
        """`chat.history.days {}`: each day with an entry, newest first (History)."""
        return {"days": self._chat_history().days()}
