"""Own the browser drivers' wiring: the extension hub and router, FERB's own browser, pairing, handoffs, uploads and the
browser RPCs. Must not choose a profile for a task: agents do."""

import asyncio
import json
from pathlib import Path
from typing import Any, AsyncIterator, Awaitable, Callable

from ferb.act.browser import downloads, profiles as browser_profiles, uploads
from ferb.act.browser.autoinstall import accepted_extension_ids
from ferb.act.browser.ext_bridge import FerbIdentity, Hub, Router
from ferb.act.browser.pairing import Pairings
from ferb.act.browser.resolver import Resolver
from ferb.act.browser.tools import FERB_BROWSER, IdentityDriver
from ferb.act.mac.driver import WebSecrets, same_title
from ferb.core import origin as origin_
from ferb.core.errors import FerbError
from ferb.core.types import Identity
from ferb.daemon.server import Session
from ferb.registry import add_identity_browser, add_owner_browsers

# DMN-507: what a browser call says while FERB may not look at other apps' data yet
WAITS_FOR_FULL_DISK_ACCESS = ("FERB can't look at your browsers until it has Full Disk Access (System Settings → "
                              "Privacy & Security → Full Disk Access → FERB)")
# HAND-407: why browser tools see only the paired profiles, by folder, while that is so
PROFILES_UNSEEN = ("FERB can't read browser profile names without Full Disk Access (System Settings → Privacy & "
                   "Security → Full Disk Access → FERB), so only the profiles already paired with FERB are known, by "
                   "their folder")
REFUSED = "browser.connection_refused"      # HAND-406: a browser connection FERB didn't take, with its reason
HELPER_FAILED = "secret.helper_failed"      # HAND-410: a secret-helper failure, with its status when known
# item 7 / HAND-413: Pair all's skip reasons FERB's Web Store page fixes in one click (Add to <browser>)
ADDABLE = frozenset({"removed", "not_added", "requested"})
# HAND-412: a pairing page nobody answered while the profile's files say FERB is on and new enough. Chrome shows the
# page as blocked (ERR_BLOCKED_BY_CLIENT) when the extension isn't running in that profile right then: turned off since
# the files were written, updating, or waiting for the owner to accept new permissions
PAIR_UNANSWERED = ("FERB's pairing page got no answer there: {label} may have FERB turned off, updating or waiting "
                   "for you to accept new permissions in that profile. Open chrome://extensions in that profile, make "
                   "sure FERB is on and up to date, then try again")


