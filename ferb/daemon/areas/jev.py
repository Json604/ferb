"""Own Jev's shadow wiring (D23, D33: advice only). Must not let Jev's answer authorize or refuse an action."""

import asyncio
from typing import Any

from ferb.core.errors import FerbError
from ferb.daemon.server import Session


class JevGlue:
    """A mixin of the Daemon (ferb/daemon/main.py): its methods use the daemon's state."""

    async def _jev_key(self) -> str | None:
        """From the Keychain (service "ferb", account jev/api_key); never from config, never logged."""
        from ferb.decide.decider import KEY_ACCOUNT
        from ferb.identity.secrets import SERVICE
        proc = await asyncio.create_subprocess_exec("security", "find-generic-password", "-s", SERVICE, "-a",
                                                    KEY_ACCOUNT, "-w", stdout=asyncio.subprocess.PIPE,
                                                    stderr=asyncio.subprocess.DEVNULL)
        out, _ = await proc.communicate()
        key = out.decode().strip() if proc.returncode == 0 else ""
        return key or None

    async def _build_jev(self) -> None:
        """Jev joins only once its key is in the Keychain; without it every hook is skipped (no rows, no calls)."""
        from ferb.decide.decider import JevClient, ShadowDecider
        if self.jev is not None:
            self.jev.close()
            self.jev = None
        key = await self._jev_key() if self.jev_post is None else "test-key"
        if key is None:
            self._wire_jev(None)
            return
        cached = key

        async def get_key() -> str:
            return cached
        assert self.db is not None
        self.jev = ShadowDecider(self.db, self.clock, self.ids, JevClient(get_key, post=self.jev_post), self.redact,
                                 enabled=lambda: set(self.config.get("jev", "enabled", [])))
        self._wire_jev(self.jev)

    def _wire_jev(self, jev: Any) -> None:
        if self.distiller is not None:
            self.distiller.jev = jev
        if self.trail is not None and self.perception is not None:
            self.trail.tag = self.perception.notify
            self.perception.batches.wake.set()

    async def _destructive_shadow(self, facts: dict[str, Any], ref: str) -> None:
        from ferb.decide.decider import Ask
        if self.jev is None:
            return
        baseline = "destructive" if facts.get("kind") in {"pay", "delete_permanent", "account_security", "send", "post"} else "uncertain"
        try:
            await self.jev.ask(self.redact(facts), [Ask("action.destructive", baseline, ref)])
            metadata = {key: facts.get(key) for key in ('tool', 'kind', 'target', 'owner_words')}
            got = await self.jev.advice(self.redact(metadata), [Ask('action.alignment', 'uncertain', ref)])
            if got.get('action.alignment') == 'mismatch':
                self._journal().append('security.alignment', {'task_id': facts.get('task_id'), 'ref': ref,
                                                              'metadata': metadata, 'advice': 'mismatch'})
                self._publish('security.alignment', {'task_id': facts.get('task_id'), 'ref': ref,
                                                     'advice': 'mismatch; orchestrator should inspect the request and action'})
        except Exception:
            pass  # shadow advice never changes the concrete action path

    async def _jev_stats(self, session: Session, params: dict[str, Any]) -> dict[str, Any]:
        if self.jev is None:
            return {"configured": False, "stats": []}
        stats = self.jev.stats()
        for st in stats:
            if st["answered"]:
                st["examples"] = self._jev_examples(st["question"])
        assert self.db is not None
        recent = self.db.conn.execute("SELECT jev, error FROM decisions WHERE error IS NULL OR error NOT IN "
                                      "('not sampled') ORDER BY seq DESC LIMIT 20").fetchall()
        answered = sum(1 for r in recent if r["jev"] is not None)
        last_error = next((r["error"] for r in recent if r["error"]), None)
        from ferb.decide.questions import QUESTIONS
        return {"configured": True, "dropped": self.jev.dropped, "stats": stats, "recent_answered": answered,
                "recent_total": len(recent), "last_error": last_error,
                "retired_enabled": sorted(set(self.config.get("jev", "enabled", [])) - QUESTIONS.keys())}

    def _jev_examples(self, question: str) -> list[str]:
        """A few items Jev flagged, read from their source now (redacted), so nothing extra is kept."""
        assert self.db is not None
        out = []
        for ref in self.jev.flagged_refs(question):
            kind, _, rid = ref.partition(":")
            row = None
            if kind == "trail":
                row = self.db.conn.execute("SELECT source, data_json FROM trail WHERE id = ?", (rid,)).fetchone()
                text = f"{row['source']}: {row['data_json']}" if row else ""
            elif kind == "span":
                row = self.db.conn.execute("SELECT input_json FROM spans WHERE id = ?", (rid,)).fetchone()
                text = row["input_json"] if row else ""
            else:
                text = ref
            if text:
                out.append(str(self._redact_text(text)))
        return out

    async def _jev_enable(self, session: Session, params: dict[str, Any]) -> dict[str, Any]:
        from ferb.decide.questions import QUESTIONS
        q = str(params["question"])
        if q not in QUESTIONS:
            raise FerbError(f"unknown question {q}")
        if QUESTIONS[q].live:
            raise FerbError(f"{q} is live by the plan of record (Q4); it has no switch to turn on or off")
        if QUESTIONS[q].shadow_only and params.get("on", True):
            raise FerbError(f"{q} is measured in shadow; it cannot act before owner-reviewed measurement")
        enabled = set(self.config.get("jev", "enabled", [])) & QUESTIONS.keys()
        if params.get("on", True):
            if self.jev is None:
                raise FerbError("Jev isn't set up: `ferb setup --jev`")
            enabled.add(q)
        else:
            enabled.discard(q)
        self.config.set("jev", "enabled", sorted(enabled))
        self._journal().append("jev.enabled" if params.get("on", True) else "jev.disabled", {"question": q})
        return {"enabled": sorted(enabled)}

    async def _jev_reload(self, session: Session, params: dict[str, Any]) -> dict[str, Any]:
        await self._build_jev()
        out: dict[str, Any] = {"configured": self.jev is not None}
        if self.jev is not None and params.get("ping"):
            from ferb.decide.decider import JevUnavailable
            try:
                await self.jev.client.ask("FERB setup check", {"q0": {"type": "noul",
                                                                      "instructions": "Is this a setup check?"}}, 20)
                out["ping"] = "ok"
            except JevUnavailable as e:
                out["ping"] = str(e)
        return out
