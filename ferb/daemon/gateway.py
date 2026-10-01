"""Tool gateway (section 7 step 6): run token -> identity + scope -> policy (intent / exposure / dataflow /
autonomy / approvals / undo) -> ledger intent -> capability -> sanitize -> span -> result + time footer.

Every model action reaches the Mac only through here (rule 12). Failures are values: the worker always gets a
result, never an exception.
"""

import asyncio
import hashlib
import json
import os
import base64
import dataclasses
from dataclasses import dataclass
from typing import Any, Callable

from pydantic import ValidationError

from ferb.act.fs.files import normalize_path
from ferb.act.locks import ResourceLocks
from ferb.policy.taint import Taint
from ferb.clock import context
from ferb.core.interfaces import Capability, Clock
from ferb.core.types import ToolContext
from ferb.daemon.auth import RunGrant
from ferb.orchestrator.ledger import DuplicateUnresolved, Ledger
from ferb.orchestrator.tree import TaskTree
from ferb.policy.engine import PolicyEngine, Verdict
from ferb.policy.exposure import LOST_WHEN_EXPOSED
from ferb.policy import contacts
from ferb.policy.intent import site_matches
from ferb.policy.rules import REVERSIBLE, ActionRequest, Kind, payload_hash
from ferb.store.spans import SpanStore

Publish = Callable[[str, dict[str, Any]], None]
Redact = Callable[[Any], Any]


@dataclass
class FsRoots:
    workspace: str
    owner_roots: tuple[str, ...]
    untrusted_roots: tuple[str, ...]


class Observer:
    """What the orchestrator watches through the gateway: tool calls (runaway guards, liveness) and waits on the owner
    (which pause the stall timer). No-ops until the supervisor installs its own."""

    def tool_call(self, grant: RunGrant, done: bool) -> None:
        pass

    def result(self, grant: RunGrant, tool: str, args: dict[str, Any], failed: bool, digest: str = "") -> None:
        """Every answered call (runaway guards: the same failing action, loops)."""

    def waiting(self, worker: str, on: bool) -> None:
        pass


