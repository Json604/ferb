"""Compose the daemon: config at start, the stores, the gateway with its deps, the supervisor with its ports, and the
published events' consumers. Must not interpret requests or select targets."""

from pathlib import Path
from typing import Any

from ferb.act.browser import downloads
from ferb.act.fs import recovery
from ferb.act.fs.changes import Changes
from ferb.core import origin as origin_
from ferb.core.errors import FerbError
from ferb.daemon.gateway import FsRoots, Gateway
from ferb.memory.learning import WORKER_MEMORY_ITEMS
from ferb.orchestrator.supervisor import Supervisor
from ferb.orchestrator.verify import Checks
from ferb.policy import protection as protection_lists
from ferb.policy.destinations import Confirmations, KnownDestinations
from ferb.registry import add_context_readers
from ferb.sanitize.redact import Redactor


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
        if isinstance(test_mode, dict) and test_mode.get('enabled') is True:
            allowed = test_mode.get('allow_send', [])
            allowlist = (set(allowed) if isinstance(allowed, list) and
                         all(isinstance(item, str) and item.strip() for item in allowed) else set())
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
                                   disturbance=self.disturbance, username_sources=self._username_sources,
                                   on_path=self.attribution.ferb_touched, route_question=questions,
                                   shadow_action=self._destructive_shadow, test_send_allowlist=allowlist))
        self.supervisor = Supervisor(
            make_runtime=self._make_runtime, gateway=self.gateway, state=self.state, tokens=self.tokens,
            tasks=self.tasks, spans=self.spans, journal=self.journal, publish=self._publish, clock=self.clock,
            ids=self.ids, device_id=device, workspace=str(workspace), on_violation=self._runtime_violation,
            on_task_end=self._task_end, facts=self._facts, checks=Checks(self._urls_of, self.spans), roots=roots,
            questions=questions,
            memory=ports.MemoryPort(
                # L36 §4: every worker and plan step gets the one search at the owner's worker budget, its task's
                # targets also ranked alone
                memory_candidates=lambda words, focus: self._learning_candidates(words, focus=focus),
                memory_push=lambda words, focus: self._learning_candidates(words, WORKER_MEMORY_ITEMS, focus=focus),
                remember=self._remember, forget_request=self._chat_forget,
                question_answered=self._question_answered, settle=self._settle_forgets,
                identity_context=self._learning_identity, on_episode=self._capture_learning_episode,
                typed_behind_memory=self._typed_behind_memory),            # attended scopes too (final review I1)
            schedules=ports.SchedulePort(
                timers=self.timers, on_schedule=self._schedule_create,
                on_schedule_change=self._schedule_change_from_chat,
                on_reminder_cancel=self._reminder_cancel_from_chat, schedule_live=self._schedule_live),
            environment=ports.EnvironmentPort(
                environment=self._environment, availability=self._availability, apps=self._installed_apps,
                running_apps=self._running_apps,
                pasted_is_data=lambda: bool(self.config.get("policy", "pasted_text_is_data", True)),
                relevant_context=self._relevant_context,           # its fourth element: the search's source refs
                worker_types=self._worker_types,
                signals=self.signals, skills_catalog=skills.catalog,
                abilities_catalog=self.registry.abilities.catalog),
            desktop=ports.DesktopPort(
                on_task_start=self.disturbance.start, on_task_finish=self._tree_finished,
                disturbance=self.disturbance, disturbance_context=self.disturbance.context, changes=self.changes),
            browser=ports.BrowserPort(
                browser_context=self._browser_context, task_tabs=self._task_tabs, on_loop_rest=self._loop_rest),
            notify=ports.NotifyPort(
                on_loop_notice=self._loop_notice,
                on_resumed_outcome=lambda event: self._task_result(event, shown=False),
                on_conversation_resume=self._resume_conversation_plans))
        # L39 M1: the records a stopped daemon left go now, before any task, except those of trees that will resume
        # (their checks still see the changes made before the restart)
        resuming = set(self.supervisor.plans.pending())
        if stuck := self.changes.sweep(keep=resuming.__contains__):
            self._journal().append("changes.cleanup_failed", {"at": "start", "paths": len(stuck)})
        add_context_readers(self.registry, self.supervisor.continuity,
                            lambda task_id: (rec.tree_id if (rec := self.tasks.get(task_id)) else None),
                            lambda: self.memory, lambda ref: self.learning._source_origins(ref) if self.learning else {origin_.external('unlabeled'), origin_.PRIVATE},
                            search=self._memory_item_search)
        from ferb.identity.secret_tools import SecretsList
        self.registry.add(SecretsList(lambda: self.home,               # L36 §5: names only, never values
                                      lambda ctx, place: self.gateway.hidden._question_sites(ctx, place) if self.gateway
                                      else [], self._username_sources))
        from ferb.orchestrator.conversation_tools import ConversationRead
        self.registry.add(ConversationRead(self.supervisor.conversations, self.tasks.tree_of))
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
            self.daily_activity.record(dict(data, what=data.get('what', data.get('source', topic)),
                                            where=data.get('where', data.get('data', {})),
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

    def _activity_device(self) -> Any:
        """This Mac's device id as today's activity rows record it (redacted, as the row's other values are)."""
        held = self._activity_device_id
        if held is None or held[0] != self.device_id:
            held = self._activity_device_id = (self.device_id, self.redact({"device_id": self.device_id})["device_id"])
        return held[1]
