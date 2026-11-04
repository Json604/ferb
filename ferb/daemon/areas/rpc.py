"""Own the socket's method table and the RPC handlers no other area owns (settings, tasks, trace, recovery, verdicts,
reports, stop, doctor, setup, logs). Must not grant a role a method: auth.py does."""

import asyncio
import contextlib
import json
import os
import sys
from pathlib import Path
from typing import Any, AsyncIterator

from ferb.act.fs import recovery
from ferb.act.shell.sandbox import SANDBOX_EXEC
from ferb.bus import DROPPED, Overflow
from ferb.core.errors import Deferred, FerbError
from ferb.daemon.areas.shared import TASK_PAGE, TASK_PAGE_MAX
from ferb.daemon.server import Session
from ferb.orchestrator import episodes
from ferb.store import retention
from ferb.trace import compose


# `ferb logs` filter flags -> bus topic patterns (policy: FERB's questions and answers, the stop, alignment advice)
LOG_FILTERS = {"tasks": ("task.*",), "trail": ("trail.event",), "tools": ("tool.*",), "browsers": ("browser.*",),
               "policy": ("approval.*", "stop.*", "security.*"), "actions": ("action.*",), "timers": ("timer.*",)}


RESTORE_LIST = 100                                # `ferb restore` lists the newest this many records


