"""Compose the daemon: config at start, the stores, the gateway with its deps, the supervisor with its ports, and the
published events' consumers. Must not interpret requests or select targets."""

from pathlib import Path
from typing import Any

from ferb.act.browser import downloads
from ferb.act.fs import recovery
from ferb.act.fs.changes import Changes
from ferb.core import origin as origin_
from ferb.core.errors import FerbError
from ferb.daemon import signed_out
from ferb.daemon.gateway import FsRoots, Gateway
from ferb.memory import search as memory_search
from ferb.orchestrator.supervisor import Supervisor
from ferb.orchestrator.verify import Checks
from ferb.policy import protection as protection_lists
from ferb.policy.destinations import Confirmations, KnownDestinations
from ferb.policy.standing import StandingPermissions
from ferb.registry import add_context_readers
from ferb.sanitize.redact import Redactor


def question_kind(capability: str, kind: str) -> str:
    """A question's kind from who asks it and how (structural; never its words)."""
    from ferb.daemon.gateway import CONFIRM_ASK, RETRY_ASK
    from ferb.orchestrator.steering import CONVERSATION_ASK, DOWNLOADS_ASK, MEMORY_ASKS
    if kind in ("owner_login_code", "owner_admin_password", "owner_secret"):
        return "secret"
    return {CONFIRM_ASK: "confirm", RETRY_ASK: "retry", CONVERSATION_ASK: "conversation_scope", "plan.ask": "task",
            DOWNLOADS_ASK: "downloads", "browsing.choose": "browsing",
            "owner.ask": "task", "loop.ask": "loop", "runtime.switch": "runtime_switch"}.get(
        capability, "memory" if capability in MEMORY_ASKS else capability)


