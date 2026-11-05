"""Own the chat RPCs and a chat turn's plumbing (acknowledgement, answers to questions, results told). Must not
interpret the owner's words: the supervisor does."""

import asyncio
import json
from datetime import datetime
from typing import Any, AsyncIterator

from ferb.act.fs.files import normalize_path
from ferb.core import origin as origin_
from ferb.core.errors import FerbError, Refused
from ferb.daemon.questions import card_flags
from ferb.daemon.server import Session
from ferb.orchestrator import acks
from ferb.orchestrator.continuity import RESERVED_SESSIONS
from ferb.orchestrator.worker import couldnt
from ferb.sanitize.redact import Redactor
from ferb.sanitize.untrusted import mark


class Chat:
    """A mixin of the Daemon (ferb/daemon/main.py): its methods use the daemon's state."""

    def _hide_code_reply(self, text: str, session_id: str | None = None) -> str:
        """Rule 14 for a chat message (typed or spoken) while a question is open, before any model sees it, as
        chat.answer hides an answer (`Redactor.answer`, re-review 2 R1): its codes are masked when the question asks
        for a code, and a one-word reply to a password or PIN question is hidden whole. The planner routes answers
        now (L3), so the secret must not travel with the message; a longer message may be a steer and keeps its
        words. The questions: workers' open ones, and the planner's own clarifying question in this conversation
        (re-review 3, R1-b), also when FERB's own "which is it?" has since replaced it as the pending question
        (`Supervisor.displaced_question`). FERB's memory questions never ask for a secret, so neither they nor FERB's own
        "which is it?" about them hide anything, and a reply that is exactly an option of a waiting question is a
        choice (L36)."""
        if not isinstance(self.redact, Redactor):
            return text
        from ferb.orchestrator.steering import MEMORY_ASKS
        open_now = self.state.approvals.pending()
        choice = text.strip().rstrip(".!").strip()      # as an option pick reads it (supervisor `_bare`): "Keep." is Keep
        for ap in open_now:
            if ap.capability in MEMORY_ASKS:            # FERB's memory questions never ask for a secret (L36)
                continue
            # a login-code prompt asks for a code by its kind, whatever its words say
            reason = "login code" if ap.kind == "owner_login_code" else ap.reason
            if ap.kind in ("owner_question", "owner_login_code") and ap.option_for(choice) is None:
                text = self.redact.answer(text, question=reason, reply=True)
        if session_id is not None and self.supervisor is not None:
            asked = self.supervisor.continuity.pending(session_id)[1]
            # never a choice (exactly an option of a waiting question), and never after FERB's own "which is it?"
            # about memory questions only, whose words quote a guess (L36: "passwords" there asks for nothing); the
            # planner's question FERB's asking displaced still counts (rule 14)
            if not any(ap.option_for(choice) is not None for ap in open_now):
                if asked and not self.supervisor.asked_only_about(session_id, MEMORY_ASKS):
                    text = self.redact.answer(text, question=asked, reply=True)
                displaced = self.supervisor.displaced_question(session_id)
                if displaced:
                    text = self.redact.answer(text, question=displaced, reply=True)
        return text

    async def _chat_status(self, session: Session, params: dict[str, Any]) -> dict[str, Any]:
        """Read-only: answers no chat has shown yet (a client marks the ones it placed with `results.ack`), tasks
        still running, and questions waiting for the owner. `peek` is still accepted; reading never marks anything
        (it used to mark every unshown result, so one client could consume another's)."""
        assert self.db is not None and self.task_results is not None
        rows = self.task_results.unshown()
        persona = self.persona.status() if self.persona is not None else {'state': 'not_started'}
        approvals = self.state.approvals.pending()
        request_trees: dict[str, set[str]] = {}
        for request_id in {ap.request_id for ap in approvals}:
            request_trees[request_id] = self.tasks.trees_of_request(request_id)
        # Each with what `approval.requested` said (step 8e): the task id that asked (`source_task_id`: a step's, or a
        # task under it, ties a confirmation to its row after an app restart), the part FERB's question is about, and
        # the runtime switch it offers.
        pending = [{"approval_id": ap.id, "capability": ap.capability, "kind": ap.kind, "reason": ap.reason,
                    "task_id": next(iter(request_trees[ap.request_id]))
                        if len(request_trees.get(ap.request_id, ())) == 1 else None,
                    "options": list(ap.options),
                    **({"summary": ap.summary} if ap.summary else {}),
                    **({"from_task": True} if ap.from_task else {}),
                    **({"why": ap.why} if getattr(ap, "why", "") else {}),
                    **({"source_task_id": ap.task_id} if ap.task_id else {}),
                    **({"part": ap.part} if ap.part else {}),
                    **({"runtime_switch": ap.runtime_switch} if ap.runtime_switch else {}),
                    **card_flags(ap)} for ap in approvals]
        running = [{"task_id": t, "goal": g}                  # a watch check isn't listed: no client adopts it (I3)
                   for t, g in (self.supervisor.running_trees(watches=False) if self.supervisor else [])]
        for r in running:                                     # the notch's elapsed timer (2026-10-27)
            started = self.tasks.column(r["task_id"], "created_at")
            if started is not None:
                r["started_at"] = started
            card = self.supervisor.card_state(r["task_id"]) if self.supervisor else None
            if card is not None:          # ORCH-319: a multi-part task's rows, rebuilt from the record
                r["card"] = self.redact(card)
        results = []
        for row in rows:
            recorded = json.loads(row['outcome_json'] or '{}')
            # request_id and at: a client whose chat.send stream ended early still places the answer (L6, L7)
            results.append({key: row[key] for key in ('task_id', 'request_id', 'at', 'status', 'goal', 'answer')} |
                           {'artifacts': recorded.get('artifacts', []), 'verified': recorded.get('verified'),
                            'verification': recorded.get('verification', [])} |
                           # the answer's own fields (a multi-part task's card and footer; its lines; ORCH-319)
                           {key: recorded[key] for key in ('card', 'note', 'correction', 'lines', 'runtime_switch')
                            if recorded.get(key)} |
                           # its typed cards as told (step 8e)
                           ({'cards': json.loads(row['cards_json'])} if row['cards_json'] else {}))
        return {"results": results, "running": running, "approvals": self.redact(pending),
                "persona": {'state': persona['state'], **({'error': persona['error']} if persona.get('error') else {})},
                "notices": self.attention_notices.unseen() if self.attention_notices else [],
                "delegated_conversations": self.supervisor.conversations.list() if self.supervisor else [],
                "loops": self.supervisor.loop_statuses() if self.supervisor else [],
                "paused": bool(self.sense is not None and self.sense.paused),        # L34 D: the menu bar's state
                "stopped": bool(self.state.stopped)}

    async def _results_ack(self, session: Session, params: dict[str, Any]) -> dict[str, Any]:
        """A client that reads `chat.status` with `peek` showed these results in its chat: mark only them shown, so
        `ferb chat` doesn't replay them as arrived while it was closed."""
        assert self.task_results is not None
        task_ids = params.get('task_ids')
        if not isinstance(task_ids, list) or not all(isinstance(t, str) and t for t in task_ids):
            raise ValueError('task_ids must be a list of task ids')
        self.task_results.mark_shown(task_ids)
        return {"ok": True}

    def _resume_conversation_plans(self) -> None:
        if self.supervisor is not None:
            job = asyncio.create_task(self.supervisor.resume_plans())
            self._runs.add(job)
            job.add_done_callback(self._runs.discard)

    async def _conversations_list(self, session: Session, params: dict[str, Any]) -> dict[str, Any]:
        assert self.supervisor is not None
        ident = params.get('id')
        return self.supervisor.conversations.digest(ident) if ident else {'conversations': self.supervisor.conversations.list()}

    async def _conversations_change(self, session: Session, params: dict[str, Any]) -> dict[str, Any]:
        assert self.supervisor is not None
        self.supervisor.change_conversation(str(params['id']), str(params['action']), str(params['owner_words']))
        self._journal().append('conversation.owner_change', self.redact(params), origin='owner')
        return {'conversation': self.supervisor.conversations.get(str(params['id']))}

    async def _ui_click_through(self, session: Session, params: dict[str, Any]) -> dict[str, Any]:
        """`ui.click_through {windows}` (D31 fix round 4, H1 and N1): the notch's click-through windows by number (FERB's
        pointer overlay on each display, its glow), sent whenever they change and on each connection. Recorded only
        from one of FERB's running apps, the caller known by its socket's pid; the Mac tools' point check then skips
        exactly those windows of that app while it runs (`MacComputerDriver.report_click_through`)."""
        windows = params.get("windows")
        if not isinstance(windows, list) or not all(type(w) is int and w > 0 for w in windows):
            raise Refused("windows must be a list of window numbers")
        if self.mac is None or session.pid is None or not await asyncio.to_thread(
                self.mac.report_click_through, session.pid, windows):
            raise Refused("only FERB's own app reports its click-through windows")
        return {"windows": len(windows)}

    async def _chat_attach(self, session: Session, params: dict[str, Any]) -> dict[str, Any]:
        """Chat is open again: approvals for running tasks are asked there, not by notification."""
        for t, _ in (self.supervisor.running_trees() if self.supervisor else []):
            self._attached_trees.add(t)
        assert self.supervisor is not None
        conversation = self.supervisor.continuity.attach(bool(params.get("new")), params.get("resume"))
        self._prepare_persona()
        return {"ok": True, "session_id": conversation, "capabilities": ["input_paths"]}

    async def _task_stop(self, session: Session, params: dict[str, Any]) -> dict[str, Any]:
        """The owner's stop of exactly these tasks (the app sends their ids), recorded as stopped by the owner. Ids are
        required: stopping everything is the global stop (`stop.engage`), never an empty list (kb area daemon
        DMN-1)."""
        assert self.supervisor is not None
        ids = [str(t) for t in params.get("task_ids") or []]
        if not ids:
            raise FerbError("task.stop needs the ids of the tasks to stop; stopping everything is stop.engage")
        stopped = [t for t in ids if self.supervisor.stop_tree(t)]
        for t in stopped:
            self._journal().append("task.stopped", {"task_id": t, "by": "owner"})
        return {"stopped": stopped}

    async def _tools_list(self, session: Session, params: dict[str, Any]) -> dict[str, Any]:
        if session.grant is None or self.gateway is None:
            raise FerbError("no run token")
        return {"tools": self.gateway.tools_for(session.grant)}

    async def _tools_call(self, session: Session, params: dict[str, Any]) -> dict[str, Any]:
        if session.grant is None or self.gateway is None:
            raise FerbError("no run token")
        return await self.gateway.call(session.grant, str(params.get("name")), dict(params.get("arguments") or {}))

    async def _chat_approve(self, session: Session, params: dict[str, Any]) -> dict[str, Any]:
        choice = params.get("choice")
        return self._resolve_approval(str(params["approval_id"]), bool(params["granted"]),
                                      int(choice) if choice is not None else None)

    async def _chat_answer(self, session: Session, params: dict[str, Any]) -> dict[str, Any]:
        """The owner's free-text answer to a worker's question (owner.ask); empty text declines."""
        text = str(params.get("text") or "")
        return self._resolve_approval(str(params["approval_id"]), bool(text), None, text or None)

    async def _chat_correct(self, session: Session, params: dict[str, Any]) -> dict[str, Any]:
        """`chat.correct {approval_id, text}` (GATE-510): the box on a safety check's card that belongs to a task ("Or
        tell the task something else"). The owner's exact option words ("yes", "no", typed by them) are that option, as
        `chat.answer` takes them; any other words never count as a Yes or a No: they are the owner's correction of
        that check's task (`Supervisor.correct_from_question`), redacted as a chat message is (`chat.send`'s path:
        the redactor, a code reply hidden while a question asks for one, the GATE-501/502 notice), never hidden whole
        because the check's own words mention a password."""
        if self.supervisor is None:
            raise FerbError("daemon not started")
        approval_id = str(params.get("approval_id") or "")
        raw = str(params.get("text") or "").strip()
        ap = self.state.approvals.get(approval_id)
        if ap is None or ap.state != "pending":
            raise FerbError("That question isn't waiting any more, so nothing was sent to the task.")
        if not ap.corrects or not ap.task_id:
            raise FerbError("This question isn't part of a task, so it takes only Yes or No.")
        if not raw:
            raise FerbError("Type what the task should do instead, or pick one of the answers.")
        if ap.option_for(raw) is not None:            # exactly an option, in the owner's own keystrokes
            return self._resolve_approval(approval_id, True, None, raw)
        hidden = self._secret_kinds(raw)               # the kinds masked below, by name: the notice and the fact line
        session_id = self.supervisor.continuity.conversation_of(self.tasks.tree_of(ap.task_id) or ap.task_id)
        text = self._hide_code_reply(self.redact(raw), session_id)
        told = await self.supervisor.correct_from_question(approval_id, text, hidden)
        if hidden:
            self._journal().append("chat.secret_hidden", {"session_id": session_id, "request_id": told["request_id"],
                                                          "kinds": hidden})
        self._journal().append("approval.corrected", {"approval_id": approval_id, "task_id": told["task_id"],
                                                      "state": told["state"]})
        return {"approval_id": approval_id, "state": told["state"], "task_id": told["task_id"],
                "message": mark(text).for_model, "reply": told["text"], **({"hidden": hidden} if hidden else {})}

    async def _test_mode(self, session: Session, params: dict[str, Any]) -> dict[str, Any]:
        if self.gateway is None:
            raise FerbError("the tool gateway is not ready")
        if params.get("status") is True:
            return {"enabled": self.gateway.test_send_allowlist is not None,
                    "allow_send": sorted(self.gateway.test_send_allowlist or ())}
        allowed = params.get("allow_send", [])
        if not isinstance(allowed, list) or not all(isinstance(x, str) and x.strip() for x in allowed):
            raise FerbError('allow_send must be a list of exact recipients or "SITE @ACCOUNT" entries for posts')
        from ferb.daemon.gateway import allowlist_problem
        problems = [p for p in (allowlist_problem(x) for x in allowed) if p]
        if problems:
            raise FerbError("; ".join(problems))
        next_allowlist = None if params.get("off") is True else set(allowed)
        record = {"enabled": next_allowlist is not None, "allow_send": sorted(next_allowlist or ())}
        self.config.set('test_mode', 'state', record)  # one atomic config write before the running state changes
        self.gateway.test_send_allowlist = next_allowlist
        self._journal().append("test.mode", record)
        return record

    def _resolve_approval(self, approval_id: str, granted: bool, choice: int | None = None,
                          text: str | None = None) -> dict[str, Any]:
        """The owner's answer to a question, typed (`ferb chat`) or spoken (push-to-talk)."""
        ap = self.state.approvals.get(approval_id)
        retained = None
        hidden: list[str] = []
        if text is not None and granted:
            if ap is not None and ap.kind in {"owner_admin_password", "owner_secret"}:
                if any(c in text for c in "\r\n\x00"):
                    raise FerbError("the hidden password must be one line")
                if not isinstance(self.redact, Redactor):
                    raise FerbError("hidden input redaction is unavailable")
                self.redact.retain_secret(text)
                retained = text
            else:
                text = text.strip()
                granted = bool(text)
                from ferb.orchestrator.steering import MEMORY_ASKS, OPTION_ONLY, choices
                # FERB's checks (GATE-510: "Pick Yes or No.", never dropped as "neither Yes nor No") and FERB's own
                # check-ins (ORCH-536) take only one of their options; typed words never answer them
                if (ap is not None and (ap.capability in OPTION_ONLY or ap.options_only) and text
                        and ap.option_for(text) is None):
                    raise FerbError(f"Pick {choices(ap.options)}.")
                # Only the sealed secret inputs (a secret, the admin password, a login code) make hidden secrets
                # (L18, D28). A picked option is never one; other typed words to a question are hidden for this
                # answer alone when the question asks for a one-time code or a sign-in password or PIN
                # (`Redactor.answer`), and never remembered.
                if (ap is not None and ap.kind == "owner_question" and isinstance(self.redact, Redactor)
                        and ap.option_for(text) is None):
                    # FERB's memory questions never ask for a secret: only secret classes are hidden (L36)
                    hidden = self._secret_kinds(text)       # as a chat message's: the kinds by name, never the value
                    text = (self.redact.text(text) if ap.capability in MEMORY_ASKS
                            else self.redact.answer(text, question=ap.reason))
                if ap is not None and ap.kind == "owner_login_code" and text:
                    self._add_secret_value(text)
        try:
            ap = self.state.approvals.resolve(approval_id, granted, choice, text)
            ap.hidden = tuple(hidden)                       # the worker gets FERB's note beside the answer (gateway)
            followed = self._question_answered(ap)          # a chat forget answered with the notch's buttons (I2)
            if followed and ap.capability == "memory.forget":
                self._publish("memory.forgot", {"approval_id": ap.id, "text": followed})
        except BaseException as e:
            if retained is not None and isinstance(self.redact, Redactor):
                if isinstance(e, Exception) and not isinstance(e, FerbError):
                    # recorded while the hidden answer is still redacted by its value (rule 14, review Q-M3)
                    self._record_internal("chat.answer", e)
                self.redact.release_secret(retained)
            raise
        self._journal().append("approval.resolved", {"approval_id": ap.id, "state": ap.state,
                                                     "capability": ap.capability})
        if hidden:
            self._journal().append("chat.secret_hidden", {"approval_id": ap.id, "request_id": ap.request_id,
                                                          "kinds": hidden})
        self._publish("approval.resolved", {"approval_id": ap.id, "state": ap.state})
        return {"approval_id": ap.id, "state": ap.state, **({"hidden": hidden} if hidden else {})}

    def _chat_ack(self, session_id: str, request_id: str, message: str = "",
                  received: datetime | None = None, hidden: list[str] | None = None) -> dict[str, Any]:
        """TQ3/P25: a neutral phrase said on receipt, never one of the last eight said; journaled into chat history,
        with when the message arrived (`ferb baseline` times the ack, plan line and answer from it, L8). `message`:
        the owner's message as the daemon redacted it, as `chat.message` records it, for clients to keep (L7).
        `hidden`: the kinds of secret masked in it (`_secret_kinds`), for the notice under the owner's message."""
        said = [str(text) for text in self._journal().recent_values("chat.ack", "text", acks.RECENT)]
        text = acks.pick(said[::-1])
        entry = self._journal().append("chat.ack", {"session_id": session_id, "request_id": request_id, "text": text,
                                                    "received_at": (received or self.clock.now()).isoformat()})
        if hidden:
            self._journal().append("chat.secret_hidden", {"session_id": session_id, "request_id": request_id,
                                                          "kinds": hidden})
        return {"type": "ack", "request_id": request_id, "text": text, "message": message.strip(),
                "at": entry.at.isoformat(), **({"hidden": hidden} if hidden else {})}

    def _secret_kinds(self, raw: str) -> list[str]:
        """The kinds of specific secret the owner's raw words hold, each once (`Redactor.kinds`: a saved value or a
        named format), read before they are masked: names only, the raw text itself is never kept (rule 7). A code
        hidden as a reply to a question (`_hide_code_reply`) and labelled text are masked too, but name no kind."""
        return self.redact.kinds(raw) if isinstance(self.redact, Redactor) else []

    def _ask_input_monitoring(self) -> None:
        """Input Monitoring is asked with the owner's first request, not in setup (owner 2026-10-26: "ask when first
        needed, like Claude and ChatGPT"): macOS's own prompt, once, off the loop, journaled."""
        from ferb.device import permissions as P
        if self._input_monitoring_asked or P.input_monitoring() is not P.Status.NOT_DETERMINED:
            return
        self._input_monitoring_asked = True
        self._journal().append("permission.asked", {"permission": "input_monitoring", "when": "first request"})
        self._spawn(asyncio.get_running_loop().run_in_executor(None, self.permission_request, "input_monitoring"),
                    "input monitoring request")

    async def _chat_send(self, session: Session, params: dict[str, Any]) -> AsyncIterator[dict[str, Any]]:
        """One chat turn: an acknowledgement on receipt, then the supervisor's events, merged with bus events about
        the tasks this turn started (tool calls, owner questions and action observations) so the app can show them
        inline. Every event names this message's request_id and when it was sent (L7). FERB never speaks
        (VOICE-409): every reply is text, whether the message was typed or dictated; a `speak` flag an older FERB.app
        sends is ignored."""
        received = self.clock.now()
        if self._shutting_down:
            raise FerbError("daemon is shutting down; no new chat task was started")
        if self.supervisor is None:
            raise FerbError("daemon not started")
        self._ask_input_monitoring()
        paths = params.get('input_paths', [])
        if not isinstance(paths, list) or not all(isinstance(p, str) and p.strip() and '\x00' not in p for p in paths):
            raise FerbError('input_paths must be a list of nonempty filesystem paths')
        input_paths = list(dict.fromkeys(normalize_path(p, str(self.home / 'workspace')) for p in paths))
        hit = getattr(self.redact, "hit", None)            # a specific secret only: a saved value or a credential format
        if any(hit(p) for p in input_paths) if hit is not None else self.redact(input_paths) != input_paths:
            raise FerbError('input_paths contains protected secret text; rename the attachment before sharing its path')
        session_id = str(params.get("session_id", "default"))
        if session_id.startswith(RESERVED_SESSIONS):     # FERB's own planner sessions (a schedule's, the intro preview)
            raise FerbError("session_id names one of FERB's own sessions, not a chat")
        raw = str(params.get("text", ""))
        hidden = self._secret_kinds(raw)                # the kinds masked below, by name: the notice and the fact line
        text = self._hide_code_reply(self.redact(raw), session_id)
        request_id = self.supervisor.ids.new_id()
        queue: asyncio.Queue[dict[str, Any] | None] = asyncio.Queue()
        # Before the per-session routing lock: a message is acknowledged while an earlier one is still planning (P29).
        # The ack carries the message as chat.message records it (`mark`): the composer's paste markers and leaked
        # ones removed, so the owner's bubble never shows them (APP-712)
        queue.put_nowait(self._chat_ack(session_id, request_id, mark(text).for_model, received, hidden))
        # L12: what the owner has in front of them as they send, captured beside routing; the planner waits for it
        screen = (asyncio.ensure_future(self.front_window(request_id, session.pid if session is not None else None))
                  if self.front_window is not None else None)
        my_tasks: set[str] = set()
        attached = {"on": True}                     # False once the owner closes chat: the task keeps running
        sub = self.bus.subscribe({"tool.called", "approval.requested", "action.*"})

        def stamped(ev: dict[str, Any]) -> dict[str, Any]:
            return {**ev, "request_id": request_id, "at": self.clock.now().isoformat()}

        async def turn() -> None:
            try:
                async for ev in self.supervisor.send(session_id, text,  # type: ignore[union-attr]
                                                     input_paths=input_paths, request_id=request_id, screen=screen,
                                                     hidden=hidden):
                    ev = stamped(ev)
                    if ev.get("task_id"):
                        my_tasks.add(ev["task_id"])
                        if attached["on"]:
                            self._attached_trees.add(ev["task_id"])
                    if ev.get("type") == "outcome":
                        await self._task_result(ev, shown=attached["on"])
                        self._attached_trees.discard(ev["task_id"])
                    await queue.put(ev)
            except FerbError as e:
                # the turn ended failed, recorded as every turn failure is, so Send to developer works on it (L35)
                self._journal().append("chat.failed", {"request_id": request_id, "why": self._redact_text(str(e))})
                await queue.put(stamped({"type": "error", "text": str(e)}))
            except Exception as e:      # logged whole; the owner reads its reason (owner 2026-10-24), never "Sorry"
                self._journal().append("chat.failed", {"request_id": request_id,
                                                       "why": f"{type(e).__name__}: {self.redact(str(e))}"})
                # no task of this turn had started (none named in its events: a task's plan line comes before its
                # work), or one may have been under way
                said = self.redact(couldnt("finish that" if my_tasks else "start that", e, self.redact))
                # in the conversation record too, as `_failure_reply` keeps its own (L21); the turn ended failed: the
                # notch offers Send to developer on it (L35)
                self._journal().append("chat.reply", {"session_id": session_id, "request_id": request_id,
                                                      "text": said}, origin=origin_.FERB)
                await queue.put(stamped({"type": "reply", "text": said, "status": "failed"}))
            finally:
                if screen is not None and not screen.done():   # the turn ended before routing used it
                    screen.cancel()
                await queue.put(None)

        async def bus_events() -> None:
            async for ev in sub:
                tid = ev.data.get("task_id")
                ours = tid in my_tasks or self.tasks.tree_of(str(tid)) in my_tasks
                if ours or (ev.topic.startswith("action.") and not tid):
                    wrapper = stamped({"type": "event", "topic": ev.topic, "data": self.redact(ev.data)})
                    # an observation outside any task goes to every open chat: it is about no message
                    await queue.put(wrapper if ours else {**wrapper, "request_id": None})

        # The turn takes the routing lock in its first step, ahead of any message that arrives after this ack is sent
        t1, t2 = asyncio.create_task(turn()), asyncio.create_task(bus_events())
        self._turns.add(t1)
        t1.add_done_callback(self._turns.discard)
        try:
            while (item := await queue.get()) is not None:
                yield item
        finally:
            t2.cancel()
            sub.close()
            if not t1.done():                           # chat closed mid-task: never stops it (Ctrl-C does)
                attached["on"] = False
                for tid in my_tasks:
                    self._attached_trees.discard(tid)