class Rpc:
    """A mixin of the Daemon (ferb/daemon/main.py): its methods use the daemon's state."""

    async def _settings_get(self, session: Session, params: dict[str, Any]) -> dict[str, Any]:
        """Settings: the values the app may change through settings.set (settings.SETTINGS)."""
        from ferb.daemon import settings
        return {"values": settings.read(self.config)}

    async def _settings_set(self, session: Session, params: dict[str, Any]) -> dict[str, Any]:
        """Settings: one checked value written to config.toml (rule 10: the owner's own connection only, auth.py);
        the daemon reads it from its next use, as it does every config value; the privacy exclusions also reach the
        Redactor (and through it the egress proxy and hearing) at once."""
        from ferb.daemon import settings
        name = str(params.get("key") or "")
        stored = settings.write(self.config, name, params.get("value"))
        if name in settings.PRIVACY:               # the Redactor, the egress proxy and hearing: now, not at restart
            self._apply_privacy()
        self._journal().append("settings.changed", {"key": name, "value": stored}, origin="owner")
        return {"key": name, "value": stored}

    async def _tasks_list(self, session: Session, params: dict[str, Any]) -> dict[str, Any]:
        """Tasks: root tasks newest first, `limit` at a time before `before` (the `created_at` the last page ended at),
        each with its status, what FERB answered (its result, else its outcome) and the owner's latest verdict."""
        if self.db is None:
            raise FerbError("daemon not started")
        limit = params.get("limit", TASK_PAGE)
        if isinstance(limit, bool) or not isinstance(limit, int) or not 1 <= limit <= TASK_PAGE_MAX:
            raise FerbError(f"limit must be a whole number from 1 to {TASK_PAGE_MAX}")
        before = str(params.get("before") or "9999")           # sorts after every ISO time
        assert self.task_results is not None
        rows = self.task_results.page(before, limit + 1)
        out = []
        for r in rows[:limit]:
            outcome = json.loads(r["outcome_json"] or "{}")
            answer = r["answer"] or outcome.get("answer") or outcome.get("summary") or ""
            out.append({"task_id": r["id"], "goal": self._redact_text(str(r["goal"])), "status": r["status"],
                        "created_at": r["created_at"], "updated_at": r["updated_at"],
                        "answer": self._redact_text(str(answer)), "verdict": r["verdict"]})
        return {"tasks": out, "more": len(rows) > limit}

    async def _reports_diagnostic(self, session: Session, params: dict[str, Any]) -> dict[str, Any]:
        """Settings → Advanced → Export a diagnostic file: reports.diagnostic writes it, with the subject Send
        diagnostics to developer (owner 2026-10-27) gives it; the app previews it and `reports.send` posts it, as a
        failure report. Params: `app_version`, the app's version; `send`, recorded only."""
        from ferb.daemon import reports
        if self.db is None:
            raise FerbError("daemon not started")
        send = params.get("send") is True
        text = getattr(self.redact, "text", None)
        if not callable(text):                     # never written unredacted
            raise FerbError("FERB couldn't redact the diagnostic file, so it wrote none")
        health = await self._doctor_check(session, {"selftest": False, "hearing": False})
        recording = await self._sense_status(session, {})
        built = await asyncio.to_thread(
            reports.diagnostic, self.db.path, health, recording, self._report_facts(), home=self.home,
            now=self.clock.now(), walk=self.redact, text=text, app_version=str(params.get("app_version") or ""))
        self._journal().append("diagnostic.built", {"file": built["path"], "send": send})
        return built

    def _update_blockers(self) -> list[dict[str, str]]:
        """What an update must wait for: the supervisor's running trees (update_blockers), each FERB process still
        running whose tree isn't listed already (a long-lived server outlives its task; a restart would kill it), and
        a transcription in flight that no listed task covers (a setup or doctor probe)."""
        running = self.supervisor.update_blockers() if self.supervisor is not None else []
        listed = {r["task_id"] for r in running}
        if self.shells is not None:
            for p in self.shells.list():
                if p.running and p.owner_task.split(".", 1)[0] not in listed:
                    running.append({"task_id": p.owner_task,
                                    "goal": "process: " + self._redact_text(p.command)[:120]})
        if not running and self.hearing.transcriber.busy:     # hearing runs from the app the update replaces
            running.append({"task_id": "", "goal": "turning captured sound into text"})
        return running

    async def _update_prepare(self, session: Session, params: dict[str, Any]) -> dict[str, Any]:
        """FERB.app is about to install an update. Ready only when nothing runs that a restart would cut, unless the
        owner chose Quit and Install (`even_if_busy`). When ready, in one step: nothing new starts, FERB's hearing ends (it
        runs from the app being replaced) and the daemon shuts down at once. The new FERB.app's quiet service update
        starts it on the new code, and suspended plans, loops included, resume (L31)."""
        running = self._update_blockers()
        if running and params.get("even_if_busy") is not True:
            return {"ready": False, "running": running}
        self._shutting_down = True                   # before any await: chat, timers and new trees stop here
        if self.supervisor is not None:
            self.supervisor.hold_new_work()
        self._journal().append("update.prepared", {"version": str(params.get("version") or "")[:40],
                                                    "interrupted": [r["task_id"] for r in running]})
        with contextlib.suppress(Exception):
            self._quit_voice_app()
        self._stop.set()
        return {"ready": True, "running": running}

    def _register_rpc(self) -> None:
        """The socket's methods: who may call each is auth.py's table (roles), not this one."""
        unary: dict[str, Any] = {
            "ping": self._ping, "sense.pause": self._sense_pause, "schedules.list": self._schedules_list,
            "schedules.change": self._schedules_change, "chat.status": self._chat_status,
            "results.ack": self._results_ack, "chat.attach": self._chat_attach, "setup.persona": self._setup_persona,
            "task.stop": self._task_stop, "reminders.list": self._reminders_list,
            "reminders.cancel": self._reminders_cancel,
            "memory.learning": self._memory_learning, "memory.list": self._memory_list,
            "memory.show": self._memory_show, "memory.add": self._memory_add, "memory.profile": self._memory_profile,
            "memory.edit": self._memory_edit, "memory.retract": self._memory_retract,
            "memory.forget": self._memory_forget, "memory.restore": self._memory_restore,
            "memory.repeats": self._memory_repeats, "memory.import.estimate": self._memory_import_estimate,
            "memory.import.start": self._memory_import_start, "memory.import.status": self._memory_import_status,
            "memory.import.cancel": self._memory_import_cancel, "jev.stats": self._jev_stats, "jev.enable": self._jev_enable,
            "jev.reload": self._jev_reload, "runtimes.check": self._runtimes_check, "timeline.get": self._timeline,
            "recovery.list": self._recovery_list, "recovery.restore": self._recovery_restore,
            "sense.resume": self._sense_resume, "sense.status": self._sense_status, "trace.get": self._trace,
            "verdict.set": self._verdict_set, "stop.engage": self._stop_engage, "stop.lift": self._stop_lift,
            "doctor.check": self._doctor_check, "clipboard.request": self._clipboard_request,
            "automation.request": self._automation_request, "tools.list": self._tools_list,
            "tools.call": self._tools_call, "chat.approve": self._chat_approve, "chat.answer": self._chat_answer,
            "test.mode": self._test_mode, "identity.check": self._identity_check,
            "browsers.list": self._browsers_list, "browsers.pair": self._browsers_pair,
            "browsers.installed": self._browsers_installed, "browsers.unpair": self._browsers_unpair,
            "setup.browser_pair": self._setup_browser_pair, "browser.show_handoff": self._show_handoff,
            "setup.browsers_pair_all": self._setup_browsers_pair_all, "identity.pair": self._identity_pair,
            "ps.list": self._ps_list, "ps.kill": self._ps_kill, "conversations.list": self._conversations_list,
            "conversations.change": self._conversations_change, "notifications.categories": self._notifications_categories, "attention.ack": self._attention_ack,
            "reports.build": self._reports_build, "reports.send": self._reports_send, "tasks.list": self._tasks_list,
            "tasks.preview": self._task_preview, "update.prepare": self._update_prepare,
            "reports.diagnostic": self._reports_diagnostic, "notices.list": self._notices_list,
            "settings.get": self._settings_get, "settings.set": self._settings_set,
            "runtimes.status": self._runtimes_status, "runtimes.use": self._runtimes_use,
            "runtimes.switch_task": self._runtimes_switch_task,
            "runtimes.sign_in": self._runtimes_sign_in, "setup.browsers": self._setup_browsers,
            "setup.browser_open": self._setup_browser_open, "setup.permissions": self._setup_permissions,
            "setup.permission_request": self._setup_permission_request, "setup.finish": self._setup_finish,
            "ui.click_through": self._ui_click_through, "ps.send": self._ps_send,
            "about.get": self._about_get, "about.guide": self._about_guide, "secrets.saved": self._secrets_saved,
            "secrets.delete": self._secrets_delete, "identity.find": self._identity_find,
            "identity.choose": self._identity_choose, "seen.list": self._seen_list, "seen.image": self._seen_image,
            "intro.suggest": self._intro_suggest
        }
        streams: dict[str, Any] = {
            "notifications.subscribe": self._notifications_subscribe, "logs.subscribe": self._logs,
            "chat.send": self._chat_send, "ps.attach": self._ps_attach, "intro.preview": self._intro_preview,
            "setup.browsers_pairing": self._setup_browsers_pairing
        }
        self.server.unary.update(unary)
        self.server.streams.update(streams)

    async def _ping(self, session: Session, params: dict[str, Any]) -> dict[str, Any]:
        return {"pid": os.getpid(), "role": session.role.value, "now": self.clock.now().isoformat(),
                "stopped": self.state.stopped}

    def _resolve_root_task(self, task_id: str, allow_last: bool = False) -> str:
        """Owner-typed task id: exact id, or a unique 6+ character root-task prefix; `last` only when allowed."""
        if self.db is None:
            raise FerbError("daemon not started")
        if task_id == "last" and allow_last:  # Explicit CLI request for the latest task, never an action target.
            return self.tasks.latest_root() or task_id
        if len(task_id) >= 6 and not self.tasks.recorded(task_id):
            matches = self.tasks.roots_with_prefix(task_id)
            if len(matches) > 1:
                raise FerbError('ambiguous task prefix: ' + ', '.join(matches))
            if matches:
                return matches[0]
        return task_id

    async def _task_preview(self, session: Session, params: dict[str, Any]) -> dict[str, Any]:
        """`tasks.preview {task_id, worker?}` (FERB.app only, auth.APP_ONLY): the live preview now (kb area app).
        `state`: working (here) / working_elsewhere / asking / waiting / paused / done / none (no window yet); `line`:
        what FERB is doing, for the app to draw over the frame; `image`: the masked frame (base64 JPEG/PNG), fresh or
        the frozen last one (`fresh`); `step`/`worker`: which step's surface it shows (parallel steps each have one);
        `masking`: only text recognition masked it; `problem`: why the last capture gave no frame (withheld, gone,
        not_allowed, slow, failed), the last frame kept; `busy`: FERB was acting there, so no new capture. Never
        waits for the worker; ends when the app leaves. Pixels are in memory only."""
        import base64
        if self.gateway is None:
            return {"state": "unavailable"}
        task_id = str(params.get("task_id") or "")
        worker = params.get("worker")
        try:
            answer = await self.gateway.hooks.previews.frame(task_id, str(worker) if worker else None)
        except asyncio.CancelledError:
            raise
        except Exception:
            return {"state": "unavailable"}
        image = answer.pop("image", None)
        answer = self.redact(answer)
        if image is not None:
            answer["image"] = base64.b64encode(image).decode("ascii")
        return answer

    async def _trace(self, session: Session, params: dict[str, Any]) -> dict[str, Any]:
        task_id = str(params.get("task_id") or "last")
        if self.spans is None or self.db is None:
            raise FerbError("daemon not started")
        task_id = self._resolve_root_task(task_id, allow_last=True)
        spans = self.spans.tree_json(task_id)
        outcome = self.spans.outcome(task_id)
        if not outcome.get('result'):
            recorded = self.tasks.column(task_id, "outcome_json")
            if recorded is not None and recorded != '{}':
                outcome['result'] = json.loads(recorded)
        episode = episodes.get(self.db, task_id)
        # running, or written before F3: compose it live; a learning model call is no task and has no story
        if episode is None and spans and not all(s["kind"] == "model.learning" for s in spans):
            episode = compose.compose({"task_id": task_id, "spans": spans, "outcome": outcome})
        return {"task_id": task_id, "spans": spans, "outcome": outcome, "episode": episode}

    async def _recovery_list(self, session: Session, params: dict[str, Any]) -> dict[str, Any]:
        """`ferb restore`: what FERB can put back, newest first (S1), at most RESTORE_LIST of `total`."""
        kept = self._recovery()
        rows, total = recovery.recent(kept.db, kept.root, RESTORE_LIST)
        return {"days": retention.days(self.config.get("retention", "recovery_days", 7)), "total": total,
                "trash_days": recovery.TRASH_RECORD_MAX_DAYS,
                "records": [r | {"original": recovery.short(r["original"])} for r in rows]}

    async def _recovery_restore(self, session: Session, params: dict[str, Any]) -> dict[str, Any]:
        """`ferb restore <id>`: put a file back (recovery.restore never overwrites a change made since); journaled. It
        holds the gateway's lock on that file (the key FERB's own fs calls take), so it never races a FERB write; the
        file work runs in a worker thread."""
        kept = self._recovery()
        try:
            row = recovery.find(kept.db, str(params.get("id") or ""))
            locks = ["file:" + str(Path(row["original"]).resolve())] if row["kind"] in ("clone", "trash") else []
            async with self._gateway().locks.hold(locks, "owner:ferb restore"):
                back = await recovery.restore_async(row["id"], kept.db, kept.root,
                                                    self._gateway()._path_closed("recovery.restore", owner=True))
        except recovery.NotRestorable as e:
            raise FerbError(str(e)) from None
        original = Path(row["original"])
        in_place = back == original.resolve()
        self._journal().append("recovery.restored", {"id": row["id"], "kind": row["kind"], "original": str(original),
                                                     "restored_to": str(back), "in_place": in_place})
        return {"id": row["id"], "kind": row["kind"], "original": recovery.short(original),
                "restored_to": recovery.short(back), "in_place": in_place}

    async def _verdict_set(self, session: Session, params: dict[str, Any]) -> dict[str, Any]:
        """`ferb verdict`: the owner's pass/fail on a task. Acceptance is their word; code never infers it."""
        if self.db is None:
            raise FerbError("daemon not started")
        verdict = str(params.get("verdict") or "")
        if verdict not in ("pass", "fail"):
            raise FerbError("verdict must be 'pass' or 'fail'")
        given = str(params.get("task_id") or "")
        task_id = self._resolve_root_task(given)
        if not self.tasks.recorded(task_id):
            raise FerbError(f"unknown task {given!r}")
        if self.tasks.column(task_id, "parent_id") is not None:
            tree = self.tasks.column(task_id, "tree_id")
            raise FerbError(f"task {task_id} is part of {tree}; give the verdict for {tree}")
        assert self.task_results is not None
        self.task_results.record_verdict(self.ids.new_id(), task_id, verdict,
                                         self.redact({"note": str(params.get("note") or "")})["note"],
                                         self.clock.now().isoformat())
        self._journal().append("owner.verdict", {"task_id": task_id, "verdict": verdict})
        if self.learning is not None:
            self.learning.inputs_changed()            # a card read before this verdict is read again (L28)
            try:
                await self._capture_card(task_id, None, report=True)
            except Exception as e:                    # the verdict stands; the next capture refreshes the card
                kind = "memory.episode_deferred" if isinstance(e, Deferred) else "memory.episode_failed"
                self._journal().append(kind, {"task_id": task_id, "error": self._redact_text(str(e))})
        return {"task_id": task_id, "verdict": verdict}

    def _report_facts(self) -> dict[str, Any]:
        """The runtimes and extension versions a failure report (L35) and a diagnostic file (L34 Phase E) name."""
        from ferb.runtimes.catalog import ROLES, model_for
        runtimes = []
        for role in ROLES:
            name = self._runtime_for(role)
            model = model_for(self.config, name)
            runtimes.append(f"{role}: {name} {model}".rstrip())
        bridges = self.hub.bridges.items() if self.hub is not None else []
        return {"runtimes": runtimes,
                "extensions": [f"{browser} {folder}: {br.version}" for (browser, folder), br in bridges if br.version]}

    async def _reports_build(self, session: Session, params: dict[str, Any]) -> dict[str, Any]:
        """Send to developer (L35): the report file of a turn FERB recorded as ending badly (`reports.failure`), with
        its subject and summary. Nothing is sent: the app shows the file and the owner's Send calls `reports.send`.
        Params: the turn's `task_id` and `request_id`, whichever the notch holds, and `app_version`, the notch's."""
        from ferb.daemon import reports
        if self.db is None:
            raise FerbError("daemon not started")
        text = getattr(self.redact, "text", None)
        if not callable(text):                # never written unredacted (review 1, M4)
            raise FerbError("FERB couldn't redact the report, so it wrote none")
        task_id = self._resolve_root_task(str(params.get("task_id") or ""))
        facts = self._report_facts()
        found, built = await asyncio.to_thread(
            reports.report, self.db.path, task_id, str(params.get("request_id") or ""), facts, home=self.home,
            now=self.clock.now(), clock=self.clock, ids=self.ids, walk=self.redact, text=text,
            app_version=str(params.get("app_version") or ""))
        self._journal().append("report.built", {"task_id": found.task_id, "file": built["path"]})
        return built

    async def _reports_send(self, session: Session, params: dict[str, Any]) -> dict[str, Any]:
        """The owner's Send under a report or diagnostic file they previewed (owner decisions 2026-11-01): the daemon
        sends that file's text, through the redaction once more, to the developer's report relay, which saves it as
        one file in the private repo and files one issue with its summary and a link to it. A report over 25 MB goes
        shortened; the saved file stays whole. Never retried by itself (only the file again on a commit conflict): on
        any failure the file stays saved, the reply says why (`sent: false`) and the owner can press Send again.
        Params: `path` (as `reports.build`/`reports.diagnostic` returned it), `title` (their subject), `app_version`."""
        from ferb.daemon import reports
        text = getattr(self.redact, "text", None)
        if not callable(text):                # never sent unredacted
            raise FerbError("FERB couldn't redact the report, so it sent nothing")
        path, kind = reports.saved(self.home, str(params.get("path") or ""))
        try:
            url = reports.relay_url(self.config.get("feedback", "relay_url", ""))
            out = await asyncio.to_thread(reports.prepare, path, kind, str(params.get("title") or ""),
                                          str(params.get("app_version") or ""), text=text)
            report_id = await reports.send(url, out)
        except reports.Unsent as e:
            self._journal().append("report.send_failed", {"file": str(path), "kind": kind, "why": str(e)})
            return {"sent": False, "path": str(path), "error": str(e)}
        self._journal().append("report.sent", {"file": str(path), "kind": kind, "report_id": report_id,
                                               "shortened": out.shortened})
        return {"sent": True, "path": str(path), "report_id": report_id, "shortened": out.shortened}

    async def _stop_engage(self, session: Session, params: dict[str, Any]) -> dict[str, Any]:
        summary = self.stop_control.engage()
        self._journal().append("stop.engaged", summary)
        self.bus.publish("stop.engaged", summary)
        return summary

    async def _stop_lift(self, session: Session, params: dict[str, Any]) -> dict[str, Any]:
        self.stop_control.lift()
        self._journal().append("stop.lifted", {})
        self.bus.publish("stop.lifted", {})
        return {"stopped": False}

    async def _identity_check(self, session: Session, params: dict[str, Any]) -> dict[str, Any]:
        """Setup / doctor: FERB's Chrome profile holds exactly FERB's account, and each
        secret is in the Keychain. The session itself is checked through the extension when a session check is requested."""
        from ferb.identity import seed
        address = str(self.config.get("mailbox", "address", "") or "")
        folder = str(self.config.get("identity", "profile", "") or "")
        out: dict[str, Any] = {"address": address, "profile": folder, "profile_ok": False, "error": None,
                               "session": None,
                               "secrets": [{"key": s.ref.key, "in_keychain": await self.secrets.present(s.ref)}
                                           for s in self.secrets.specs()]}
        if not address:
            out["error"] = "FERB has no account of its own set up (Settings → Browsers → FERB's own account)"
        elif not folder:
            out["error"] = "FERB's own account has no browser profile chosen (Settings → Browsers → FERB's own account)"
        elif not self._other_app_data():                # DMN-507: no browser folder is looked at before it
            from ferb.daemon.areas.browsers import WAITS_FOR_FULL_DISK_ACCESS
            out["error"] = WAITS_FOR_FULL_DISK_ACCESS
        else:
            data = self._browser_data().get(str(self.config.get("identity", "browser", "chrome")))
            try:
                p = await asyncio.to_thread(seed.verify, address, folder, *([data] if data else []))
                out["profile_ok"], out["profile_name"] = True, p.name
            except seed.SeedError as e:
                out["error"] = str(e)
        out["paired"] = self._attach_identity_if_paired()
        if out["profile_ok"] and out["paired"] and self.ferb_identity is not None and params.get("session", True):
            try:
                out["session"] = await self.ferb_identity.session_ok()     # through the extension (wakes it)
            except (FerbError, Exception) as e:
                out["session"], out["error"] = False, f"{type(e).__name__}: {e}"
        return out

    async def _doctor_check(self, session: Session, params: dict[str, Any]) -> dict[str, Any]:
        """Permissions and self-test as seen by *this* process — the one that will really act."""
        import ferb
        from ferb.device import permissions, selftest
        from ferb.device.install import code_hash

        def run() -> dict[str, Any]:
            results = selftest.run_all() if params.get("selftest", True) else []
            return {"permissions": permissions.check_all(),
                    "selftest": [{"name": r.name, "ok": r.ok, "detail": r.detail} for r in results]}
        out = await asyncio.to_thread(run)
        out["runtimes"] = await self._runtime_status()
        ok, reason = await asyncio.to_thread(self.sandbox.available) if self.sandbox else (False, "not built")
        # The proxy's liveness check (expect 407): it also restarts a dead serving thread, and says so; a down proxy
        # means the shell has no network and fails closed (F5a Task 3 fix rounds 1-2)
        probe = await asyncio.to_thread(self.proxy.probe) if self.proxy is not None else {}
        out["containment"] = {"sandbox_exec": os.path.exists(SANDBOX_EXEC), "profile_ok": ok, "reason": reason,
                              "shell": "shell.run" in self.registry.capabilities,
                              "shell_reason": self.containment[1],
                              "proxy": self.proxy is not None, "proxy_alive": bool(probe.get("alive")),
                              "proxy_restarted": bool(probe.get("restarted")),
                              "proxy_last_error": probe.get("last_error"),
                              "local_reports": self._local_reports_status(),
                              "snapshots": bool(self.snapshots is not None and self.snapshots.take is not None),
                              # OPEN QUESTION 3: shell commands run without a snapshot when one can't be taken
                              "snapshot_failure": self.snapshots.last_failure() if self.snapshots else None}
        out["hearing"] = await self._hearing_status(bool(params.get("hearing", params.get("selftest", True))))
        out["clipboard"] = await asyncio.to_thread(self._clipboard_access)     # I3: never reads the contents
        from ferb.core.context import token_method
        out["tokenizer"] = token_method()                 # FERB's pinned o200k copy, or the byte ceiling (8a)
        if self.db is not None:                       # FERB.app's Health: the store and its clean-up (STORE-402)
            from ferb.store import cleanup
            from ferb.store.db import store_size
            out["database"] = {"bytes": await asyncio.to_thread(store_size, self.db.path),
                               "cleanup_pending": await asyncio.to_thread(cleanup.pending, self.db.path)}
            # MEM-421: days whose learning pass failed and waits for the next hourly try, with why
            out["learning"] = {"undistilled": [
                {"day": r[0], "error": self._redact_text(str(r[1] or ""))[:300], "attempts": int(r[2])}
                for r in self.db.conn.execute("SELECT day, detail, attempts FROM distill_runs WHERE status = 'failed' "
                                              "ORDER BY day")]}
            # MEM-422: episode cards that failed twice on the same inputs and wait for them to change
            learning = getattr(self, "learning", None)
            out["learning"]["stuck_cards"] = [
                {"task_id": c["task_id"], "error": self._redact_text(c["error"])[:300]}
                for c in (learning.stuck_cards() if learning is not None else [])]
        out.update({"pid": os.getpid(), "executable": os.path.realpath(sys.executable),
                    "version": ferb.__version__, "code_hash": code_hash(Path(ferb.__file__).parent),
                    "home": str(self.home), "stopped": self.state.stopped})
        return out

    @staticmethod
    def _clipboard_access() -> str | None:
        """How macOS treats this daemon reading the clipboard (I3): accessBehavior, read without the contents."""
        try:
            from ferb.sense.watchers.clipboard import access, general_board
            return access(general_board())
        except Exception:
            return None

    async def _clipboard_request(self, session: Session, params: dict[str, Any]) -> dict[str, Any]:
        """`ferb setup` only, with the owner's yes (I3): one read by this daemon, so macOS shows its paste alert now
        and lists FERB in Paste from Other Apps. Nothing read is kept; journaled."""
        from ferb.sense.watchers.clipboard import request_access
        state = await asyncio.to_thread(request_access)
        self._journal().append("clipboard.access", {"access": state, "requested": True})
        return {"access": state}

    def _setup_containment(self) -> dict[str, Any]:
        """The containment facts this daemon started with (CQ5) for the Full Disk Access row: no sandbox-exec run or proxy
        probe per poll; `doctor.check` is the live check."""
        return {"profile_ok": bool(self.containment[0]), "shell": "shell.run" in self.registry.capabilities,
                "proxy": self.proxy is not None}

    async def _setup_permissions(self, session: Session, params: dict[str, Any]) -> dict[str, Any]:
        """L34 C: the setup window's Permissions step (and Settings → Permissions): each grant as this daemon's own
        process holds it, never a prompt. Speech Recognition (dictation and hearing, one grant) is FERB.app's own
        row, which the app checks itself."""
        from ferb.device import permissions
        from ferb.device.install import APP_BUNDLE_ID
        from ferb.paths import app_bundle
        grants = await asyncio.to_thread(self.permission_check)
        profile = self._device_profile()
        running = {bundle for bundle, _, _ in self._running_apps()}
        apps = permissions.automation_candidates(profile.apps if profile is not None else [], running)

        async def automation(bundle: str) -> str:
            try:
                return str((await asyncio.wait_for(self.automation_check(bundle), 2)).value)
            except TimeoutError:
                return permissions.Status.UNKNOWN.value
        statuses = await asyncio.gather(*(automation(bundle) for _, bundle in apps))
        clipboard = permissions.clipboard_row(await asyncio.to_thread(self._clipboard_access))   # owner decision 2
        return {"rows": permissions.rows(grants, self._setup_containment()) + [clipboard],
                "automation": [{"name": name, "bundle_id": bundle, "status": status}
                               for (name, bundle), status in zip(apps, statuses)],
                "automation_pane": permissions.pane_url("automation"),
                "grant_target": permissions.grant_target(app_bundle(), sys.executable),
                "app_panes": permissions.app_panes(APP_BUNDLE_ID),
                "setup_done": int(self.config.get("setup", "permissions_version", 0) or 0) >= 27}

    async def _setup_permission_request(self, session: Session, params: dict[str, Any]) -> dict[str, Any]:
        """L34 C: the owner's Grant on one row: macOS's own prompt, which also lists FERB in that pane; made here so the
        grant attaches to this daemon's process. Journaled."""
        from ferb.device import permissions
        permission = str(params.get("permission", ""))
        if permission not in permissions.REQUESTABLE:
            raise FerbError(f"{permission or 'That permission'} has no request; add FERB in its System Settings pane")
        if permission == "clipboard":           # as `clipboard.request`: one read, journaled; nothing read is kept
            access = await asyncio.to_thread(self.clipboard_request)
            self._journal().append("clipboard.access", {"access": access, "requested": True})
            status = str(permissions.clipboard_row(access)["status"])
        else:
            status = str((await asyncio.to_thread(self.permission_request, permission)).value)
        self._journal().append("setup.permission_requested", {"permission": permission, "status": status})
        return {"permission": permission, "status": status}

    async def _setup_finish(self, session: Session, params: dict[str, Any]) -> dict[str, Any]:
        """L34 C: the setup window's Done (FERB.app's alone, auth.APP_ONLY). Setup is recorded as done
        (`permissions_version`, so launch stops showing the window) only when every permission is granted: this
        daemon's own (its four grants, the clipboard) and the ones FERB.app reports for
        itself (`app`: Notifications, Microphone with Speech Recognition); else `missing` names what isn't, and the window comes back at the
        next launch. Journaled either way."""
        from ferb.device import permissions
        grants = await asyncio.to_thread(self.permission_check)
        rows = permissions.rows(grants, self._setup_containment()) + [
            permissions.clipboard_row(await asyncio.to_thread(self._clipboard_access))]
        app = params.get("app") if isinstance(params.get("app"), dict) else {}
        rows += [{"id": str(name), "status": str(status)} for name, status in app.items()]
        missing = permissions.missing(rows)
        if not missing:
            permissions.record_done(self.config)
        self._journal().append("setup.finished", {"by": "app", "done": not missing,
                                                  "missing": [name for name, _ in missing]})
        return {"setup_done": not missing, "missing": [{"id": name, "status": status} for name, status in missing]}

    async def _automation_request(self, session: Session, params: dict[str, Any]) -> dict[str, Any]:
        """Ask macOS for Apple Events permission to each app (shows the system prompt; must run in the daemon so
        the grant attaches to FERB's interpreter). Apps that aren't running are asked on first use instead."""
        from ferb.device import permissions
        bundle_ids = [str(b) for b in params.get("bundle_ids", [])]
        results = {b: (await permissions.automation_async(b, ask=True)).value for b in bundle_ids}
        self._journal().append("automation.requested", results)
        return {"results": results}

    async def _logs(self, session: Session, params: dict[str, Any]) -> AsyncIterator[dict[str, Any]]:
        filters = [str(f) for f in params.get("filters", [])]
        if unknown := [f for f in filters if f not in LOG_FILTERS]:     # never "everything" for a filter FERB lacks
            raise FerbError(f"unknown log filter {', '.join(unknown)}; use {', '.join(LOG_FILTERS)}")
        wanted = {pattern for f in filters for pattern in LOG_FILTERS[f]} or None
        sub = self.bus.subscribe(wanted, maxsize=int(params.get("maxsize", 1000)))
        self._log_subscriptions.add(sub)
        try:
            async for ev in sub:
                yield {"topic": ev.topic, "at": ev.at.isoformat(),
                       "class": "notice" if ev.topic == DROPPED else ev.cls.value, "data": self.redact(ev.data)}
        except Overflow:
            yield {"topic": "bus.overflow", "at": self.clock.now().isoformat(), "class": "notice",
                   "data": {"message": "fell behind on critical events; restart `ferb logs`"}}
        finally:
            self._log_subscriptions.discard(sub)
            sub.close()