class Compose:
    """A mixin of the Daemon (ferb/daemon/main.py): its methods use the daemon's state."""

    def _load_config(self) -> None:
        if self.protection is None:     # [protection] and the workspace, once at start: before migrations and browsers
            self.protection = protection_lists.load(self.config.get, Path.home(), self.home)
        self.device_id = str(self.config.get("device", "id", "unknown-device"))
        self._apply_privacy()

    def _apply_privacy(self) -> None:
        """[privacy] exclude_domains / exclude_apps into the one Redactor everything holds: the journal, spans,
        gateway, the shell's egress proxy (its redactor and excluded-host check) and hearing (its excluded-host check)
        all read this instance, so a change applies to them at once."""
        if self._redact_override is None:
            domains, apps = set(self.config.get("privacy", "exclude_domains", [])), set(self.config.exclude_apps())
            if isinstance(self.redact, Redactor):      # the journal, spans and gateway hold this instance: keep it
                self.redact.configure(domains, apps)
            else:
                self.redact = Redactor(domains, apps)

    def _build_stores(self) -> None:
        """The stores the agents are handed, built from the database before them (they need nothing else that is
        built later): memory and learning, timers and schedules, owner notices, FERB's questions' answers and the
        destinations it knows."""
        from ferb.clock.timers import Timers
        from ferb.memory.items import Memory
        from ferb.memory.learning import Learning
        from ferb.orchestrator.attention import AttentionNotices
        from ferb.orchestrator.schedules import Schedules
        assert self.db is not None
        self.memory = Memory(self.db, self.clock, self.ids, self._publish, on_change=self._export_memory,
                             redact=self._redact_text)
        self.learning = Learning(self.db, self.clock, self._publish, self._redact_text, redact_id=self._redact_id,
                                 redact_value=lambda value: self.redact(value))
        self.timers = Timers(self.db, self.clock, self.ids, self._timer_fired, on_error=self._timer_failed)
        self.timers.on_change = self._timer_changed               # DMN-548: every client shows the same timer state
        self.timers.cancel_all_sleeps()
        self.schedules = Schedules(self.db, self.clock, self.ids)
        self.attention_notices = AttentionNotices(self.db, self.clock, self.redact)
        self.confirmations = Confirmations(self.db, self.clock, self.ids, self.redact)     # D33, S4 (F5a)
        self.destinations = KnownDestinations(self.db, self.clock, self.ids, self.redact)  # SQ6

    def _build_agents(self) -> None:
        """The gateway, handed everything it uses in one GatewayDeps, then the supervisor, handed its ports
        (ferb/orchestrator/ports.py). A worker's question reaches the supervisor through a router both are given, so
        neither is set on the other afterwards."""
        from ferb.daemon.gateway import GatewayDeps
        from ferb.orchestrator import ports
        device = self.device_id
        workspace = self.home / "workspace"
        workspace.mkdir(parents=True, exist_ok=True)
        # The protection lists (S2, SQ3, SQ5, D27), read in _load_config: FERB's own state is always added by code
        # (rule 10); the shell and FERB's tools that name files share this one object.
        assert self.protection is not None
        # FERB's downloads folder and its upload copies of outside content are always outside content (D35, MP2):
        # code adds them, config can't remove them.
        roots = FsRoots(str(workspace), tuple(self.config.get("fs", "owner_roots", [])),
                        downloads.untrusted_roots(self.config.get("fs", "untrusted_roots", ["~/Downloads"]), workspace),
                        protection=self.protection)
        assert self.ledger and self.spans and self.journal and self.db and self.shells is not None
        assert self.confirmations is not None and self.destinations is not None
        from ferb.orchestrator.skills import Skills, SkillsRead
        skills = Skills(self.home / 'skills')
        self.registry.add(SkillsRead(skills))
        # L39: what each step's files looked like before it changed them, for its checker's diff. Closed exactly as
        # the shell's sandbox closes reads (FERB's state, the never-read stores: Protection.read_denied), whatever
        # FERB's file tools may read: the unsandboxed daemon must never read for the shell what its sandbox denies
        protection = self.protection
        self.changes = Changes(self.home / "changes", closed=lambda path: protection.read_denied(path) is not None)
        test_mode = self.config.get('test_mode', 'state', {})
        allowlist: set[str] | None = None
        record_questions = False
        if isinstance(test_mode, dict) and test_mode.get('enabled') is True:
            allowed = test_mode.get('allow_send', [])
            allowlist = (set(allowed) if isinstance(allowed, list) and
                         all(isinstance(item, str) and item.strip() for item in allowed) else set())
            record_questions = test_mode.get('questions') == "record"   # GATE-546
        questions = ports.QuestionRouter()
        self.gateway = Gateway(self.registry.capabilities, self.state, self.ledger, self.spans, self.tasks,
                               self.clock, self.redact, self._publish, roots, device, deps=GatewayDeps(
                                   evidence=self.evidence, recovery=self._recovery(), snapshots=self.snapshots,
                                   changes=self.changes, confirmations=self.confirmations,
                                   destinations=self.destinations,
                                   proc_task=self.shells.live_owner,   # a running shell process's destinations
                                   app_window=self._app_window,
                                   journal_problem=lambda kind, data: self._journal().append(kind, data),   # R4
                                   profile_name=self._profile_display_name,   # browser_pair's question names it
                                   profile_account=self._profile_account,     # and its signed-in account
                                   disturbance=self.disturbance, username_sources=self._username_sources,
                                   owner_contacts=self._owner_contacts,       # GATE-538
                                   on_path=self.attribution.ferb_touched, route_question=questions,
                                   shadow_action=self._destructive_shadow, test_send_allowlist=allowlist,
                                   test_questions=record_questions,
                                   standing=StandingPermissions(self.db),
                                   signed_out=lambda url: signed_out.fetch(url)))     # GATE-569
        self.supervisor = Supervisor(
            make_runtime=self._make_runtime, gateway=self.gateway, state=self.state, tokens=self.tokens,
            tasks=self.tasks, spans=self.spans, journal=self.journal, publish=self._publish, clock=self.clock,
            ids=self.ids, device_id=device, workspace=str(workspace), on_violation=self._runtime_violation,
            on_task_end=self._task_end, facts=self._facts, checks=Checks(self._urls_of, self.spans), roots=roots,
            questions=questions,
            memory=ports.MemoryPort(
                # L36 §4: every worker and plan step gets the one search within the worker's limits (step 9b), its
                # task's targets also ranked alone
                memory_candidates=lambda words, focus: self._learning_candidates(words, memory_search.ATTENTION, focus=focus),
                memory_push=lambda words, focus: self._learning_candidates(words, memory_search.WORKER, focus=focus),
                remember=self._remember, forget_request=self._chat_forget,
                question_answered=self._question_answered, settle=self._settle_forgets,
                identity_context=self._core_profile_text, identity_pending=self._core_profile_pending,
                settle_identity=self._settle_core_profile, on_episode=self._capture_learning_episode,
                typed_behind_memory=self._typed_behind_memory),            # attended scopes too (final review I1)
            schedules=ports.SchedulePort(
                timers=self.timers, on_schedule=self._schedule_create,
                on_schedule_change=self._schedule_change_from_chat,
                on_reminder_cancel=self._reminder_cancel_from_chat, schedule_live=self._schedule_live,
                on_schedule_edit=self._schedule_edit_from_chat, schedule_problems=self._schedule_problems,
                schedules_card=self._schedules_card),
            environment=ports.EnvironmentPort(
                environment=self._environment, live_facts=self._live_facts, availability=self._availability,
                apps=self._installed_apps,
                running_apps=self._running_apps,
                relevant_context=self._relevant_context,           # its fourth element: the search's source refs
                worker_types=self._worker_types, runtime_signed_out=self._runtime_signed_out,
                signals=self.signals, skills_catalog=skills.catalog,
                abilities_catalog=self.registry.abilities.catalog),
            desktop=ports.DesktopPort(
                on_task_start=self.disturbance.start, on_task_finish=self._tree_finished,
                disturbance=self.disturbance, disturbance_context=self.disturbance.context, changes=self.changes),
            browser=ports.BrowserPort(
                browser_context=self._browser_context, task_tabs=self._task_tabs, on_loop_rest=self._loop_rest,
                keep_tabs_open=self._keep_tabs_open, browsing=self._browsing),
            notify=ports.NotifyPort(
                on_loop_notice=self._loop_notice,
                on_resumed_outcome=self._resumed_outcome,          # a resumed scheduled run reports to its schedule
                on_conversation_resume=self._resume_conversation_plans))
        # L39 M1: the records a stopped daemon left go now, before any task, except those of trees that will resume
        # (their checks still see the changes made before the restart)
        resuming = set(self.supervisor.plans.pending())
        if stuck := self.changes.sweep(keep=resuming.__contains__):
            self._journal().append("changes.cleanup_failed", {"at": "start", "paths": len(stuck)})
        add_context_readers(self.registry, self.supervisor.continuity,
                            lambda task_id: (rec.tree_id if (rec := self.tasks.get(task_id)) else None),
                            lambda: self.memory, lambda ref: self.learning._source_origins(ref) if self.learning else {origin_.external('unlabeled'), origin_.PRIVATE},
                            search=self._memory_item_search, chat_search=self._chat_search)
        from ferb.identity.secret_tools import SecretsList
        self.registry.add(SecretsList(lambda: self.home,               # L36 §5: names only, never values
                                      lambda ctx, place: self.gateway.hidden._question_sites(ctx, place) if self.gateway
                                      else [], self._username_sources))
        from ferb.daemon.about import AboutRead
        self.registry.add(AboutRead(self._about_document))             # DMN-6: read-only, redacted
        from ferb.orchestrator.conversation_tools import ConversationRead
        self.registry.add(ConversationRead(self.supervisor.conversations, self.tasks.tree_of))
        # the live preview's state over its last frame (APP-404), from the supervisor's records
        self.gateway.hooks.previews.status = self.supervisor.preview_status
        self.stop_control.register("runtimes", self.supervisor.cancel_all)
        self.stop_control.register("identity_browser", self._stop_identity_browser)

    def _gateway(self) -> Gateway:
        if self.gateway is None:
            raise FerbError("daemon not started")
        return self.gateway

    def _recovery(self) -> recovery.Recovery:
        """S1: where FERB keeps copies of files before it changes them (~/.ferb/recovery) and their records."""
        if self.db is None:
            raise FerbError("daemon not started")
        return recovery.Recovery(self.home / "recovery", self.db, self.clock, self.ids)

    def _publish(self, topic: str, data: dict[str, Any]) -> None:
        if topic == "approval.requested" and data.get("task_id") and "request_id" not in data:
            asked = self.state.approvals.get(str(data.get("approval_id", "")))
            if asked is not None and asked.request_id:      # a task's question names its request (event contract)
                data = {**data, "request_id": asked.request_id}
        self.events.publish(topic, data)

    def _wire_events(self) -> None:
        """Who hears a published event, in this order, each given the data redacted once (`Events`)."""
        def plans(topic: str, data: dict[str, Any]) -> None:
            if self.supervisor:
                self.supervisor.notify_plan_event(topic, data)

        def activity(topic: str, data: dict[str, Any]) -> None:
            # SENSE-1: accessibility rows stay in the trail only (pull-only, trail.query), never in today's
            # activity or its per-turn search. Each row's data is stored once (fix list: it was also copied as
            # `where`).
            if data.get('source') == 'accessibility':
                return
            self.daily_activity.record(dict(data, what=data.get('what', data.get('source', topic)),
                                            device_id=self._activity_device()))

        def conversations(topic: str, data: dict[str, Any]) -> None:
            if self.supervisor is not None:
                self.supervisor.conversations.observe(data)

        def attention(topic: str, data: dict[str, Any]) -> None:
            if not self._shutting_down:
                self._attention_arrived(data)

        self.events.on(None, plans)
        self.events.on({'trail.event', 'action.observed'}, activity)
        self.events.on({'trail.event'}, conversations)
        self.events.on({'perception.attention'}, attention)
        self.events.on(None, lambda topic, data: self.bus.publish(topic, data))   # the bus as it is then
        self.events.on({'approval.requested'}, lambda topic, data: self._approval_notice(data))
        self.events.on({'secret.refused'}, lambda topic, data: self._spawn(self._tell_refused(data), topic))   # GATE-562
        self.events.on({'approval.requested', 'approval.resolved'}, self._question_record)
        # ORCH-312: a pause stops the task's terminal commands and listening, and a scheduled run's
        # pause pauses its schedule (resumed with it)
        self.events.on({'task.paused', 'task.resumed'}, lambda topic, data: self._spawn(
            self._task_paused(data) if topic == 'task.paused' else self._task_resumed(data), topic))

    def _question_record(self, topic: str, data: dict[str, Any]) -> None:
        """Every question FERB asks the owner, with its kind, and how it was answered (fix list: so "few questions" can
        be measured): `question.asked` and `question.answered` in the journal. The kind is structural (who asks and
        why: a task's question, FERB's confirmation, a check-in, a closed tab, a runtime switch, a secret...), never a
        reading of its words. An answer records its state and the option chosen, never free-typed words."""
        ap = self.state.approvals.get(str(data.get("approval_id", "")))
        if ap is None:
            return
        kind = str(data.get("question_kind") or "") or question_kind(ap.capability, ap.kind)
        if topic == "approval.requested":
            self._asked_kinds[ap.id] = kind
            self._journal().append("question.asked", {"approval_id": ap.id, "kind": kind,
                                                      "capability": ap.capability,
                                                      "task_id": data.get("task_id") or ap.task_id or None,
                                                      "request_id": ap.request_id})
            return
        if ap.state == "pending" or ap.id in self._answered_ids:
            return
        self._answered_ids.add(ap.id)
        self._journal().append("question.answered", {
            "approval_id": ap.id, "kind": self._asked_kinds.pop(ap.id, kind), "state": ap.state,
            "answer": "option" if ap.choice else "words" if ap.answer else "none",
            **({"choice": ap.choice} if ap.choice else {})})

    def _activity_device(self) -> Any:
        """This Mac's device id as today's activity rows record it (redacted, as the row's other values are)."""
        held = self._activity_device_id
        if held is None or held[0] != self.device_id:
            held = self._activity_device_id = (self.device_id, self.redact({"device_id": self.device_id})["device_id"])
        return held[1]
