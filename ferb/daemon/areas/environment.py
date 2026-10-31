"""Own what the planner and workers are told about this Mac and FERB right now: environment, availability, facts, a
task's tabs and relevant activity. Must not select a route: agents interpret it."""

import asyncio
import json
from typing import Any

from ferb.act.browser.autoinstall import accepted_extension_ids
from ferb.core.types import Identity


# A paired owner profile that isn't connected, where FERB can't start it without a window (no wake service): plain
# words, not a fault (live 2026-10-24: "extension transport disconnected" read as broken)
IDLE_PROFILE = "not connected now — FERB connects once that profile is open in its browser"


class Environment:
    """A mixin of the Daemon (ferb/daemon/main.py): its methods use the daemon's state."""

    async def _environment(self, own_words: str) -> dict[str, Any]:
        """Installed apps and browser connection facts; the model interprets the owner's requested route. The
        database is read here, on the event loop; the Mac's and the browsers' files and the running apps are read in a
        thread (`_environment_files`), so a slow read never holds up another message's ack or a task's live line (L8)."""
        from ferb.policy import services as svc
        ferb = self._identity_profile()
        bindings = self.pairings.all() if self.pairings else []
        alignment = self.journal.of_running_tasks("security.alignment") if self.journal is not None else []
        found, apps, running, macos, extension = await asyncio.to_thread(
            self._environment_files, {(b.browser, b.folder) for b in bindings})
        names = {(p.browser, p.folder): p.display_name for p in found} if self.pairings else {}
        paired = [(b.browser, b.folder, names.get((b.browser, b.folder), b.folder)) for b in bindings
                  if b.identity is Identity.OWNER and (b.browser, b.folder) != ferb]
        stable, live = svc.summary(svc.Environment(apps, paired), macos, running)
        profiles = [{'browser': p.browser, 'profile': p.folder, 'name': p.display_name, 'account_hint': p.email,
                     'identity': 'ferb' if (p.browser, p.folder) == ferb else 'owner'} for p in found]
        # each profile's connection changes between turns: with the live facts, after the conversation (decision 4)
        connections = [{'browser': p.browser, 'profile': p.folder,
                        'connection': self._profile_state(p, extension.get((p.browser, p.folder)))} for p in found]
        if alignment:
            live += "\nNon-enforcing alignment advice for active tasks: " + json.dumps([e.data for e in alignment])
        if connections:
            live += '\nBrowser profile connections now: ' + json.dumps(connections)
        stable += '\nBrowser/profile inventory (facts, not a selected route): ' + json.dumps(profiles)
        # in the planner's fixed Layer 1 (never dropped for room, unlike today's activity): what worker_type may name
        stable += '\nWorker types: ' + json.dumps(self._worker_types())
        return {"stable": stable, "live": live}

    def _environment_files(self, paired: set[tuple[str, str]]) -> tuple[list[Any], list[tuple[str, str]], set[str], str,
                                                                        dict[tuple[str, str], bool]]:
        """The environment's reads that touch no database, run in a thread: browser profiles (Local State), installed
        apps (the device profile), running apps, the macOS version, and whether FERB's extension is in each profile not
        yet paired (its Preferences files)."""
        import platform
        from ferb.act.browser.profiles import extension_state
        from ferb.act.mac import apps as appsm
        found = self._profiles()
        try:
            running = {a.bundle_id for a in appsm.running()}
        except Exception:
            running = set()
        ids = frozenset(accepted_extension_ids(self.config))   # the pinned unpacked id and the store id (L25)
        extension = {(p.browser, p.folder): extension_state(p, ids, (self.browser_data_dirs or {}).get(p.browser))
                     for p in found if (p.browser, p.folder) not in paired}
        return found, self._installed_apps(), running, platform.mac_ver()[0] or "unknown", extension

    def _task_root(self, task_id: str) -> str:
        """The request's root task a worker step belongs to: download budgets and the tab cap are per task tree."""
        rec = self.tasks.get(task_id)
        return (rec.tree_id if rec is not None and rec.tree_id else None) or task_id.split(".", 1)[0]

    def _inspection_parent(self, task_id: str) -> str | None:
        """A registered checker borrows its worker's tabs/evidence, without changing their ownership."""
        rec = self.tasks.get(task_id)
        if rec is None or rec.task.role != "verifier":
            return None
        child = rec.task
        parent = self.tasks.get(child.parent_id) if child.parent_id else None
        if parent is None or parent.task.role != "worker" or not child.scopes <= parent.task.scopes:
            return None
        if (child.request_id, child.identity, child.device_id, rec.tree_id) != (
                parent.task.request_id, parent.task.identity, parent.task.device_id, parent.tree_id):
            return None
        return parent.task.id

    async def _loop_rest(self, task_id: str, resting: bool) -> None:
        """A loop's round starts or ends (L31): each browser driver gives the owner's held tab its file picker back
        between rounds, or takes it for the round (`_TabDriver.rest`, extension 0.15.10+)."""
        for drv in (self.router, self.ferb_identity):
            if drv is not None:
                await drv.rest(task_id, resting)

    def _task_tabs(self, task_id: str) -> list[dict[str, Any]]:
        """The browser tabs a task holds now, by tab id, with the page each held when the task took it over (a checker's
        browser context; a loop round's step context, L31). A model reads them: redacted as every tool result is
        (re-review 2, I4)."""
        tabs = [{"tool_prefix": prefix, **tab}
                for prefix, drv in (("browser", self.router), ("ferb_browser", self.ferb_identity))
                if drv is not None for tab in drv.task_tabs(task_id)]
        return list(self.redact(tabs)) if tabs else []

    def _browser_context(self, task_id: str) -> str:
        parent = self._inspection_parent(task_id)
        if parent is None:
            return ""
        tabs = self._task_tabs(parent)
        return ("\n\nExisting task tabs available for inspection (reuse these aliases; do not reopen "
                "the page or guess tab ids). A tab is known by its id: one you navigated elsewhere is still the same "
                "tab, and taken_over_from is the page it held when the task took it over: "
                + json.dumps(tabs)) if tabs else ""

    def _relevant_context(self, words: str) -> tuple[str, str, str, list[str]]:
        """Today's matching activity, with its origin (S3): recorded trail rows carry the trail labels; FERB's own
        observed actions are FERB's. No copy of the tool catalog (step 10b, prompts fix list): a worker gets its tools'
        descriptions once, with the tools; actions.search reads the registry."""
        from ferb.core import origin
        from ferb.sanitize.untrusted import wrap_untrusted
        from ferb.sense.tools import trail_sources
        activity = self.daily_activity.search(words)        # the turn's one search (L28): text and refs share it
        from ferb.memory.search import PLANNER, tokens
        from ferb.sense.daily import without_queries
        activity = without_queries(activity)               # addresses without query strings (live 2026-10-28)
        # decision 7 (step 9b): within the activity limit, best first, never a row cut; the rest counted
        kept, used = [], 0
        for row in activity:
            cost = tokens(json.dumps(row, ensure_ascii=False))
            if used + cost <= PLANNER.activity_tokens:
                kept.append(row)
                used += cost
        left_out = len(activity) - len(kept)
        activity = kept
        trail = {str(row['source']) for row in activity if row.get('source')}
        labels = {origin.from_read_source(k, i) for k, i in trail_sources(trail)} if trail else set()
        return ('\nRelevant activity today (data): ' + wrap_untrusted(json.dumps(activity)) + '\n' +
                (f'({left_out} more matching activity row(s) today left out to stay within the limit; '
                 'trail.query finds them.)\n' if left_out else ''),
                origin.join({origin.FERB} | labels),
                json.dumps([row["data"] if "data" in row else row for row in activity], ensure_ascii=False),
                ["trail:" + str(row["source_id"]) for row in activity if row.get("source") and row.get("source_id")])

    def _availability(self) -> Any:
        """What the planner may use, and what's missing with how to fix it (owner decision, 2026-09-30). Only facts
        that hold across turns (the installed groups, the paired profiles, permissions): it sits in every role's
        instructions, the planner's cached prefix included (decision 4); whether a paired profile is connected right now
        is a live fact (`_live_facts`, "Layer 2 — right now")."""
        from ferb.orchestrator.availability import build
        registered = set().union(*(c.scopes for c in self.registry.capabilities.values()))
        problems: dict[str, str] = {}
        profiles = ""
        if "browser:act" in registered:
            ferb = self._identity_profile()
            owner = [b for b in (self.pairings.all() if self.pairings else [])
                     if b.identity is Identity.OWNER and (b.browser, b.folder) != ferb]
            names = {(p.browser, p.folder): p.display_name for p in self._fresh_profiles()} if owner else {}
            if not owner:
                problems["owner_browsers"] = ("none of the owner's browser profiles is paired — browser_pair pairs "
                                              "one (FERB asks the owner once)")
            profiles = ", ".join(f"{b.browser.capitalize()} “{names.get((b.browser, b.folder), b.folder)}”"
                                 for b in owner)
            if profiles:
                profiles = f"paired: {profiles} (whether each is connected now: the live facts)"
        prof = self._identity_profile()
        if prof is not None and self.identity_browser is None:   # FERB's own browser is part of this FERB
            problems["ferb_browser"] = "not paired yet"
        # Not open right now is fine: FERB opens its own profile when a task needs it (the owner's report, 2026-09-30)
        perms = self._fresh.get(self._permissions)      # TCC checks: kept fresh off the loop, not asked per call
        if registered & {"mac:read", "mac:act"} and not perms.get("accessibility", True):
            problems["mac"] = "FERB doesn't have the Accessibility permission — run `ferb doctor`"
        if "screen" in registered and not perms.get("screen", True):
            problems["screen"] = "FERB doesn't have the Screen Recording permission — run `ferb doctor`"
        if "audio" in registered and not perms.get("screen", True):   # app sound is captured under Screen Recording
            problems["hearing"] = "FERB doesn't have the Screen Recording permission, which covers app sound — the " \
                                  "owner grants it in FERB's Settings → Permissions"
        return build(registered, problems, profiles)

    def _urls_of(self, task_id: str) -> list[str]:
        return [u for drv in (self.router, self.ferb_identity) if drv is not None for u in drv.urls_of(task_id)]

    def _add_secret_value(self, value: str) -> None:
        add = getattr(self.redact, "add_secret_value", None)
        if add is not None:
            add(value)

    def _keychain_values(self) -> tuple[list[str], int]:
        """The values of FERB's Keychain handles ([secrets], and the API keys of the providers this home's runtime
        table uses) and of the owner's saved secrets, read through their stores (a worker thread: each read is a helper
        or `security` call). A home that names none reads nothing."""
        from ferb.identity.owner_secrets import OwnerSecrets
        from ferb.runtimes.catalog import ROLES
        from ferb.runtimes.keys import stored_api_keys
        values, unreadable = self.secrets.values()
        try:
            owner, missed = OwnerSecrets(self.home).values()
            values, unreadable = values + owner, unreadable + missed
        except Exception:                          # the handles file can't be read: those values come at first use
            unreadable += 1
        try:
            values += stored_api_keys({self._runtime_for(role) for role in ROLES})
        except Exception:
            unreadable += 1
        return values, unreadable

    async def _preload_secrets(self) -> None:
        """The known-value registry, preloaded at start (step 6a): FERB's Keychain handles and the owner's saved
        secrets are known before this run first uses them, so a page, an error or a model's echo of one is hidden from
        the start. Values stay in the daemon's memory; only counts are journaled."""
        preload = getattr(self.redact, "preload", None)
        if preload is None:
            return
        values, unreadable = await asyncio.to_thread(self._keychain_values)
        if values or unreadable:
            self._journal().append("secret.registry_preloaded", {"values": preload(values), "unreadable": unreadable})

    def _facts(self) -> str:
        """Facts about this FERB that hold across turns (in every role's instructions, a cacheable prefix)."""
        address = self.config.get("mailbox", "address", "")
        lines = [self._about_summary()]
        if address:
            lines.append(f"- FERB's own email (its identity, never the owner's): {address}")
        else:                                       # daemon decision 7: the owner skipped FERB's own account
            browser, _, folder = str(self.config.get("browsing", "owner_profile", "") or "").partition("/")
            lines.append("- FERB has no Google account or browser profile of its own (the owner can add one in "
                         "FERB.app → Settings → Browsers): what needs FERB's own inbox (one-time codes for FERB's own "
                         "sign-ups, sending from FERB's address) isn't set up; web work uses identity \"owner\" "
                         + (f"in the owner's {browser} profile folder “{folder}”, which they chose for it."
                            if browser and folder else "in a profile the owner names (none chosen for it)."))
        lines += [f"- FERB secret {s.ref.key}: {s.account_hint}, usable on {', '.join(s.allowed_origins)}"
                  for s in self.secrets.specs()]
        for phrase, meaning in self._time_meanings():
            lines.append(f"- When the owner says “{phrase}”, they mean: {meaning}")
        return "\n".join(lines)

    def _live_facts(self) -> str:
        """Facts that change between turns (decision 4: after the conversation in the planner's context, so the prefix
        before it stays cacheable): the schedules and reminders, and the owner's paired browser profiles as FERB can
        use them now."""
        lines: list[str] = []
        for sc in (self._listed_schedules() if self.schedules is not None else []):
            state = "done; its last run is under way" if sc.state == "done" else sc.state
            lines.append(f"- Schedule {sc.id} ({state}): {sc.request} — {sc.describe()}")
        for t in (self.timers.pending(("reminder", "timer")) if self.timers is not None else []):
            lines.append(f"- {t.kind.capitalize()} {t.id}: {t.message} — {t.fire_at.astimezone():%a %d %b %Y %H:%M}")
        if self.pairings is not None and self.hub is not None:           # the owner's browsers, as FERB can use them
            ferb = self._identity_profile()
            names: dict[tuple[str, str], str] | None = None
            for b in self.pairings.all():
                if (b.browser, b.folder) == ferb or b.identity is not Identity.OWNER:
                    continue
                br = self.hub.bridges.get((b.browser, b.folder))
                if names is None:            # one profile discovery per call, kept fresh off the loop (`_fresh`)
                    names = {(p.browser, p.folder): p.display_name for p in self._fresh_profiles()}
                name = names.get((b.browser, b.folder), b.folder)
                lines.append(f"- The owner's {b.browser} profile “{name}”: paired{self._paired_state(b, br)}")
            for browser in sorted(set(self.hub.unpaired.values())):
                note = self._unpaired_note(browser)
                if note:
                    lines.append(f"- {note}")
        return "\n".join(lines)

    def _keep_tabs_open(self, task_id: str) -> None:
        """Hands decision 3: the owner asked to keep this task's tabs; at its end they leave the group and stay open."""
        for drv in (self.router, self.ferb_identity):
            if drv is not None:
                drv.keep_tabs_open(task_id)

    def _paired_state(self, binding: Any, bridge: Any) -> str:
        """How the planner may use one paired owner profile now, in plain words: a profile that isn't connected is
        usually just not running, and the browser tools start it without a window when the wake service is there
        (`_TabDriver._connected`, spike 19), then open its tab themselves (`_TabDriver._open_in`). Live 2026-10-24:
        "extension transport disconnected" read as broken; live 2026-10-26: "may switch Space" made a worker ask."""
        if bridge is not None and bridge.conn:
            return ", connected"
        refused = self.hub.refused.get(binding.install_id) if self.hub is not None else None
        if refused:
            return f"; its extension was refused: {refused}"
        if self.router is not None and self.router.wake is not None:
            return ("; not connected now (usually the profile just isn't running) — FERB's browser tools start it in "
                    "the background and open its tab there themselves, nothing to ask")
        return f"; {IDLE_PROFILE}"
