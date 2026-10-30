"""Own the Mac driver's and the desktop observer's wiring: native captures, the front window, a task's released
resources, installed and running apps. Must not act on the owner's screen itself."""

import asyncio
import os
import subprocess
from pathlib import Path
from typing import Any

from ferb.act.mac.driver import MacComputerDriver, profiles_dir, same_title
from ferb.act.mac.input import RealInput
from ferb.act.mac.launched import LaunchedApps, brought_forward
from ferb.act.mac.profile import Profiles
from ferb.act.shell.local import DirectReports, LocalAccess
from ferb.act.shell.manager import ShellManager
from ferb.act.shell.proxy import EgressProxy
from ferb.act.shell.sandbox import Sandbox
from ferb.core.errors import NoTarget, Refused, Withheld
from ferb.core.types import Identity
from ferb.registry import add_evidence, add_hearing, add_mac, add_shell, add_time, add_trail
from ferb.sanitize.redact import Redactor


class Desktop:
    """A mixin of the Daemon (ferb/daemon/main.py): its methods use the daemon's state."""

    def _record_disturbance(self, task_id: str, kind: str, data: dict[str, Any]) -> None:
        """Persist observations before broadcast; tool arguments stay in their referenced action span."""
        if self.spans is None:
            return
        task = self.tasks.get(task_id)
        span = self.spans.start(kind, task_id=task_id, parent_id=task.span_id if task else None,
                                input={"operation": "observe desktop"})
        self.spans.finish(span, output=data)
        if kind in ("disturbance.change", "disturbance.error"):
            self._publish("disturbance.observed", {"task_id": task_id, "span_id": span.id, **data})

    async def _task_end(self, task_id: str) -> list[dict[str, Any]]:
        resources = await self.hearing.end_task(task_id)          # L26: listening ends with the task
        resources.extend(await self.recordings.end_task(task_id))
        if (launched := getattr(getattr(self, "mac", None), "launched", None)) is not None:
            try:                                                  # L39: apps this task launched
                resources.extend(await launched.release(task_id))
            except Exception as exc:
                resources.append({'resource': 'app', 'released': False, 'error': f"{type(exc).__name__}: {exc}"})
        for name, drv in (("ferb_browser", self.identity_browser), ("browser", self.router)):
            if drv is not None:
                try:
                    resources.extend(await drv.release(task_id) or [])
                except Exception as exc:
                    resources.append({'resource': name, 'released': False, 'error': f"{type(exc).__name__}: {exc}"})
        if (kept := getattr(self, "downloads", None)) is not None:
            kept.forget_task(task_id)                    # after its tabs are let go of: their locks and unreported facts
        if self.shells is not None:
            try:
                await self.shells.end_task(task_id)
            except Exception as exc:
                resources.append({'resource': 'shell', 'released': False, 'error': f"{type(exc).__name__}: {exc}"})
        return resources

    async def _tree_finished(self, tree_id: str) -> None:
        """A task tree ended: the desktop observer's summary, then its change record's copies (L39) and FERB's upload
        copies for it go (rule 17), except those a tab that outlives the tree still shows. What couldn't be removed is
        journaled; the next start-up sweeps it."""
        try:
            await self.disturbance.finish(tree_id)
        finally:
            if self.changes is not None:                  # L39: the tree's copies from before its changes
                if stuck := await asyncio.to_thread(self.changes.forget, tree_id):
                    self._journal().append("changes.cleanup_failed", {"task_id": tree_id, "paths": len(stuck)})
            if self.uploads is not None:
                await self._clean_uploads(self.uploads.forget_tree, tree_id, about={"task_id": tree_id})

    def _owner_used_app(self, task_id: str, app: Any) -> bool:
        """L39: the desktop observer recorded the app coming forward in the task's tree after FERB launched it, with
        no FERB on-screen call near (launched.brought_forward)."""
        rec = self.tasks.get(task_id) if self.tasks is not None else None
        tree = rec.tree_id if rec is not None and rec.tree_id else task_id.split(".", 1)[0]
        return brought_forward(self.db, tree, app.pid, app.at)

    def _running_processes(self) -> list[dict[str, Any]]:
        """FERB-started processes running now (the `ferb ps` data), commands redacted (rule 14): a front-app or Space
        change's record names them (P26). Facts only; nothing is inferred about what they did."""
        if self.shells is None:
            return []
        return [{"id": p.id, "task": p.owner_task, "command": self._redact_text(p.command), "background": p.long_lived}
                for p in self.shells.list() if p.running]

    async def _native_page_secrets(self, pid: int, bounds: tuple[float, float, float, float], title: str) -> Any:
        """B6: for a task tab's capture whose page the extension couldn't read, the secret fields Accessibility finds in
        the page area of that browser process's window at `bounds` showing the tab titled `title`
        (`MacComputerDriver.page_secrets`). None: no Mac driver, or it can't tell (a failure is journaled by type; the
        capture then goes to text recognition)."""
        if self.mac is None:
            return None
        try:
            return await self.mac.page_secrets(pid, bounds, title)
        except Exception as e:
            from ferb.daemon.gateway import error_name
            self._journal().append("screen.page_secrets_failed", {"error": error_name(e)})
            return None

    async def _native_place(self, rect: tuple[float, float, float, float], viewport: tuple[float, float, float, float],
                            pid: int, bounds: tuple[float, float, float, float],
                            title: str) -> tuple[float, float, float, float] | None:
        """L33: a task tab's element in screen points for FERB's cursor, placed on the page area of the on-screen
        browser window showing the tab (`MacComputerDriver.place_on_web_area`); None without a Mac driver."""
        if self.mac is None:
            return None
        return await self.mac.place_on_web_area(rect, viewport, pid, bounds, title)

    async def _native_window_capture(self, pid: int, bounds: tuple[float, float, float, float],
                                     title: str) -> tuple[bytes | None, str]:
        """Round 3 (I1): for a task tab the debugger can't attach to, which is the one its window shows: that on-screen
        browser window (at `bounds`, carrying the tab's title `title`) captured natively, as mac.screenshot
        captures (masked from AX and the extension where they can read it, `Shot.masking`): (JPEG, note). Otherwise
        (None, why), in FERB's words (round 4, item 3): no Mac driver, screenshots switched off ([mac] snapshot_image),
        no such window on screen or more than one, the app excluded, or the capture failed (journaled by type)."""
        mac = self.mac
        if mac is None:
            return None, "FERB has no Mac driver to capture windows with"
        if not mac.snapshot_images():
            return None, "screenshots are switched off ([mac] snapshot_image)"
        from ferb.act.mac import screenshot
        try:
            near = [w for w in await asyncio.to_thread(screenshot.normal_windows)
                    if w.pid == pid and all(abs(a - b) <= 2 for a, b in zip(w.bounds, bounds))]
            if title:                          # the window showing the tab carries its title, even when it is the only
                near = [w for w in near if same_title(w.title, title)]   # one there (round 4: rule 5, the right window)
            if not near:
                return None, "no window of the browser showing that tab is on screen (it may be on another Space)"
            if len(near) > 1:
                return None, "more than one window of the browser at that place shows that tab's title"
            _, shot, jpeg = await mac.window_image(f"pid:{pid}", window=str(near[0].id))
            now = await asyncio.to_thread(screenshot.window, near[0].id)
            if title and (now is None or not same_title(now.title, title)):     # checked again (round 5): it may
                return None, "the window's page changed while it was captured (its title isn't the tab's now)"
            return jpeg, shot.masking
        except NoTarget:                       # gone between the listing and the capture
            return None, "the window went away before it could be captured"
        except Withheld as e:                  # its masks couldn't be painted: said once, by the caller
            return None, str(e).replace("; pixels withheld", "").removeprefix("pixels withheld: ")
        except Refused as e:                   # FERB's own words (an excluded app)
            return None, str(e)
        except Exception as e:
            from ferb.daemon.gateway import error_name
            self._journal().append("screen.native_capture_failed", {"error": error_name(e)})
            return None, f"the window capture failed ({error_name(e)})"

    async def _front_window(self, request_id: str, client_pid: int | None = None) -> Any:
        """L12: the owner's front window as this message arrived (ferb/sense/front_window.py): the topmost window that
        isn't FERB's (nor, while in front, that of the app hosting the CLI that sent it, `client_pid`), its redacted
        screenshot kept as evidence under the request; an app on the exclusion list is reported unnamed and unlooked
        at, a site on it under the excluded label. None: no Mac driver or evidence store, sensing paused (`ferb
        pause`), or nothing but FERB in front. A failure is journaled by type and never blocks the message."""
        from ferb.act.mac import accessibility, screenshot
        from ferb.daemon.gateway import error_name
        from ferb.sanitize.images import ImageRedactor
        from ferb.sense import front_window as fw
        mac, evidence = self.mac, self.evidence
        if mac is None or evidence is None or (self.sense is not None and self.sense.paused):
            return None
        exclusions, daemon_pid = self._exclusions(), os.getpid()

        async def shoot(w: Any) -> tuple[bytes, str, tuple[str, str] | None]:
            if not mac.snapshot_images():                    # the owner's `[mac] snapshot_image` switch
                raise Refused("screenshots are switched off ([mac] snapshot_image)")
            _, shot, jpeg = await mac.window_image(f"pid:{w.pid}", window=str(w.id))
            return jpeg, shot.masking, shot.tab

        async def keep(image: bytes, facts: dict[str, Any], origin: str) -> str:
            return await evidence.put_async(request_id, None, "front_window", image=image, data=facts, origin=origin)
        try:
            sender = await asyncio.to_thread(fw.sending_app, client_pid, fw.app_of, fw.parent_pid) \
                if client_pid else None
            sources = fw.Sources(
                windows=screenshot.normal_windows, app_of=fw.app_of,
                own=lambda pid, bundle: fw.ferb_process(pid, bundle, daemon_pid, fw.parent_pid),
                excluded=lambda bundle, name, url: exclusions.app(bundle, name) or exclusions.url(url),
                tab=self._front_tab, shoot=shoot, keep=keep, redact=self.redact, sender=sender,
                main_frame=accessibility.main_window_frame,
                redact_image=ImageRedactor(self.redact if isinstance(self.redact, Redactor) else Redactor()))
            return await fw.capture(sources)
        except Exception as e:
            self._journal().append("screen.capture_failed", {"request_id": request_id, "error": error_name(e)})
            return None

    def _front_tab(self, bundle_id: str, window: Any, after_shot: bool = False) -> tuple[str, str] | None:
        """The active tab (URL, title) of the browser window the front-window capture took (L12, `window`): every
        window of the browser read through AppleScript and the captured one picked by its place and title
        (`browser_tabs.window_tab`; LookupError when no single window is it: no URL rather than another window's).
        Only when the owner already granted Automation for that browser (the check never asks; while it is still
        answering the read is None, and the capture tries again after the shot). A kept GRANTED answer serves only that
        read after the shot, and only while a fresh check is still pending; it is dropped as soon as a check answers
        anything else (a permission reset mid-run must not lead to a prompt), and when a read fails."""
        from ferb.sense.watchers.browser_tabs import WINDOW_SCRIPTS, applescript_windows, window_tab
        if bundle_id not in WINDOW_SCRIPTS:
            return None
        if not self._automation_granted(bundle_id):
            probe = self._automation_checks.get(bundle_id)
            if probe is None or probe.done():          # a check answered something other than GRANTED
                self._tab_granted.discard(bundle_id)
                return None
            if not (after_shot and bundle_id in self._tab_granted):
                return None
        self._tab_granted.add(bundle_id)
        try:
            listed = applescript_windows(bundle_id)
        except (OSError, subprocess.SubprocessError):
            listed = None
        if listed is None:
            self._tab_granted.discard(bundle_id)
            return None
        found = window_tab(listed, window.bounds, window.title)
        return (found.url, found.title) if found.url else None

    async def _build_mac_and_shell(self) -> None:
        device = self.device_id
        def owner_wait(holder: str, on: bool) -> None:          # the owner's time never counts as a stall
            if self.supervisor is not None:
                self.supervisor.activity.waiting(holder, on)
        def available() -> bool:                                  # paused: no owner-input tap, so no real input
            return not (self.sense is not None and self.sense.paused)
        def owner_active() -> bool:                    # foreground input always yields when the owner provides input
            return self.owner_input.active()
        self.mac = MacComputerDriver(device, RealInput(owner_active, self.gui, on_wait=owner_wait,
                                                       available=available),
                                     excluded=self.config.exclude_apps, web_secrets=self._web_secrets,
                                     profiles=Profiles(profiles_dir(self.home)))   # what landed per app, learned
        self.mac.snapshot_images = lambda: bool(self.config.get("mac", "snapshot_image", True))   # image + tree
        mac = self.mac
        # L39: apps a task's calls launch are its resources, quit at its release unless the owner used them
        self.mac.launched = LaunchedApps(
            lambda: list(mac.apps.running()),
            lambda task, app: self._owner_used_app(task, app),
            now=lambda: self.clock.now().isoformat())      # FERB's clock, as the desktop records use (M7)
        # FERB's shell and AppleScript run only inside its sandbox, built from the protection lists (S2, D27, rule
        # 10): when sandbox-exec can't run, their tools are left out and `ferb doctor` says why, never uncontained (Q7)
        assert self.protection is not None
        # Its egress proxy is the shell's only way off the Mac (S2, D28): each process gets its own credentials, and
        # every destination is checked by the gateway (L2: any remote host for a tracked process) and recorded with its
        # byte counts (F5a Task 3). Every TLS hello's server name goes through the same check (S4 fronting: a tree's
        # No to a host can't be passed through a differing SNI).
        self._loop = asyncio.get_running_loop()
        self.proxy = EgressProxy(self._allow_destination, redactor=self.redact if isinstance(self.redact, Redactor)
                                 else None, excluded=getattr(self.redact, "excluded_host", None),
                                 inspect_sni=lambda proc, host, port: True,
                                 local_access=LocalAccess(self.protection),
                                 allow_local=self._allow_local_destination,
                                 on_destination=self._shell_destination)
        self.sandbox = Sandbox(self.protection, self.home / "run" / "sandbox",
                               lambda: self.proxy.port if self.proxy is not None else None, socket=self.server.path)
        self.sandbox.sweep()
        self.containment = await asyncio.to_thread(self.sandbox.available)   # runs sandbox-exec: off the loop
        contained = self.sandbox if self.containment[0] else None
        if contained is None:                                   # no shell, so no proxy for it
            self.proxy.close()
            self.proxy = None
        self.mac.ask_secret = lambda ctx, target, **kw: self.gateway.ask_secret(ctx, target, **kw)
        self.mac.release_secret = lambda value: self.gateway.release_secret(value)
        add_mac(self.registry, self.mac, self._device_profile, self.recordings, contained, self.protection)
        self.hearing.apps = self.mac.app                          # the Mac tools' exact app lookup (exclusions too)
        self.hearing.sweep()                                      # sound and answers a restart left behind (L26)
        add_hearing(self.registry, self.hearing,
                    lambda identity: self.identity_browser if identity is Identity.FERB else self.router)
        if contained is not None and self.protection.local_ports:   # pinned local ports: read the kernel's reports
            self.direct_reports = DirectReports(on_end=self._local_reports_stopped)
            self.direct_reports.start()
        self.shells = ShellManager(self.home / "logs" / "shell", self._redact_text,
                                   stream_factory=getattr(self.redact, "stream", None), sandbox=self.sandbox,
                                   proxy=self.proxy, on_network=self._shell_network,
                                   redactor=self.redact if isinstance(self.redact, Redactor) else None,
                                   on_escape=lambda task, grant: self._journal().append("escape.used", {
                                       "task_id": task, "rule": grant.rule, "command": grant.command,
                                       "cwd": grant.cwd, "confirmation": grant.confirmation_id}),
                                   reports=self.direct_reports, local_refused=self._local_refused)
        if contained is not None:
            add_shell(self.registry, self.shells, self.protection)
        else:
            self._journal().append("shell.unavailable", {"why": self.containment[1]})
        add_trail(self.registry, lambda: self.trail)
        add_evidence(self.registry, self.spans, self._inspection_parent,
                     lambda task_id: (rec.tree_id if (rec := self.tasks.get(task_id)) else None), self.evidence,
                     lambda: getattr(self.redact, "secret_values", set)())   # a page never splits one (rule 14)
        add_time(self.registry, lambda: self.timers, self.clock.now, lambda: self._sleep_until,
                 lambda task_id: (rec.task.deadline if (rec := self.tasks.get(task_id)) else None))

    def _automation_granted(self, bundle_id: str) -> bool:
        """Already granted by the owner; never asks (the prompt would appear out of nowhere)."""
        import time as _t
        from ferb.device import permissions
        hit = self._automation_cache.get(bundle_id)
        if hit and _t.monotonic() - hit[0] < 60:
            return hit[1]
        probe = self._automation_checks.get(bundle_id)
        if probe is None:
            probe = self._automation_checks[bundle_id] = permissions.automation_check(bundle_id, ask=False)
        if not probe.done():
            return False  # watcher skips this sample; a stuck native call must not hold its executor thread
        self._automation_checks.pop(bundle_id, None)
        ok = probe.result() is permissions.Status.GRANTED
        self._automation_cache[bundle_id] = (_t.monotonic(), ok)
        return ok

    def _app_changed(self, change: str, info: Any, path: str) -> None:
        """An app appeared or disappeared: rescan that app only (Info.plist) into the device profile, in a worker
        thread (the profile is a file read and written whole), then tell clients."""
        self._spawn(self._app_changed_off_loop(change, info, path), "device profile update")

    async def _app_changed_off_loop(self, change: str, info: Any, path: str) -> None:
        from ferb.device.profile import ProfileStore
        did = str(self.config.get("device", "id", "") or "")
        home = self.home

        def update() -> None:
            store = ProfileStore(home)
            profile = store.load(did) if did else None
            if profile is not None:
                apps = [x for x in profile.apps if x.path != path and (info is None or x.bundle_id != info.bundle_id)]
                if info is not None:
                    apps.append(info)
                profile.apps = sorted(apps, key=lambda x: x.bundle_id)
                store.save(profile, f"app {change}: {info.bundle_id if info else Path(path).name}")
        await asyncio.to_thread(update)
        self._publish("device.changed", {"change": change, "path": path,
                                         "bundle_id": info.bundle_id if info else None})

    async def _app_window(self, app: str, scopes: frozenset[str]) -> Any:
        """The gateway's first look at a Mac app a worker started on (D26, D29): one background picture of its
        window, or None (no Mac driver, or nothing to take under mac.snapshot's image rules)."""
        if self.mac is None:
            return None
        from ferb.act.mac.tools import app_window
        return await app_window(self.mac, app, scopes)

    def _permissions(self) -> dict[str, bool]:
        """The daemon's own macOS permissions (the process that acts). Replaceable in tests."""
        from ferb.device import permissions as perm
        try:
            return {"accessibility": perm.accessibility() is perm.Status.GRANTED,
                    "screen": perm.screen_recording() is perm.Status.GRANTED}
        except Exception:
            return {"accessibility": True, "screen": True}              # can't tell: plan validation still guards

    def _installed_apps(self) -> list[tuple[str, str]]:
        """(bundle id, name) of the Mac's apps, for matching the names the owner uses."""
        p = self._device_profile()
        return [(a.bundle_id, a.name) for a in p.apps] if p is not None else []

    def _running_apps(self) -> list[tuple[str, str, int]]:
        """(bundle id, name, pid) of the apps running now; none when they can't be read."""
        from ferb.act.mac import apps as appsm
        try:
            return [(a.bundle_id, a.name, a.pid) for a in appsm.running()]
        except Exception:
            return []

    def _device_profile(self) -> Any:
        from ferb.device.profile import ProfileStore
        did = self.config.get("device", "id")
        return ProfileStore(self.home).load(str(did)) if did else None