class Gateway:
    def __init__(self, capabilities: dict[str, Capability], engine: PolicyEngine, ledger: Ledger, spans: SpanStore,
                 tasks: TaskTree, clock: Clock, redact: Redact, publish: Publish, roots: FsRoots,
                 device_id: str, hold_poll_s: float = 0.2) -> None:
        self.caps, self.engine, self.ledger, self.spans = capabilities, engine, ledger, spans
        self.tasks, self.clock, self.redact, self.publish = tasks, clock, redact, publish
        self.roots, self.device_id, self.hold_poll_s = roots, device_id, hold_poll_s
        self.locks = ResourceLocks()
        self.taint = Taint()
        self.observer: Observer = Observer()
        self.on_path: Callable[[str, str], None] = lambda path, task_id: None   # trail attribution (build step 10)
        self.jev: Any = None                             # decide.ShadowDecider (build step 13): stricter only
        # Progressive setup (section 8), supplied by the daemon: record a granted folder; queue a question for later
        self.grant_folder: Callable[[str], None] = lambda folder: None
        self.queue_question: Callable[[str, str, str], None] = lambda kind, target, text: None
        # Contacts (build step 16a): what this task's snapshots showed; the owner's words for a request + name
        self.seen = contacts.Seen()
        self._sent: dict[str, dict[str, str]] = {}      # task tree -> who was sent a message -> what (9.4)
        self.owner_words: Callable[[str, str], str] = lambda request_id, name: ""
        self.request_words: Callable[[str], str] = lambda request_id: ""    # this request's own words only
        self._picked: dict[tuple[str, str], str] = {}     # the owner's answer, for the next attempt of that one action
        self.name_of: Callable[[str], str] = lambda target: target    # bundle id -> app name, for plain questions

    def tools_for(self, grant: RunGrant) -> list[dict[str, Any]]:
        """What this worker may call: scopes granted, identity allowed, and not lost to exposure."""
        exposed = self.engine.exposure.state(grant.worker_id).exposed
        out = []
        for cap in self.caps.values():
            if not cap.scopes <= grant.scopes or grant.identity not in cap.identities:
                continue
            if exposed and cap.scopes & LOST_WHEN_EXPOSED:
                continue
            schema = cap.Input.model_json_schema()
            schema.setdefault("properties", {})["why"] = {"type": "string", "description": WHY_HELP}
            out.append({"name": cap.name, "description": cap.description, "input_schema": schema})
        return out

    def tools_for_task(self, task: Any) -> list[str]:
        """The tool names a worker of this task gets (its scopes and identity)."""
        return [c.name for c in self.caps.values() if c.scopes <= set(task.scopes) and task.identity in c.identities]

    async def call(self, grant: RunGrant, name: str, arguments: dict[str, Any]) -> dict[str, Any]:
        r = await self._call(grant, name, arguments)
        content = r.get("content") or {}
        waiting_on_owner = isinstance(content, dict) and "needs_owner_approval" in content   # not a failure
        digest = hashlib.sha256(json.dumps(content, sort_keys=True, default=str).encode()).hexdigest()[:16]
        self.observer.result(grant, name, arguments, bool(r.get("is_error")) and not waiting_on_owner, digest)
        return r

    async def _call(self, grant: RunGrant, name: str, arguments: dict[str, Any]) -> dict[str, Any]:
        rec = self.tasks.get(grant.task_id)
        started = rec.started_at if rec else None
        deadline = rec.task.deadline if rec else None

        def reply(content: dict[str, Any], is_error: bool) -> dict[str, Any]:
            return {"content": self.redact(content), "is_error": is_error,
                    "footer": context.footer(self.clock.now(), started, deadline)}

        cap = self.caps.get(name)
        if cap is None or name not in {t["name"] for t in self.tools_for(grant)}:
            return reply({"error": f"tool {name} is not available to this worker"}, True)
        if rec is None:
            return reply({"error": "task is not running"}, True)
        arguments = dict(arguments)
        why = str(arguments.pop("why", "") or "").strip()[:400]         # the worker's stated reason (10.2 / 10.3)
        try:
            args = cap.Input.model_validate(arguments)
        except ValidationError as e:
            return reply({"error": "invalid arguments", "details": e.errors(include_url=False)}, True)
        if hasattr(cap, "precheck") and (why := cap.precheck(args)):       # refused before any policy step
            return reply({"error": why}, True)

        spec = cap.describe(args)
        reader_click = (hasattr(cap, "reader_ok") and cap.reader_ok(args)) \
            or self._reader_types_own_words(name, args, rec.task.request_id)  # opening to read; searching their words
        if rec.task.role == "reader" and spec.kind not in REVERSIBLE and not reader_click:   # readers read (7.4)
            why_not = f"blocked by FERB policy: a reader subtask can't {spec.kind}"
            if name == "mac.act":                         # say what a reader CAN do (owner's run: it gave up instead)
                why_not += (". A reader may press a labeled control that only opens something, and may type the "
                            "owner's own words into a box with action set_value — that also runs a search, even in a "
                            "box that looks like a button (e.g. WhatsApp's search). Try that.")
            refusal = self.spans.start("tool.call", task_id=grant.task_id, parent_id=rec.span_id,   # in the trail
                                       device_id=self.device_id, input={"tool": name, "args": arguments})
            self.spans.finish(refusal, error=why_not)
            return reply({"error": why_not}, True)
        labels = self.taint.labels_for(rec.tree_id, spec.content)          # owner data inside this content (10.2)
        if labels:
            spec = dataclasses.replace(spec, data_labels=spec.data_labels | labels)
        if spec.target.path is not None:                 # the same path form the intent was recorded in
            spec = dataclasses.replace(spec, target=dataclasses.replace(
                spec.target, path=normalize_path(spec.target.path, self.roots.workspace)))
        req_intent = self.engine.intents.get(rec.task.request_id)
        action = ActionRequest(capability=name, kind=spec.kind, identity=grant.identity, scopes=frozenset(cap.scopes),
                               task_id=grant.task_id, worker_id=grant.worker_id, request_id=rec.task.request_id,
                               task_scopes=grant.scopes, target=spec.target, content=spec.content,
                               data_labels=spec.data_labels, unverified_account=spec.unverified_account,
                               flagged_irreversible=bool(req_intent and spec.kind in req_intent.irreversible))
        if beyond_plan(spec.target.site, req_intent):                     # exploring / a route outside the plan
            blocked = self.engine.blocker(action) if spec.kind not in REVERSIBLE else None
            if blocked is not None:                                      # never offered: policy forbids it anyway
                self.publish("policy.blocked", {"step": blocked[0], "reason": blocked[1], "capability": name,
                                                "task_id": grant.task_id, "worker_id": grant.worker_id})
                return reply({"error": f"blocked by FERB policy: {blocked[1]}"}, True)
            if not why:
                return reply({"error": EXPLORE_NEEDS_WHY if spec.kind in REVERSIBLE else ROUTE_NEEDS_WHY}, True)
            if spec.kind not in REVERSIBLE:                              # acting there: the owner decides
                refused = await self._route_change(name, spec.target.site or "", why, action, grant)
                if refused:
                    return reply({"error": refused}, True)
        if spec.kind in (Kind.CALL, Kind.SEND) and "contact" in type(args).model_fields \
                and self._needs_contact(spec.kind, rec.task.request_id):        # never guess between contacts (16a)
            got = await self._contact(cap, name, args, action, grant, rec)
            if isinstance(got, str):
                return reply({"error": got}, True)
            action, spec = got, dataclasses.replace(spec, target=got.target)
        tree = rec.tree_id or grant.task_id
        if spec.kind == Kind.SEND and (to := _send_to(action.target)) and to in self._sent.get(tree, {}):
            refused = await self._send_again(name, to, self._sent[tree][to], spec.content, action, grant)
            if refused:
                return reply({"error": refused}, True)
        span = self.spans.start("tool.call", task_id=grant.task_id, parent_id=rec.span_id, device_id=self.device_id,
                                input={"tool": name, "args": arguments} | ({"why": why} if why else {}))
        self.publish("tool.called", {"tool": name, "task_id": grant.task_id, "worker_id": grant.worker_id})

        verdict = self.engine.evaluate(action)
        self._events(verdict)
        if verdict.decision == "ask":                                    # wait for the owner, then carry on (10.3)
            verdict = await self._await_owner(grant, action, verdict)
        if verdict.decision == "block":
            self.spans.finish(span, error=f"blocked ({verdict.step}): {verdict.reason}")
            return reply({"error": f"blocked by FERB policy: {verdict.reason}"}, True)
        if verdict.decision == "ask":
            self.spans.finish(span, output={"needs_approval": verdict.approval_id})
            return reply({"needs_owner_approval": verdict.approval_id, "reason": verdict.reason,
                          "next": "The owner has been asked in `ferb chat`. Retry the exact same call after they "
                                  "approve; don't change it."}, True)
        if self.jev is not None and spec.kind not in REVERSIBLE:          # Jev: only ever stricter (section 12)
            stricter = await self._jev_action(cap, name, spec, action, rec, span.id)
            if stricter is not None:
                if verdict.decision == "hold":
                    self.engine.undo(verdict.hold_id or "")
                kind, why = stricter
                if kind == "block":
                    self.spans.finish(span, error=f"blocked (jev): {why}")
                    self.publish("policy.blocked", {"step": "jev", "reason": why, "capability": name,
                                                    "task_id": grant.task_id, "worker_id": grant.worker_id})
                    return reply({"error": f"blocked by FERB policy: {why}"}, True)
                flagged = dataclasses.replace(action, flagged_irreversible=True)     # Jev: ask every time
                if self.engine.approvals.consume(flagged) is None:
                    ap = self.engine.approvals.request(flagged, why)
                    self.publish("approval.requested", {"approval_id": ap.id, "capability": name, "kind": spec.kind,
                                                        "reason": why, "task_id": grant.task_id})
                    answer = await self._wait_answer(grant, ap.id)
                    if answer != "granted" or self.engine.approvals.consume(flagged) is None:
                        self.spans.finish(span, error=f"not approved ({answer})")
                        return reply({"error": _declined(answer)}, True)
        if verdict.decision == "hold":
            self.observer.waiting(grant.worker_id, True)                  # the undo window is the owner's time
            try:
                state = await self._wait_hold(verdict.hold_id or "")
            finally:
                self.observer.waiting(grant.worker_id, False)
            if state != "released":
                self.spans.finish(span, error="cancelled by the owner during the undo window")
                return reply({"error": "cancelled by the owner (undo)"}, True)

        lock_keys = cap.locks(args) if hasattr(cap, "locks") else []
        self.observer.tool_call(grant, done=False)
        try:
            async with self.locks.hold(lock_keys, grant.worker_id):        # after policy, never across model calls
                return await self._execute(cap, name, args, spec, action, grant, rec, span, reply)
        finally:
            self.observer.tool_call(grant, done=True)

    async def _execute(self, cap: Capability, name: str, args: Any, spec: Any, action: ActionRequest,
                       grant: RunGrant, rec: Any, span: Any, reply: Callable[[dict[str, Any], bool], dict[str, Any]]) \
            -> dict[str, Any]:
        intent_id = None
        if spec.kind not in REVERSIBLE:                  # consequential: ledger row BEFORE it runs
            try:
                intent_id = self.ledger.intend(grant.task_id, name, _target_dict(spec.target), payload_hash(action)).id
                self.ledger.mark(intent_id, "executing")
            except DuplicateUnresolved as e:
                self.spans.finish(span, error=str(e))
                return reply({"error": str(e)}, True)

        ctx = ToolContext(task_id=grant.task_id, worker_id=grant.worker_id, request_id=rec.task.request_id,
                          identity=grant.identity, scopes=grant.scopes, device_id=self.device_id, span_id=span.id,
                          workspace=self.roots.workspace, owner_roots=self.roots.owner_roots,
                          untrusted_roots=self.roots.untrusted_roots, intent_id=intent_id)
        if spec.target.path is not None:
            self.on_path(spec.target.path, grant.task_id)              # file events it causes are FERB's
        try:
            result = await cap.run(args, ctx)
            if (result.is_error and name.startswith("fs.") and spec.target.path
                    and "outside the allowed folders" in str(result.content.get("error", ""))):
                folder = await self._folder_access(grant, action, spec.target.path)   # progressive setup (8)
                if folder is not None:
                    ctx = dataclasses.replace(ctx, owner_roots=self.roots.owner_roots)
                    result = await cap.run(args, ctx)
        except Exception as e:                           # outcome unknown: the ledger reconciles, never retries blindly
            if intent_id:
                self.ledger.mark(intent_id, "unknown", [f"exception: {type(e).__name__}"])
            self.spans.finish(span, error=f"{type(e).__name__}: {e}")
            return reply({"error": f"{name} failed ({type(e).__name__}); outcome unknown"}, True)
        if intent_id:
            self.ledger.mark(intent_id, "failed" if result.is_error else "done", [span.id])
        if spec.kind == Kind.SEND and not result.is_error and (to := _send_to(action.target)):
            self._sent.setdefault(rec.tree_id or grant.task_id, {}).setdefault(to, spec.content or "")
        for kind, source_id in result.read_sources:
            self.taint.read(grant.worker_id, kind, source_id)
            if self.engine.exposure.read(grant.worker_id, kind, source_id):
                self.publish("worker.exposed", {"worker_id": grant.worker_id, "task_id": grant.task_id,
                                                "reason": f"read {kind}: {source_id}"})
        content = self.redact(result.content)
        if (name.endswith(".snapshot") or result.evidence) and not result.is_error:   # what was on screen (16a)
            shown = result.evidence or result.content
            self.seen.record(grant.task_id, self._where(cap, name, args, shown), shown)
        if self.jev is not None:
            stop = await self._jev_result(grant, name, result, content, span.id)
            if stop is not None:
                self.spans.finish(span, output=content, error=stop["error"])
                return reply(stop, True)
        self.spans.finish(span, output=content, error=content.get("error") if result.is_error else None)
        out = reply(content, result.is_error)
        if result.image is not None and not result.is_error:     # to the model only: not in the span, journal or logs
            out["image"] = {"mime": "image/jpeg", "data": base64.b64encode(result.image).decode()}
        return out

    async def _route_change(self, name: str, target: str, why: str, action: ActionRequest,
                            grant: RunGrant) -> str | None:
        """An action outside the plan's apps / sites: the worker's plain-words proposal goes to the owner ("Your Brave
        had no call button. The WhatsApp app does. Call Rahul from there?"). A yes adds that app / site to this
        request for the rest of it; then normal policy runs. Returns why not, or None to carry on."""
        ask = dataclasses.replace(action, kind="route_change", content=None, flagged_irreversible=False)
        question = f"{why} (FERB would {KIND_WORDS.get(action.kind, action.kind)} in {self.name_of(target)}.)"   # facts from code
        ap = self.engine.approvals.request(ask, question)
        self.publish("approval.requested", {"approval_id": ap.id, "capability": name, "kind": "route_change",
                                            "reason": question, "task_id": grant.task_id})
        answer = await self._wait_answer(grant, ap.id)
        if answer != "granted":
            return (_declined(answer, action.request_id in self.engine.approvals.unattended) +
                    "; stay within the plan's apps and sites")
        self.engine.intents.extend(action.request_id, target)
        self.publish("plan.extended", {"task_id": grant.task_id, "request_id": action.request_id, "target": target,
                                       "why": why})
        return None

    async def _send_again(self, name: str, to: str, first: str, text: str | None, action: ActionRequest,
                          grant: RunGrant) -> str | None:
        """A second message to the same contact in one task (owner decision 2026-10-01): never a retry by default —
        the worker checks by reading the chat; sending again needs the owner's OK, with both texts shown."""
        said = f": “{first[:120]}”" if first else ""
        new = f": “{text[:120]}”" if text else ""
        question = f"FERB already sent {to} a message in this task{said}. Send another one{new}?"
        ask = dataclasses.replace(action, kind="send_again", flagged_irreversible=True)
        ap = self.engine.approvals.request(ask, question)
        self.publish("approval.requested", {"approval_id": ap.id, "capability": name, "kind": "send_again",
                                            "reason": question, "task_id": grant.task_id})
        answer = await self._wait_answer(grant, ap.id)
        if answer != "granted":
            return (_declined(answer, action.request_id in self.engine.approvals.unattended) +
                    f"; the first message to {to} stands — check it by reading the chat, don't send again")
        return None

    def _needs_contact(self, kind: str, request_id: str) -> bool:
        """Calls always name their contact; a send does when the owner named a person rather than an address."""
        if kind == Kind.CALL:
            return True
        req = self.engine.intents.get(request_id)
        return any("@" not in r and r.strip().lower() not in ("me", "myself", "owner") for r in
                   (req.recipients if req else ()))

    def _where(self, cap: Any, name: str, args: Any, content: dict[str, Any] | None) -> str:
        if name.startswith("mac."):
            if content and content.get("bundle_id"):
                return str(content["bundle_id"])
            return str(cap._bundle(args)) if hasattr(cap, "_bundle") else str(getattr(args, "app", ""))
        tab = (content or {}).get("tab") or getattr(args, "tab", "")
        return f"{name.split('.')[0]}:{tab}"

    async def _contact(self, cap: Any, name: str, args: Any, action: ActionRequest, grant: RunGrant,
                       rec: Any) -> ActionRequest | str:
        """The selected contact must have been on screen; several matches (and nothing the owner said picks one)
        means asking "Which Rahul?". The owner's answer is for this one action. Returns the action (its target now
        naming the exact contact) or why not."""
        c: contacts.Contact | None = getattr(args, "contact")
        if c is None:
            return ("say who this is for: contact = {name as the owner said it, selected as the screen shows it, "
                    "detail (number / username / subtitle) as the screen shows it}")
        req = self.engine.intents.get(action.request_id)
        rname = next((r for r in (req.recipients if req else ()) if r.strip().lower() == c.name.strip().lower()), None)
        if rname is None:
            named = ", ".join(sorted(req.recipients)) if req and req.recipients else "nobody"
            return f"“{c.name}” isn't someone the owner named in this request (they named: {named})"
        shots = self.seen.shots(grant.task_id, self._where(cap, name, args, None))
        words = self.owner_words(action.request_id, c.name)
        res = contacts.check(c, shots, words, self._picked.pop((grant.task_id, rname.lower()), None))
        how = "on screen"
        if res.decision == "ask":
            ap = self.engine.approvals.request_choice(action, res.why, res.options)
            self.publish("approval.requested", {"approval_id": ap.id, "capability": name, "kind": "contact_choice",
                                                "reason": res.why, "options": list(res.options),
                                                "task_id": grant.task_id})
            answer = await self._wait_answer(grant, ap.id)
            if answer != "granted" or ap.choice is None:
                return _declined(answer, action.request_id in self.engine.approvals.unattended)
            res, how = contacts.check(c, shots, words, ap.choice), "the owner chose"
            if res.decision != "ok":
                self._picked[(grant.task_id, rname.lower())] = ap.choice          # the retry of this one action
        if res.decision != "ok":
            return res.why
        shown = contacts.display(c)
        self.publish("contact.selected", {"task_id": grant.task_id, "name": c.name, "contact": shown, "how": how})
        return dataclasses.replace(action, target=dataclasses.replace(action.target, recipients=(rname,),
                                                                      contact=shown))

    def _reader_types_own_words(self, name: str, args: Any, request_id: str) -> bool:
        """A reader may type into an app only text from the owner's own words this request ("syllabus") — whatever the
        box is (WhatsApp's search box reports itself as an unlabeled button). Their own words can't leak anything, and
        a reader still can't press Send / Return. Owner's audit 2026-10-01: background search had worked as set_value;
        a box-type guess refused it."""
        if name != "mac.act" or getattr(args, "action", None) not in ("set_value", "type"):
            return False
        text = " ".join(str(getattr(args, "text", "") or "").lower().split())
        words = " ".join(self.request_words(request_id).lower().split())
        return bool(text) and len(text) <= 100 and text in words

    async def _folder_access(self, grant: RunGrant, action: ActionRequest, path: str) -> str | None:
        """"Allow FERB to access ~/Projects?" — asked only for a folder the owner named in this request (content FERB
        reads can't trigger it); a yes goes to config ([fs] owner_roots). An unattended run queues the question."""
        req = self.engine.intents.get(action.request_id)
        named = next((p for p in (req.paths if req else ()) if path == p or path.startswith(p.rstrip("/") + "/")), None)
        if named is None:                       # not in the plan's intent, but named in the owner's own words?
            named = _named_folder(path, self.request_words(action.request_id))
        if named is None:
            return None
        folder = named if not os.path.splitext(named)[1] else os.path.dirname(named)
        shown = folder.replace(os.path.expanduser("~"), "~", 1)
        if action.request_id in self.engine.approvals.unattended:
            self.queue_question("folder_access", folder, f"Allow FERB to access {shown}?")
            return None
        ask = dataclasses.replace(action, kind="folder_access", target=dataclasses.replace(action.target, path=folder),
                                  content=None, flagged_irreversible=False)
        ap = self.engine.approvals.request(ask, f"Allow FERB to access {shown}?")
        self.publish("approval.requested", {"approval_id": ap.id, "capability": "fs", "kind": "folder_access",
                                            "reason": ap.reason, "task_id": grant.task_id})
        if await self._wait_answer(grant, ap.id) != "granted":
            return None
        self.grant_folder(folder)
        return folder

    async def _await_owner(self, grant: RunGrant, action: ActionRequest, verdict: Verdict) -> Verdict:
        """The tool call waits for the owner's answer (their time is never a stall), then policy runs again: a grant
        covers this call (and, unless it's irreversible, the same target and type for the rest of the request)."""
        answer = await self._wait_answer(grant, verdict.approval_id or "")
        if answer != "granted":
            return Verdict("block", _declined(answer, action.request_id in self.engine.approvals.unattended), "approval")
        again = self.engine.evaluate(action)
        self._events(again)
        return again

    async def _wait_answer(self, grant: RunGrant, approval_id: str) -> str:
        self.observer.waiting(grant.worker_id, True)
        try:
            while True:
                ap = self.engine.approvals.get(approval_id)
                if ap is None:
                    return "gone"
                if ap.state != "pending":
                    return ap.state
                if self.clock.monotonic() >= ap.expires_at:
                    ap.state = "expired"
                    return "expired"
                await asyncio.sleep(self.hold_poll_s)
        finally:
            self.observer.waiting(grant.worker_id, False)

    async def _jev_action(self, cap: Capability, name: str, spec: Any, action: ActionRequest, rec: Any,
                          span_id: str) -> tuple[str, str] | None:
        """action.matches_request + action.risk in one request; ("block" | "ask", why) only when Jev is stricter."""
        from ferb.decide.decider import Ask
        req = self.engine.intents.get(action.request_id)
        state = {"request": {"goal": rec.task.goal, "allowed_kinds": sorted(req.kinds) if req else [],
                             "recipients": sorted(req.recipients) if req else [], "sites": sorted(req.sites) if req else [],
                             "paths": sorted(req.paths) if req else []},
                 "action": {"tool": name, "kind": spec.kind, "target": _target_dict(spec.target),
                            "content": (spec.content or "")[:2000]}}
        ref = f"span:{span_id}"
        got = await self.jev.ask(state, [Ask("action.matches_request", "yes", ref),
                                         Ask("action.risk", getattr(cap.risk, "name", "LOW").lower(), ref)])
        if got.get("action.matches_request") == "no":
            self.jev.mark_effect("action.matches_request", ref, "blocked")
            return "block", "a second check says this doesn't match what the owner asked for"
        if got.get("action.matches_request") == "unsure":
            self.jev.mark_effect("action.matches_request", ref, "asked the owner")
            return "ask", "a second check isn't sure this matches what you asked for"
        if got.get("action.risk") == "high":
            self.jev.mark_effect("action.risk", ref, "asked the owner")
            return "ask", "a second check rates this action high-risk"
        return None

    async def _jev_result(self, grant: RunGrant, name: str, result: Any, content: dict[str, Any],
                          span_id: str) -> dict[str, Any] | None:
        """content.injection_suspected (untrusted reads) + browser.login_state (owner snapshots) on one state.
        Returns replacement content when a signed-out page stops the worker."""
        from ferb.decide.decider import Ask
        from ferb.policy.exposure import UNTRUSTED_KINDS
        ref = f"span:{span_id}"
        asks = []
        if any(k in UNTRUSTED_KINDS for k, _ in result.read_sources):
            asks.append(Ask("content.injection_suspected", "no", ref))
        if name == "browser.snapshot":
            asks.append(Ask("browser.login_state", "unknown", ref))
        if not asks or result.is_error:
            return None
        got = await self.jev.ask({"tool": name, "content": content}, asks)
        if got.get("content.injection_suspected") == "yes":
            self.engine.exposure.injection_signal(grant.worker_id, "a second check found instructions in content")
            self.jev.mark_effect("content.injection_suspected", ref, "worker exposed")
            self.publish("worker.exposed", {"worker_id": grant.worker_id, "task_id": grant.task_id,
                                            "reason": "injection signal"})
        if got.get("browser.login_state") == "signed_out":
            self.jev.mark_effect("browser.login_state", ref, "stopped: signed out")
            return {"error": "this page looks signed out; FERB never signs in as the owner — stop and tell the owner"}
        return None

    async def _wait_hold(self, hold_id: str) -> str:
        while True:
            h = self.engine.holds.get(hold_id)
            if h is None or h.state in ("released", "undone"):
                return h.state if h else "undone"
            await asyncio.sleep(self.hold_poll_s)

    def _events(self, v: Verdict) -> None:
        for topic, data in v.events:
            self.publish(topic, data)