class Browsers:
    """A mixin of the Daemon (ferb/daemon/main.py): its methods use the daemon's state."""

    def use_identity_browser(self, driver: IdentityDriver) -> None:
        """Attach FERB's own browser: registers the browser.* tools; its tabs close with their task and on stop."""
        self.identity_browser = driver
        if hasattr(driver, "native_secrets"):          # B6: an unread tab's fields from Accessibility, as the router's
            driver.native_secrets = self._native_page_secrets
        if hasattr(driver, "native_capture"):          # I1: a tab the debugger can't attach to, as the router's
            driver.native_capture = self._native_window_capture
        if hasattr(driver, "native_place"):            # L33: FERB's cursor, as the router's
            driver.native_place = self._native_place
        from ferb.identity.helper import seal as helper_seal
        driver.seal_value = lambda key, value, command, origin: helper_seal(self.home, key, value, command, origin)
        add_identity_browser(self.registry, driver, self.recordings)

    def _detach_identity_browser(self) -> None:
        """FERB's account removed: its `ferb_browser.*` tools go and the tabs it opened close (no task can release them
        through it any more). Choosing and pairing a profile again attaches it (`_attach_identity_if_paired`)."""
        driver, self.identity_browser = self.identity_browser, None
        if driver is None:
            return
        for name in [n for n in self.registry.capabilities if n.startswith(f"{FERB_BROWSER.prefix}.")]:
            del self.registry.capabilities[name]
        self._spawn(driver.shutdown(), "identity cleared: browser")

    def _tab_closed(self, br: Any, tab_id: int) -> Awaitable[None] | None:
        """A tab closed in a paired browser: the upload copies it showed need not stay (rule 17). Those of a tree that
        already ended go now, in the background."""
        if self.uploads is None or not self.uploads.closed(uploads.tab_key(br.name, tab_id)):
            return None
        return self._clean_uploads(self.uploads.collect)

    async def _clean_uploads(self, step: Callable[..., int], *args: Any, about: dict[str, Any] | None = None) -> None:
        """One upload-copy cleanup in a worker thread; what it couldn't remove is journaled (paths, never contents)."""
        try:
            await asyncio.to_thread(step, *args)
        except OSError as e:
            self._journal().append("browser.uploads_cleanup_failed",
                                   {**(about or {}), "error": f"{type(e).__name__}: {e}",
                                    "left": list(getattr(e, "left", []))})

    def _stop_identity_browser(self) -> None:
        self._spawn(self.hearing.shutdown(), "stop: hearing")
        self._spawn(self.recordings.shutdown(), "stop: recordings")
        for drv in (self.identity_browser, self.router):
            if drv is not None:
                self._spawn(drv.shutdown(), "stop: browser")
        if self.shells is not None:
            self._spawn(self.shells.kill_all(), "stop: shell")   # kills task-owned processes

    def _unpaired_note(self, browser: str) -> str | None:
        """The FERB extension is installed and connected in this browser but not paired."""
        if self.hub is None or browser not in self.hub.unpaired.values() or self._owner_extension_connected(browser):
            return None
        name = {"brave": "Brave", "chrome": "Chrome"}.get(browser, browser)
        return (f"The FERB extension is installed in {name} but not paired: a task pairs the profile it needs with "
                "browser_pair (FERB asks the owner once); the owner can also pair it in Settings → Browsers.")

    def _owner_extension_connected(self, browser: str) -> bool:
        """A paired owner profile of this browser is connected: browser work goes through the extension."""
        return self.hub is not None and any(br.conn is not None and br.binding.browser == browser
                                            and br.binding.identity is Identity.OWNER for br in self.hub.bridges.values())

    async def _web_secrets(self, browser: str, bounds: tuple[float, float, float, float],
                           title: str = "") -> WebSecrets | None:
        """For a native capture of a browser window (L13): ask every connected profile of this browser, together, for
        the mask state of the active tab of its (not minimized) window at these bounds (with the captured window's
        `title`, when several of its windows are there, protocol 15), and that tab. Exactly one profile must own
        such a window, and its answer is the WebSecrets. Otherwise nobody could tell, and the WebSecrets has no page
        and says why (no paired profile connected, none or two of a profile's windows there, more than one profile,
        no answer), for the capture's note (fix wave 4: the live case's "neither read" couldn't be explained)."""
        async def ask(br: Any) -> Any:
            try:
                return await br.call("window_secrets", {"bounds": list(bounds), "title": title}, timeout=4)
            except FerbError:
                return None
        bridges = [br for br in (self.hub.bridges.values() if self.hub else []) if br.conn is not None
                   and br.binding.browser == browser]
        if not bridges:
            return WebSecrets(None, f"no paired profile of {browser} is connected")
        answers = await asyncio.gather(*(ask(br) for br in bridges))
        owners = [r for r in answers if isinstance(r, dict) and r.get("matched")]
        if len(owners) > 1 and title:          # each profile's own window there: the one showing the captured page
            owners = [r for r in owners if isinstance(r.get("tab"), dict) and isinstance(r["tab"].get("title"), str)
                      and same_title(r["tab"]["title"], title)] or owners      # (two maximized profiles, m4)
        if len(owners) == 1:
            return WebSecrets.from_reply(owners[0])
        if owners:
            return WebSecrets(None, "more than one paired profile has a window at this window's place")
        if any(isinstance(r, dict) and r.get("reason") == "ambiguous_window" for r in answers):
            return WebSecrets(None, "a paired profile has more than one window at this window's place")
        if any(not isinstance(r, dict) for r in answers):      # timed out or failed: the driver won't ask again
            return WebSecrets(None, "a paired profile didn't answer", late=True)
        return WebSecrets(None, "no paired profile has a window at this window's place (another profile, or a "
                                "private window)")

    def _profile_state(self, p: Any, extension: str | None = None) -> str:
        """connected / paired (FERB opens it) / installed (turned on, not paired) / off (installed, turned off: its
        pages are blocked, so it can't pair until the owner turns it on) / missing, or removed when `extension` says so
        (HAND-449: the owner removed FERB's store copy there). `extension`: the profile's `extension_state`, when its
        files were already read (the environment reads them in a thread)."""
        from ferb.act.browser.profiles import extension_state
        b = self.pairings.for_profile(p.browser, p.folder) if self.pairings else None
        if b is not None:
            br = self.hub.bridges.get((p.browser, p.folder)) if self.hub else None
            return "connected" if br is not None and br.conn else "paired"
        if extension is None:
            data = (self.browser_data_dirs or {}).get(p.browser)
            extension = extension_state(p, frozenset(accepted_extension_ids(self.config)), data)
        return {"on": "installed", "off": "off", "removed": "removed"}.get(extension, "missing")

    def _other_app_data(self) -> bool:
        """DMN-507: whether FERB may read or write another app's data (browser folders and profiles) now: only once
        FERB has Full Disk Access (the daemon's one gate, `data_access`). Every browser-folder read below goes
        through it."""
        return bool(self.data_access())

    def _profiles(self) -> list[browser_profiles.BrowserProfile]:
        if not self._other_app_data():
            return []
        return browser_profiles.discover_all(self._browser_data())

    def _fresh_profiles(self) -> list[Any]:
        """The browser profiles as last discovered (`_fresh`): what the planner's facts and availability name."""
        dirs = tuple(sorted((k, str(v)) for k, v in (self.browser_data_dirs or {}).items()))
        return list(self._fresh.get(self._profiles, dirs))

    def _owner_browsing_profile(self) -> tuple[str, str] | None:
        """The owner's profile FERB browses in while it has no profile of its own (`browsing.owner_profile`, chosen in
        FERB.app; APP-1), else None."""
        browser, _, folder = str(self.config.get("browsing", "owner_profile", "") or "").partition("/")
        return (browser, folder) if browser and folder else None

    def _browsing_profiles(self) -> list[Any]:
        """The owner's profiles FERB knows now (discovered, plus paired ones it can't read: HAND-407), without FERB's
        own: what the browsing question offers."""
        ferb = self._identity_profile()
        paired = [(b.browser, b.folder) for b in (self.pairings.all() if self.pairings else [])]
        return [p for p in browser_profiles.with_paired(self._fresh_profiles(), paired)
                if (p.browser, p.folder) != ferb]

    def _browsing(self, attended: bool, request_id: str) -> dict[str, Any]:
        """APP-777 (refining APP-1): where a task FERB runs as itself browses. FERB's own profile set up: as before
        ({}). Else the owner's pick (`browsing.owner_profile`) while it is still there: as the owner ({"as_owner",
        "label"}). Else the owner chose None ({"declined"}). Else, asked once in FERB's lifetime: with the owner here,
        FERB's one question ({"ask": its id}, the open one when there is one); a run nobody attends goes without a
        browser and says why ({"note"}). Profiles FERB can't see at all leave nothing to offer ({})."""
        from ferb.orchestrator import browsing
        if self._identity_profile() is not None:
            return {}
        chosen, known = self._owner_browsing_profile(), self._browsing_profiles()
        if chosen is not None and (not known or any((p.browser, p.folder) == chosen for p in known)):
            return {"as_owner": True, "label": browser_profiles.shown(browser_profiles.named(known, *chosen))}
        gone = browser_profiles.shown(browser_profiles.named(known, *chosen)) if chosen is not None else ""
        if not gone and self.config.get("browsing", browsing.ASKED, False):
            return {"declined": True}
        if not known:
            return {}
        if not attended:
            return {"note": browsing.GONE_RUN.format(label=gone) if gone else browsing.NO_BROWSER_RUN}
        for approval_id, (kind, _) in self._owner_followups.items():
            ap = self.state.approvals.get(approval_id)
            if kind == browsing.KIND and ap is not None and ap.state == "pending":
                return {"ask": approval_id}             # one question at a time, whichever task or schedule asked
        options = {browser_profiles.shown(p): f"{p.browser}/{p.folder}" for p in known}
        reason = browsing.QUESTION_TITLE + "\n" + (browsing.GONE.format(label=gone) if gone else "") + \
            browsing.QUESTION_DETAIL
        return {"ask": self._ask_owner(browsing.KIND, json.dumps({"options": options}), reason,
                                       f"browsing-{self.clock.now().strftime('%Y%m%dT%H%M%S')}",
                                       options=[*options, browsing.NONE])}

    def _browsing_answered(self, ap: Any, ident: str) -> str:
        """The owner's answer to the browsing question: a profile is stored as `browsing.owner_profile` through the
        settings path and used from now on; None stores no profile; either way FERB never asks again unless that
        profile goes. Skipped or set aside: nothing is stored, and the next task that needs it asks. A line for the
        owner."""
        from ferb.daemon import settings
        from ferb.orchestrator import browsing
        if ap.state != "granted" or not ap.choice:
            return ""
        try:
            options = dict(json.loads(ident).get("options") or {})
        except (TypeError, ValueError, AttributeError):
            options = {}
        if ap.choice == browsing.NONE:
            profile, line = "", browsing.NONE_CHOSEN
        elif ap.choice in options:
            profile, line = str(options[ap.choice]), browsing.CHOSEN.format(label=ap.choice)
        else:
            return ""
        settings.write(self.config, "browsing.owner_profile", profile)
        self.config.set("browsing", browsing.ASKED, True)
        self._journal().append("browsing.chosen", {"profile": profile}, origin=origin_.OWNER)
        return line

    def _identity_profile(self) -> tuple[str, str] | None:
        folder = self.config.get("identity", "profile")
        return (str(self.config.get("identity", "browser", "chrome")), str(folder)) if folder else None

    async def _build_browsers(self) -> None:
        assert self.db and self.ledger
        self.pairings = Pairings(self.db, self.clock)
        self.hub = Hub(self.pairings, self.ledger, self._publish, lambda: self.device_id)
        # HAND-406: each refusal journaled, the first at once and then counted per reason and profile (`_Repeats`)
        self.hub.record_refusal = lambda facts: self._repeats.record(
            REFUSED, (REFUSED, str(facts.get("reason")), str(facts.get("profile") or facts.get("app") or "")), facts)
        self.server.on_native_refused = lambda why, origin: self.hub.note_refusal(  # type: ignore[union-attr]
            "host", why, origin=origin)
        resolver = Resolver(self._profiles, self.pairings.all, self._identity_profile,
                            owner_default=self._owner_browsing_profile,
                            unseen=lambda: None if self._other_app_data() else PROFILES_UNSEEN)
        await self.wake.start()
        self.router = Router(self.hub, resolver, self._profiles, self.wake, self.browser_launch)
        self.router.native_secrets = self._native_page_secrets      # B6: an unread tab's fields from Accessibility
        self.router.native_capture = self._native_window_capture    # I1: a tab the debugger can't attach to
        self.router.native_place = self._native_place                # L33: where an element is, for FERB's cursor
        # D35: a download in a task tab becomes a file in the workspace plus facts, never a dialog.
        self.downloads = downloads.Downloads(self.home / "workspace" / downloads.FOLDER, self.db, self.clock.now,
                                             self.config.get, lambda text: str(self.redact({"u": text})["u"]),
                                             self._publish)
        self.downloads.on_over_cap = self._downloads_cap     # HAND-211: over the cap, the owner is asked
        for name, step in (("sweep", self.downloads.sweep), ("finalize_stale", self.downloads.finalize_stale)):
            try:                                         # what a restart cut off, each step on its own
                step()
            except Exception as e:
                self._journal().append("browser.downloads_startup_failed",
                                       {"step": name, "error": f"{type(e).__name__}: {e}"})
        self.downloads.task_root = self._task_root
        self.hub.on_download = self.downloads.receive
        self.hub.on_tab_closed = self._tab_closed
        # Rule 17: a file given to a browser to upload is FERB's own copy, never one in a folder macOS protects.
        self.uploads = uploads.Uploads(self.home / "workspace" / uploads.FOLDER,
                                       lambda: self._gateway().roots.untrusted_roots)
        try:
            await asyncio.to_thread(self.uploads.sweep)  # copies a restart left behind belong to no task now
        except OSError as e:
            self._journal().append("browser.uploads_startup_failed", {"error": f"{type(e).__name__}: {e}",
                                                                      "left": list(getattr(e, "left", []))})
        from ferb.identity import helper as secret_helper
        from ferb.identity.helper import seal as helper_seal
        # HAND-410: each secret-helper failure journaled with what is known of why (the helper runs in worker threads)
        loop = asyncio.get_running_loop()
        secret_helper.on_failure = lambda facts: loop.call_soon_threadsafe(
            self._repeats.record, HELPER_FAILED,
            (HELPER_FAILED, str(facts.get("operation")), str(facts.get("status", facts.get("found")))), facts)
        self.router.seal_value = lambda key, value, command, origin: helper_seal(self.home, key, value, command, origin)
        self.router.ask_code = lambda ctx, origin: self.gateway.ask_login_code(ctx, origin)
        self.router.ask_secret = lambda ctx, target, **kw: self.gateway.ask_secret(ctx, target, **kw)
        self.router.pair_profile = self._pair_for_task       # browser_pair, after the owner's Yes (2026-10-27)
        self.router.handoff = self._handoff                  # browser_handoff: a human check for the owner (2026-10-27)
        add_owner_browsers(self.registry, self.router, self.recordings)
        pairings = self.pairings
        self.ferb_identity = FerbIdentity(self.hub, self._identity_profile, self.secrets,
                                          lambda: str(self.config.get("mailbox", "address", "") or ""),
                                          lambda install_id: pairings.key(install_id), self._profiles,
                                          self.wake, self.browser_launch)
        for drv in (self.router, self.ferb_identity):
            drv.inspection_parent = self._inspection_parent
            drv.downloads = self.downloads
            drv.uploads = self.uploads
            drv.task_running = lambda task_id: self.tasks.get(task_id) is not None   # removed when its tree ends
            # L31: between a loop's rounds, the owner's tab it holds is theirs again (downloads, new tabs)
            drv.task_resting = lambda task_id: self.supervisor is not None and self.supervisor.loop_resting(task_id)
            drv.task_root = self._task_root
            drv.max_tabs = lambda: int(self.config.get("browser", "task_max_tabs", 50))
            drv.redact_text = lambda text: str(self.redact({"u": text})["u"])
            # HAND-201: the task's tab group is labelled with its request's first words (group_title); a paused
            # task keeps its group (ORCH-308: nothing of it is released). "Keep the tabs open" (keep_tabs_open)
            # waits for the planner's wiring (step 8c).
            drv.task_title = self._task_title
            drv.task_paused = lambda task_id: self.supervisor is not None and self.supervisor.task_paused(task_id)
        self.downloads.task_tab = self.hub.task_tab      # only a live task's tab holds downloads (D35)
        # Browser/profile selection is an agent decision, not a phrase-derived permission gate.
        self._attach_identity_if_paired()
        self.server.native = self.hub.serve
        # the pinned unpacked id and, once configured, the Chrome Web Store id (L25)
        self.server.extension_ids = lambda: frozenset(accepted_extension_ids(self.config))

    def _task_title(self, root: str) -> str:
        """A task tree's goal, for its tab group's label (the driver shortens and redacts it)."""
        rec = self.tasks.get(root)
        return rec.task.goal if rec is not None else ""

    def _identity_paired(self) -> bool:
        """FERB's own profile is paired as FERB (read only)."""
        prof = self._identity_profile()
        b = self.pairings.for_profile(*prof) if (prof and self.pairings) else None
        return b is not None and b.identity is Identity.FERB

    def _attach_identity_if_paired(self) -> bool:
        """FERB's tools exist only once its identity profile is paired as FERB."""
        prof = self._identity_profile()
        b = self.pairings.for_profile(*prof) if (prof and self.pairings) else None
        if b is None or b.identity is not Identity.FERB or self.ferb_identity is None:
            return False
        if self.identity_browser is None:
            self.use_identity_browser(self.ferb_identity)
        return True

    async def _identity_pair(self, session: Session, params: dict[str, Any]) -> dict[str, Any]:
        """Pair FERB's identity profile as identity FERB: its Chrome accounts must be exactly FERB's;
        re-pairing needs the owner's confirmation."""
        from ferb.identity import seed
        assert self.pairings
        prof, address = self._identity_profile(), str(self.config.get("mailbox", "address", "") or "")
        if not prof or not address:
            raise FerbError("FERB's identity profile isn't chosen yet")
        if not self._other_app_data():
            raise FerbError(WAITS_FOR_FULL_DISK_ACCESS)
        data = self._browser_data().get(prof[0])
        try:
            await asyncio.to_thread(seed.verify, address, prof[1], *([data] if data else []))
        except seed.SeedError as e:
            raise FerbError(f"refusing to pair: {e}") from None
        current = self.pairings.for_profile(*prof)
        if current is not None and not params.get("confirm"):
            raise FerbError("FERB's identity profile is already paired; re-pairing needs your confirmation")
        # HAND-449: no pairing page where FERB's copy is removed, missing, off or too old (Chrome blocks it there); the
        # owner's step instead, as Pair all says it
        found = next((p for p in await asyncio.to_thread(self._profiles) if (p.browser, p.folder) == prof), None)
        if found is not None and (stands := await asyncio.to_thread(self._extension_stands, found)) is not None:
            raise FerbError(f"FERB's own profile can't be paired yet: {stands[1]}")
        before = self.hub.bridges.get(prof) if self.hub else None
        hellos0 = before.hellos if before is not None else 0
        token = self.pairings.issue_token(prof[0], prof[1], Identity.FERB)
        wake = await self._open_pairing(prof[0], prof[1], token)
        end = self.clock.monotonic() + float(params.get("timeout_s", 60))
        while self.clock.monotonic() < end:
            b = self.pairings.for_profile(*prof)
            br = self.hub.bridges.get(prof) if self.hub else None
            # its own token and a connection attached after it (not a changed binding: re-pairing the same install
            # gives an identical one, and its bridge may still hold the connection from before)
            if token in self.pairings.paired and b is not None and b.identity is Identity.FERB and br is not None \
                    and br.conn and (br is not before or br.hellos > hellos0):
                await self._close_wake(br, wake)
                self._attach_identity_if_paired()
                return {"paired": True, "profile": prof[1]}
            await asyncio.sleep(0.2)
        self.pairings.revoke(token)                          # its page went unanswered: the key can't pair later
        return {"paired": False, "profile": prof[1], "hint": "is the FERB extension loaded in FERB's profile?"}

    def _browser_data(self) -> dict[str, Path]:
        """Each installed browser's user-data folder (tests give their own); none before Full Disk Access (DMN-507)."""
        if not self._other_app_data():
            return {}
        if self.browser_data_dirs is not None:          # only browsers whose app is installed (HAND-414)
            return browser_profiles.only_installed(self.browser_data_dirs)
        return {b: browser_profiles.data_dir(b, self.browser_support)
                for b in browser_profiles.installed(self.browser_support)}

    def _identity_candidates(self, address: str) -> list[dict[str, Any]]:
        """Every browser profile signed into `address` (Local State and Preferences, read only; never cookies), and
        whether it holds that account alone, as FERB's profile must (`seed`)."""
        from ferb.identity import seed
        out: list[dict[str, Any]] = []
        for browser, data in self._browser_data().items():
            try:
                found = seed.profiles(data)
            except seed.SeedError:
                continue
            signed = {p.folder: p.email for p in browser_profiles.discover(browser, data)}     # HAND-424
            for p in found:
                if address in p.accounts:
                    out.append({"browser": browser, "browser_name": browser_profiles.BROWSERS[browser].label
                                if browser in browser_profiles.BROWSERS else browser,
                                "folder": p.folder, "name": p.name, "email": signed.get(p.folder, ""),
                                "only": p.accounts == (address,), "other_accounts": len(p.accounts) - 1})
        return out

    async def _identity_find(self, session: Session, params: dict[str, Any]) -> dict[str, Any]:
        """FERB.app's "Give FERB its own Google account" (APP-1; auth.APP_ONLY): FERB's account as set now
        and whether its profile is paired, and, for `address` (the account the owner made), each profile signed into
        it. The owner made the account and signed it in; FERB only looks."""
        address = str(params.get("address") or self.config.get("mailbox", "address", "") or "").strip().lower()
        prof, owner = self._identity_profile(), self._owner_browsing_profile()
        found = await asyncio.to_thread(self._profiles) if prof or owner else []
        mine = browser_profiles.named(found, *prof) if prof else None
        theirs = browser_profiles.named(found, *owner) if owner else None
        # HAND-424: each profile's name and the account it is signed in to, read fresh (folder names and no account
        # without Full Disk Access)
        current = {"address": str(self.config.get("mailbox", "address", "") or ""),
                   "browser": prof[0] if prof else "", "profile": prof[1] if prof else "",
                   "profile_name": mine.display_name if mine else "", "profile_email": mine.email if mine else "",
                   "paired": self._identity_paired(),
                   "owner_profile": str(self.config.get("browsing", "owner_profile", "") or ""),
                   "owner_profile_name": theirs.display_name if theirs else "",
                   "owner_profile_email": theirs.email if theirs else ""}
        candidates = await asyncio.to_thread(self._identity_candidates, address) if address else []
        return {"current": current, "address": address, "candidates": candidates}

    async def _identity_choose(self, session: Session, params: dict[str, Any]) -> dict[str, Any]:
        """The owner's pick of FERB's profile (auth.APP_ONLY): written through the settings path
        (`settings.write_identity`) only when that profile holds exactly that account (`seed.verify`); `clear` removes
        FERB's account and detaches its browser (its pairing stays in Browsers until unpaired). Journaled as a settings
        change."""
        from ferb.daemon import settings
        from ferb.identity import seed
        if params.get("clear") is True:
            stored = settings.write_identity(self.config, "", "", "")
            self._detach_identity_browser()
        else:
            address = str(params.get("address") or "").strip().lower()
            browser, folder = str(params.get("browser") or ""), str(params.get("folder") or "")
            if not self._other_app_data():
                raise FerbError(WAITS_FOR_FULL_DISK_ACCESS)
            data = self._browser_data().get(browser)
            if data is None:
                raise FerbError(f"{browser or 'That browser'} isn't installed")
            try:
                await asyncio.to_thread(seed.verify, address, folder, data)
            except seed.SeedError as e:
                raise FerbError(str(e)) from None
            stored = settings.write_identity(self.config, address, browser, folder)
        self._journal().append("settings.changed", {"key": "identity", "value": stored}, origin="owner")
        return stored

    async def _open_pairing(self, browser: str, folder: str, token: str) -> str | None:
        """Open the extension's pairing page in that profile. A profile with no window isn't loaded, and a pairing
        page handed to it can go unanswered: load it first with a blank local page (closed after pairing)."""
        known = self._profiles()
        kwargs: dict[str, Any] = {"launch": self.browser_launch} if self.browser_launch else {}
        wake = None
        br = self.hub.bridges.get((browser, folder)) if self.hub else None
        if br is None or br.conn is None:
            wake = self.wake.url()
            browser_profiles.open_known_profile(browser, folder, wake, known, background=False, **kwargs)
            await asyncio.sleep(self.pair_load_wait_s)
        ids = accepted_extension_ids(self.config)          # the copy installed in that profile: unpacked or store
        p = next((p for p in known if (p.browser, p.folder) == (browser, folder)), None)
        found = browser_profiles.extension_entry(p, ids, (self.browser_data_dirs or {}).get(browser)) if p else None
        ext = found[0] if found else ids[0]
        browser_profiles.open_known_profile(browser, folder, f"chrome-extension://{ext}/pair.html#token={token}",
                                            known, background=False, **kwargs)   # the owner must see this page
        return wake

    @staticmethod
    async def _close_wake(br: Any, wake: str | None) -> None:
        if wake:
            try:
                await br.call("wake_close", {"url": wake}, timeout=5)
            except FerbError:
                pass

    async def _browsers_list(self, session: Session, params: dict[str, Any]) -> dict[str, Any]:
        assert self.pairings and self.hub
        ferb = self._identity_profile()
        paired = {(b.browser, b.folder): b for b in self.pairings.all()}
        out = []
        for p in browser_profiles.with_paired(self._profiles(), list(paired)):      # HAND-407
            b = paired.get((p.browser, p.folder))
            br = self.hub.bridges.get((p.browser, p.folder))
            connected = bool(br and br.conn)
            out.append({"browser": p.browser, "folder": p.folder, "name": p.display_name, "email": p.email,
                        "ferb_identity": (p.browser, p.folder) == ferb,
                        "paired": b is not None, "identity": b.identity.value if b else None,
                        "connected": connected, "version": br.version if br and br.conn else "",
                        "last_version": (br.version if br and br.version else "")
                        or (self.pairings.version(b.install_id) if b else ""),
                        # HAND-406: why it can't connect (this run's refusal, or a recorded version that's too old)
                        "refused": self.hub.problem(b) if b and not connected else None})
        unpaired = list(self.hub.unpaired.values())
        return {"profiles": out, "extension_ids": sorted(self.server.extension_ids()),
                "unpaired_connections": {browser: unpaired.count(browser) for browser in sorted(set(unpaired))}}

    async def _browsers_pair(self, session: Session, params: dict[str, Any]) -> dict[str, Any]:
        """Pair one profile (`ferb browsers pair`): by its exact browser and folder when given (several profiles can
        share a name, live 2026-10-27), else by name. A profile without the extension opens its extensions page."""
        assert self.pairings
        p = self._exact_profile(params) or self._named_profile(params)
        if (p.browser, p.folder) == self._identity_profile():
            raise FerbError(f"{browser_profiles.shown(p)} is FERB's own profile; it's paired as FERB, not as yours")
        if self._profile_state(p) == "missing":                 # the extension first (owner's report 2026-10-01)
            from ferb.device.extensions import extension_path
            browser_profiles.open_known_profile(p.browser, p.folder, "chrome://extensions", self._profiles(),
                                                background=False)   # the owner does the clicks: they must see it
            return {"paired": False, "needs_install": True, "browser": p.browser, "folder": p.folder,
                    "name": p.display_name, "email": p.email, "extension_dir": str(extension_path(self.home))}
        return await self._pair_one(p, float(params.get("timeout_s", 60)))

    def _named_profile(self, params: dict[str, Any]) -> Any:
        known = self._profiles()
        name, browser = str(params.get("profile", "")), params.get("browser")
        r = browser_profiles.resolve(name, known, browser=browser)
        if r.kind != "one":
            raise FerbError(r.message(name, known))
        return r.candidates[0]

    def _exact_profile(self, params: dict[str, Any]) -> Any:
        """The discovered profile with exactly that browser and folder, or None when the params name no folder."""
        browser, folder = str(params.get("browser") or ""), str(params.get("folder") or "")
        if not folder:
            return None
        p = next((p for p in self._profiles() if (p.browser, p.folder) == (browser, folder)), None)
        if p is None:
            raise FerbError(f"no {browser} profile in folder {folder!r} on this Mac")
        return p

    async def _pair_one(self, p: Any, timeout_s: float) -> dict[str, Any]:
        """Pair one profile: the extension's own pairing page, with a one-time token, opened in that profile (the page
        closes itself); done once that token is used and the profile's extension is connected. The owner's click
        (Pair, Pair all) or Yes (a task's browser_pair) is what authorizes it (owner 2026-10-27, replacing automatic
        pairing). A page left unanswered revokes its token."""
        assert self.pairings
        # the account it is signed in to the browser with, read fresh, for the owner's view (HAND-424); the journal
        # entries below name only the browser and folder
        base = {"browser": p.browser, "folder": p.folder, "name": p.display_name, "email": p.email}
        if (p.browser, p.folder) == self._identity_profile():
            return base | {"paired": False, "why": "FERB's own profile is paired as FERB"}
        state = self._profile_state(p)
        # HAND-449: a copy that isn't there (removed by the owner: the browser won't add it again; not added; a request
        # waiting for the browser's restart), is off or too old gets Pair all's own reason and the owner's step
        # (`not_paired_step`, one source for every route), and no pairing page: Chrome shows it as blocked there
        flag = {"missing": {"needs_install": True}, "off": {"needs_enable": True}}.get(state, {})
        if (stands := await asyncio.to_thread(self._extension_stands, p)) is not None:
            return base | flag | {"paired": False, "reason": stands[0], "why": stands[1]}
        if state == "missing":                 # its files read differently a moment apart: still no page
            return base | flag | {"paired": False, "why": "FERB's extension isn't in that profile"}
        if state == "off":                     # live 2026-10-27: a page sent to a turned-off copy is blocked by Chrome
            return base | flag | {"paired": False,
                                  "why": "FERB's extension is in that profile but turned off: the owner turns it on "
                                         "there (Chrome shows “Action Required”, or the switch on its extensions "
                                         "page), then pair"}
        token = self.pairings.issue_token(p.browser, p.folder)
        try:
            wake = await self._open_pairing(p.browser, p.folder, token)
            end = self.clock.monotonic() + timeout_s
            recheck = self.clock.monotonic() + self.pair_load_wait_s
            while self.clock.monotonic() < end:
                br = self.hub.bridges.get((p.browser, p.folder)) if self.hub else None
                if token in self.pairings.paired and br is not None and br.conn:
                    await self._close_wake(br, wake)
                    self._journal().append("browsers.paired", {"browser": p.browser, "folder": p.folder})
                    return base | {"paired": True}
                # HAND-412: what FERB learns meanwhile ends the wait at once: the page was answered but the extension's
                # hello was refused (too old, newer than FERB, no signing nonce), or the profile's own files now say
                # FERB's copy is off, gone or too old (read again each time the page has had its load time again)
                why = self._answered_but_refused(p, token)
                if why is None and self.clock.monotonic() >= recheck:
                    recheck = self.clock.monotonic() + self.pair_load_wait_s
                    why = await asyncio.to_thread(self._extension_stands, p)
                if why is not None:
                    self.pairings.revoke(token)              # a page answered late can't pair it after this
                    self._journal().append("browsers.pair_failed", {"browser": p.browser, "folder": p.folder,
                                                                     "reason": why[0]})
                    return base | {"paired": False, "reason": why[0], "why": why[1]}
                await asyncio.sleep(0.2)
        except asyncio.CancelledError:
            self.pairings.revoke(token)                      # Pair all's caller left: the key can't pair later
            raise
        self.pairings.revoke(token)                          # its page went unanswered: the key can't pair later
        stands = await asyncio.to_thread(self._extension_stands, p)
        self._journal().append("browsers.pair_failed", {"browser": p.browser, "folder": p.folder,
                                                         "why": "the pairing page wasn't answered",
                                                         **({"reason": stands[0]} if stands else {})})
        if stands is not None:
            return base | {"paired": False, "reason": stands[0], "why": stands[1]}
        label = browser_profiles.BROWSERS[p.browser].label if p.browser in browser_profiles.BROWSERS else p.browser
        return base | {"paired": False, "reason": "no_answer",
                       "hint": "is the FERB extension loaded and enabled in that profile?",
                       "why": PAIR_UNANSWERED.format(label=label)}

    def _answered_but_refused(self, p: Any, token: str) -> tuple[str, str] | None:
        """HAND-412: the pairing page was answered (its key used) and FERB then refused that profile's hello."""
        if self.pairings is None or self.hub is None or token not in self.pairings.paired:
            return None
        b = self.pairings.for_profile(p.browser, p.folder)
        problem = self.hub.problem(b) if b is not None else None
        return ("problem", problem) if problem is not None else None

    def _extension_stands(self, p: Any) -> tuple[str, str] | None:
        """HAND-412: why this profile's FERB copy can't answer a pairing page, from its files now (off, with
        Chromium's reason; not there: removed by the owner, a request waiting for the browser's restart, or not added;
        installed version below the minimum), as Pair all's skip says it (HAND-449: the same `not_paired_step`, with
        the browser's install request read as `read_states` reads it); None: on and new enough as far as its files say.
        Blocking (file reads)."""
        from ferb.act.browser import autoinstall
        from ferb.device.extensions import extension_path, not_paired_step, store_page
        data = (self.browser_data_dirs or {}).get(p.browser) \
            or browser_profiles.data_dir(p.browser, self.browser_support)
        store = autoinstall.store_extension_id(self.config)
        st = autoinstall.profile_status(p, autoinstall.accepted_extension_ids(self.config), data, None, store)
        label = browser_profiles.BROWSERS[p.browser].label if p.browser in browser_profiles.BROWSERS else p.browser
        requested = autoinstall.valid_id(store) and (data / "External Extensions" / f"{store}.json").is_file()
        return not_paired_step(st, label, requested, store_page(store), extension_path(self.home))

    async def _pair_for_task(self, browser: str, folder: str) -> dict[str, Any]:
        """A task's browser_pair, after the owner's Yes (the gateway asked): that exact profile."""
        return await self._pair_one(self._exact_profile({"browser": browser, "folder": folder}), self.pair_timeout_s)

    def _profile_display_name(self, browser: str, folder: str) -> str:
        p = next((p for p in self._profiles() if (p.browser, p.folder) == (browser, folder)), None)
        return p.display_name if p is not None else folder

    def _profile_account(self, browser: str, folder: str) -> str:
        """The account that profile is signed in to its browser with, read fresh (HAND-424); "" when it isn't signed in
        or FERB can't read it (Full Disk Access off)."""
        p = next((p for p in self._profiles() if (p.browser, p.folder) == (browser, folder)), None)
        return p.email if p is not None else ""

    async def _handoff(self, ref: Any, message: str, ctx: Any) -> dict[str, Any]:
        """A human check handed to the owner (owner 2026-10-27: one notification opens the exact tab, and FERB carries
        on by itself once the page passes, with no "done" reply). One notification, whose click brings this tab forward
        (`browser.show_handoff`); then a wait outside the stall clock, however long (rule 9), until the page no longer
        shows the check's own words on two looks in a row. The browser_verification ability's fallback: the worker
        tries the check first."""
        import uuid
        from urllib.parse import urlsplit
        from ferb.core.errors import DriverError
        assert self.router is not None
        first = await self.router.shows(ref, message)
        if not first.get("open"):
            return {"passed": False, "error": "that tab isn't open any more"}
        if first.get("shows") is False:
            return {"passed": True, "url": first["url"], "title": first["title"],
                    "next": "the page doesn't show those words now: take a fresh snapshot and carry on"}
        hid = uuid.uuid4().hex[:12]
        self._handoffs[hid] = ref
        host = (urlsplit(first["url"]).hostname or "a page").removeprefix("www.")
        browser, folder = self.router.tab_profile(ref)
        await self._notify("FERB needs you for a moment",
                           f"{host} wants to check you're human. Click to open the tab; FERB carries on by itself "
                           "once it lets you through.", urgent=True, category="question",
                           target={"kind": "handoff", "handoff_id": hid, "task_id": ctx.task_id}, ident=f"handoff:{hid}")
        self._journal().append("browser.handoff", {"task_id": ctx.task_id, "handoff_id": hid, "host": host,
                                                    "browser": browser, "folder": folder})
        if self.supervisor is not None:
            self.supervisor.activity.waiting(ctx.worker_id, True)    # the owner's time never counts as a stall
        try:
            clear, now = 0, first
            while clear < 2:
                await asyncio.sleep(self.handoff_poll_s)
                try:
                    now = await self.router.shows(ref, message)
                except DriverError:
                    continue                                  # the extension is reconnecting: look again
                if not now.get("open"):
                    self._journal().append("browser.handoff_ended", {"handoff_id": hid, "passed": False})
                    return {"passed": False, "error": "the tab was closed before the check let the owner through"}
                clear = clear + 1 if now.get("shows") is False else 0
            self._journal().append("browser.handoff_ended", {"handoff_id": hid, "passed": True})
            return {"passed": True, "url": now.get("url", ""), "title": now.get("title", ""),
                    "next": "the check let the owner through: take a fresh snapshot of this tab and carry on"}
        finally:
            if self.supervisor is not None:
                self.supervisor.activity.waiting(ctx.worker_id, False)
            self._handoffs.pop(hid, None)

    async def _show_handoff(self, session: Session, params: dict[str, Any]) -> dict[str, Any]:
        """The owner clicked a human check's notification: its tab comes forward (when its profile is connected), else
        that profile's browser does, with a window of the profile."""
        ref = self._handoffs.get(str(params.get("handoff_id") or ""))
        if ref is None or self.router is None:
            return {"shown": False, "why": "that check is already over"}
        if await self.router.show_tab(ref):
            return {"shown": True, "tab": True}
        browser, folder = self.router.tab_profile(ref)
        kwargs: dict[str, Any] = {"launch": self.browser_launch} if self.browser_launch else {}
        browser_profiles.open_known_profile(browser, folder, None, self._profiles(), window=True, **kwargs)
        return {"shown": True, "tab": False}

    async def _setup_browser_pair(self, session: Session, params: dict[str, Any]) -> dict[str, Any]:
        """Setup's or Settings' Pair on one profile (owner 2026-10-27: "single per profile too")."""
        p = self._exact_profile(params)
        if p is None:
            raise FerbError("name the profile's browser and folder")
        return await self._pair_one(p, float(params.get("timeout_s", 60)))

    async def _setup_browsers_pair_all(self, session: Session, params: dict[str, Any]) -> dict[str, Any]:
        """Pair all as one reply: the last event of `_pair_all_events` (the stream FERB.app follows live)."""
        reply: dict[str, Any] = {}
        async for event in self._pair_all_events(params):
            reply = event
        return {k: v for k, v in reply.items() if k != "type"}

    async def _setup_browsers_pairing(self, session: Session, params: dict[str, Any]) -> AsyncIterator[dict[str, Any]]:
        """Pair all, live (HAND-412): `setup.browsers_pairing` streams `_pair_all_events`, so FERB.app shows each
        profile's pairing as it opens and each result as it lands, and ends with the same reply as Pair all's."""
        async for event in self._pair_all_events(params):
            yield event

    async def _pair_all_events(self, params: dict[str, Any]) -> AsyncIterator[dict[str, Any]]:
        """Pair all (owner 2026-10-27: "a pair all which opens all pairs instantly and closes all too. SO THERES NO
        MANUAL STEP LATER"): every profile whose FERB extension is in and not yet paired, all at once.
        HAND-411: every other owner profile is listed in `skipped` with its structural reason and the owner's step
        (`not_paired_step`; a paired profile that can't connect gives `hub.problem`'s words), so nothing is passed
        over in silence; `profiles` counts the owner's profiles (FERB's own is left out). Nothing opens or restarts a
        browser here. HAND-412: events, in order: `plan` (the profiles it pairs, `todo`, and every one it skips, each
        skipped one that FERB's store page could add marked `can_add`), a `result` per profile as it lands, then
        `done` with the whole reply. Leaving the stream stops the pairings still waiting (their keys revoked)."""
        from ferb.act.browser import autoinstall
        from ferb.device.extensions import extension_path, not_paired_step, store_page
        store = autoinstall.store_extension_id(self.config)
        ids = autoinstall.accepted_extension_ids(self.config)
        paired = {(b.browser, b.folder) for b in self.pairings.all()} if self.pairings is not None else None
        data = await asyncio.to_thread(self._browser_data)
        states = await asyncio.to_thread(autoinstall.read_states, self.browser_support, store, ids, paired, data)
        known = {(p.browser, p.folder): p for p in await asyncio.to_thread(self._profiles)}
        url, ext_dir, ferb = store_page(store), extension_path(self.home), self._identity_profile()
        todo: list[Any] = []
        skipped: list[dict[str, Any]] = []
        labels: dict[tuple[str, str], str] = {}
        owner = 0
        for s in states:
            label = browser_profiles.BROWSERS[s.browser].label if s.browser in browser_profiles.BROWSERS \
                else s.browser
            for st in s.profiles:
                if (st.browser, st.folder) == ferb:
                    continue
                owner += 1
                labels[(st.browser, st.folder)] = label
                why = not_paired_step(st, label, s.request is not None, url, ext_dir)
                if why is None and st.paired:
                    why = self._paired_problem(st.browser, st.folder)
                    if why is None:
                        continue                            # paired already, nothing known against it
                if why is not None:
                    skipped.append({"browser": st.browser, "label": label, "folder": st.folder, "name": st.name,
                                    "email": st.email, "reason": why[0], "step": why[1],
                                    "can_add": bool(url) and why[0] in ADDABLE})
                elif (p := known.get((st.browser, st.folder))) is not None:
                    todo.append(p)
        if skipped:
            self._journal().append("browsers.pair_all_skipped", {"skipped": [
                {k: s[k] for k in ("browser", "folder", "reason")} for s in skipped]})
        yield {"type": "plan", "profiles": owner, "skipped": skipped, "store_url": url or None,
               "todo": [{"browser": p.browser, "folder": p.folder, "name": p.display_name, "email": p.email,
                         "label": labels.get((p.browser, p.folder), p.browser)} for p in todo]}
        timeout = float(params.get("timeout_s", 60))
        jobs = [asyncio.ensure_future(self._pair_one(p, timeout)) for p in todo]
        results: list[dict[str, Any]] = []
        try:
            for finished in asyncio.as_completed(jobs):
                r = await finished
                r = r | {"label": labels.get((r["browser"], r["folder"]), r["browser"])}
                results.append(r)
                yield {"type": "result"} | r
        finally:
            for job in jobs:                                 # the caller left: nothing keeps waiting for it
                job.cancel()
        yield {"type": "done", "profiles": owner, "skipped": skipped, "store_url": url or None,
               "results": results, "paired": sum(1 for r in results if r.get("paired"))}

    def _paired_problem(self, browser: str, folder: str) -> tuple[str, str] | None:
        """A paired profile whose extension isn't connected and that FERB knows why (HAND-406: refused in this run,
        or too old), as Pair all's skip reason."""
        b = self.pairings.for_profile(browser, folder) if self.pairings else None
        if b is None or self.hub is None:
            return None
        br = self.hub.bridges.get((browser, folder))
        if br is not None and br.conn:
            return None
        why = self.hub.problem(b)
        return ("problem", why) if why is not None else None

    async def _browsers_installed(self, session: Session, params: dict[str, Any]) -> dict[str, Any]:
        """Is FERB's extension installed in that profile yet (read from its settings on disk)?"""
        r = browser_profiles.resolve(str(params.get("profile", "")), self._profiles(), browser=params.get("browser"))
        if r.kind != "one":
            raise FerbError(r.message(str(params.get("profile", "")), self._profiles()))
        return {"installed": self._profile_state(r.candidates[0]) != "missing"}

    async def _browsers_unpair(self, session: Session, params: dict[str, Any]) -> dict[str, Any]:
        assert self.pairings
        known = self._profiles()
        r = browser_profiles.resolve(str(params.get("profile", "")), known, browser=params.get("browser"))
        if r.kind != "one":
            raise FerbError(r.message(str(params.get("profile", "")), known))
        p = r.candidates[0]
        return {"unpaired": self.pairings.unpair(p.browser, p.folder)}

    async def _setup_browsers(self, session: Session, params: dict[str, Any]) -> dict[str, Any]:
        """L34 C: each browser and profile as the setup window shows it (`autoinstall.read_states`): FERB's extension
        not added, removed, added, on or paired; whether FERB asked the browser to install it; where Load unpacked is
        the way. No browser is opened. Before Full Disk Access nothing is looked at (`waiting`); with it, FERB's own
        files are written into each installed browser first (`_connect_browsers`), and a browser that refused them is
        named in `problem` (DMN-507)."""
        from ferb.act.browser import autoinstall
        from ferb.device.extensions import browser_rows, extension_path
        store = autoinstall.store_extension_id(self.config)
        if not await asyncio.to_thread(self._other_app_data):
            return browser_rows([], store, extension_path(self.home), self._identity_profile(), set()) | {
                "waiting": "full_disk_access", "problem": WAITS_FOR_FULL_DISK_ACCESS}
        files = await self._connect_browsers()
        ids = autoinstall.accepted_extension_ids(self.config)
        paired = {(b.browser, b.folder) for b in self.pairings.all()} if self.pairings is not None else None
        states = await asyncio.to_thread(autoinstall.read_states, self.browser_support, store, ids, paired,
                                         self.browser_data_dirs)
        # APP-892: the version each connected copy said in its hello, and the browsers with an unpaired copy connected
        connected = {key: br.version for key, br in self.hub.bridges.items() if br.conn} if self.hub is not None else {}
        others = await asyncio.to_thread(browser_profiles.unsupported)          # HAND-415
        rows = browser_rows(states, store, extension_path(self.home), self._identity_profile(), connected, others,
                            self.hub.unpaired_copies() if self.hub is not None else {})
        why = self._profile_problems()
        for browser in rows["browsers"]:          # APP-883: each profile's own problem on its row
            for row in browser["profiles"]:
                if (text := why.get((browser["browser"], row["folder"]))) is not None:
                    row["problem"] = text
        problems = ([files["problem"]] if files.get("problem") else []) + await self._extension_problems()
        return rows | ({"problem": " ".join(problems)} if problems else {})

    def _profile_problems(self) -> dict[tuple[str, str], str]:
        """HAND-406: why each paired profile that isn't connected isn't (refused in this run, or its extension last
        reported a version FERB no longer takes), when FERB knows."""
        if self.pairings is None or self.hub is None:
            return {}
        out = {}
        for b in self.pairings.all():
            br = self.hub.bridges.get((b.browser, b.folder))
            if (br is None or br.conn is None) and (why := self.hub.problem(b)) is not None:
                out[(b.browser, b.folder)] = why
        return out

    async def _extension_problems(self) -> list[str]:
        """HAND-406: each paired profile that isn't connected and that FERB knows why, in the owner's words for setup's
        message."""
        why = self._profile_problems()
        if not why:
            return []
        found = await asyncio.to_thread(self._profiles)
        return [f"{browser_profiles.shown(browser_profiles.named(found, b, f))}: {text}."
                for (b, f), text in why.items()]

    async def _connect_browsers(self) -> dict[str, Any]:
        """DMN-507: FERB's native host and the Web Store install request in every installed browser, written by the
        daemon (which holds Full Disk Access; FERB.app never does), at the setup tour's Pair browsers step (each
        `setup.browsers`) and at start once access is there, so an update and a browser installed later are covered.
        Written again only when the installed browsers or the accepted extension ids changed, or the last try failed.
        A browser folder that refused is reported (`problem`) and journaled, never taken as done."""
        if not await asyncio.to_thread(self._other_app_data):
            return {"state": "waiting"}
        async with self._browser_files_lock:
            result: dict[str, Any] = await asyncio.to_thread(self._write_browser_files)
        if result.get("written"):
            self._journal().append("browser.files_written", {k: result[k] for k in ("hosts", "requests", "failed")})
        return result

    def _write_browser_files(self) -> dict[str, Any]:
        """The file work of `_connect_browsers`, in a worker thread."""
        from ferb.act.browser import autoinstall
        from ferb.config import InsecureConfig
        from ferb.device.install import NATIVE_HOST_MANIFEST, Installer, write_native_hosts
        from ferb.paths import app_bundle
        support = self.browser_support
        try:
            ids = autoinstall.accepted_extension_ids(self.config)
            store = autoinstall.store_extension_id(self.config)
        except InsecureConfig as e:
            return {"state": "failed", "written": False, "problem": f"FERB couldn't set up your browsers: {e}"}
        installed = browser_profiles.installed(support)
        key = (tuple(installed), ids, store)
        if key == self._browser_files_done:
            return {"state": "connected", "written": False}
        inst = Installer(self.home, bundle=app_bundle())
        dirs = autoinstall.native_host_dirs(support, installed)
        _, failed_hosts = write_native_hosts(self.home, inst.native_host_launcher, inst.python_command(), ids, dirs)
        autoinstall.withdraw(self.home, store if autoinstall.valid_id(store) else "")
        states = autoinstall.prepare(support, store, ids, None, home=self.home)
        labels = {browser_profiles.data_dir(k, support): v.label for k, v in browser_profiles.BROWSERS.items()}
        failed = [{"browser": labels.get(d.parent, d.parent.name), "file": NATIVE_HOST_MANIFEST, "error": why}
                  for d, why in failed_hosts]
        failed += [{"browser": browser_profiles.BROWSERS[s.browser].label, "file": "install request",
                    "error": s.error} for s in states if s.error]
        self._browser_files_done = None if failed else key
        problem = ("FERB couldn't connect " + ", ".join(dict.fromkeys(f["browser"] for f in failed)) + " ("
                   + "; ".join(dict.fromkeys(f["error"] for f in failed)) + "); it tries again the next time it "
                   "starts or you open Browsers.") if failed else ""
        return {"state": "failed" if failed else "connected", "written": True, "hosts": len(dirs) - len(failed_hosts),
                "requests": sum(1 for s in states if s.request is not None), "failed": failed, "problem": problem}

    async def _setup_browser_open(self, session: Session, params: dict[str, Any]) -> dict[str, Any]:
        """L34 C: the owner's Add to <browser> on one profile: FERB's Web Store page opens in that profile, in front (the
        owner does the clicks; FERB never adds it for them, L25), or chrome://extensions when no store id is set (Load
        unpacked). Only a profile discovery returned; journaled."""
        from ferb.act.browser import autoinstall
        from ferb.device.extensions import store_page
        browser, folder = str(params.get("browser", "")), str(params.get("folder", ""))
        url = store_page(autoinstall.store_extension_id(self.config)) or "chrome://extensions"
        known = self._profiles()
        kwargs: dict[str, Any] = {"launch": self.browser_launch} if self.browser_launch else {}
        try:
            await asyncio.to_thread(browser_profiles.open_known_profile, browser, folder, url, known,
                                    background=False, **kwargs)
        except ValueError as e:
            raise FerbError(str(e)) from None
        self._journal().append("setup.browser_opened", {"browser": browser, "folder": folder, "url": url})
        return {"opened": url}