KIND_WORDS = {"call": "start a call", "send": "send a message", "mac_input": "click and type",
              "submit": "click / submit", "fill_form": "type", "post": "post", "reply": "reply",
              "write_file": "write a file", "applescript": "run AppleScript"}
WHY_HELP = ("Only when this goes outside the plan's apps and sites. Opening / reading there: a short reason. Acting "
            "there: one or two plain sentences for the owner — what you found and what you want to do, e.g. \"Your "
            "Brave had no call button. The WhatsApp app does. Call Rahul from there?\"")
EXPLORE_NEEDS_WHY = ("this app / site isn't in the plan: you may look, with a short reason — call again with why = "
                     "why you're looking here")
ROUTE_NEEDS_WHY = ("acting here is outside the plan's apps and sites. If it's the better route, call again with why = "
                   "one or two plain sentences for the owner: what you found and what you want to do (e.g. \"Your "
                   "Brave had no call button. The WhatsApp app does. Call Rahul from there?\"); FERB asks them")


def beyond_plan(site: str | None, intent: Any) -> bool:
    """A target outside the request's apps / sites (only when the plan named some)."""
    return bool(site and intent is not None and intent.sites and not site_matches(site, intent.sites))


def _declined(answer: str, unattended: bool = False) -> str:
    if answer == "expired" and unattended:
        return "the owner didn't answer within an hour; skip this step and say so in your report"
    return ("the owner didn't answer in time; don't try this again in this task" if answer == "expired"
            else "the owner declined this; don't try it again in this task")


def _target_dict(t: Any) -> dict[str, Any]:
    return {k: v for k, v in {"account": t.account, "site": t.site, "recipients": list(t.recipients),
                              "path": t.path, "amount": t.amount, "contact": t.contact}.items()
            if v not in (None, [], ())}


def _send_to(t: Any) -> str | None:
    """Who a send goes to, for the one-message-per-contact rule: the exact contact, else the recipients."""
    if t.contact:
        return str(t.contact)
    people = sorted(str(r) for r in t.recipients if r)
    return ", ".join(people) if people else None


def _named_folder(path: str, own_words: str) -> str | None:
    """The deepest folder on `path` that the owner named in their own words this request ("clg:uni folder in
    desktop" names ~/Desktop/clg:uni) — never a bare home folder (all of Desktop) and never from content FERB read
    (owner's report 2026-10-01: the access question never came because the plan left the folder out)."""
    from ferb.sanitize.untrusted import HOME_FOLDERS, targets_in_own_words
    if not own_words:
        return None
    home = os.path.expanduser("~")
    p = os.path.normpath(os.path.expanduser(path))
    while p.startswith(home + os.sep):
        rel = p[len(home) + 1:]
        if rel.lower() not in HOME_FOLDERS and not targets_in_own_words([rel], own_words):
            return p
        p = os.path.dirname(p)
    return None
