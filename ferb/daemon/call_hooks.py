"""Owns what happens around a tool call without deciding whether it may run (kb area gateway:
"non-authorization features out of the authorization path"): the task's live preview and FERB's cursor point before
it, the desktop samples around a call that can change the screen (DMN-5), the change record's copies of
the files it names (L39), the working app's first picture, image sanitizing and the evidence store (V1, D26, L13), the
advisory shadow check (D23, shadow only) and the live line. Each hook only records or shows; a failing hook never
stops or changes the call's authorization, and none is run before the owner's questions are answered."""

import asyncio
import dataclasses
import logging
from pathlib import Path
from typing import Any

from ferb.act.fs import changes as file_changes
from ferb.act.fs.files import owner_path
from ferb.core import origin
from ferb.core.errors import NoTarget, Withheld
from ferb.daemon.auth import RunGrant
from ferb.policy.rules import Kind
from ferb.sanitize.redact import Redactor

# L39: the kinds of call that can change the owner's files, whose named files the change record copies first
CHANGE_KINDS = frozenset({Kind.WRITE_FILE, Kind.TRASH, Kind.DELETE_PERMANENT, Kind.RUN_SHELL, Kind.APPLESCRIPT})


def evidence_kind(tool: str) -> str:
    """What a tool's image is, for the evidence store: a snapshot's image, a recording's frames, else a screenshot."""
    if tool.endswith(".snapshot"):
        return "snapshot"
    return "recording" if tool.startswith("recording.") else "screenshot"


IMAGE_FACTS = ("app", "bundle_id", "window", "title", "url", "tab", "shot", "width", "height", "window_id", "path",
               "duration_s", "sample_times_s")


def image_facts(tool: str, content: dict[str, Any]) -> dict[str, Any]:
    """The small facts kept beside a tool's image (already redacted content): where it came from, its size, and how
    many controls the result listed."""
    facts: dict[str, Any] = {"tool": tool} | {k: content[k] for k in IMAGE_FACTS if content.get(k) not in (None, "")}
    if isinstance(content.get("elements"), list):
        facts["elements"] = len(content["elements"])
    return facts


def on_screen(cap: Any) -> bool:
    """A capability that can change what is on the owner's screen (bring an app or window forward, switch the Space,
    move the cursor, open a dialog) declares `on_screen = True`: only those calls are sampled (DMN-5). A
    declaration, like `observation`, never a name list (rule 6)."""
    return bool(getattr(cap, "on_screen", False))


def error_name(e: BaseException) -> str:
    """An error for the trace by type (and errno, or a capture error's domain and code, I7) only: its text can hold an
    absolute path."""
    errno = getattr(e, "errno", None) if isinstance(e, OSError) else None
    domain, code = getattr(e, "domain", None), getattr(e, "code", None)
    if errno is None and isinstance(domain, str) and isinstance(code, int):
        return f"{type(e).__name__} ({domain} {code})"
    return type(e).__name__ + (f" (errno {errno})" if errno is not None else "")


def _screen_recording_allowed() -> bool:
    """Whether macOS lets FERB record the screen (read only when a preview capture failed, to say why)."""
    try:
        from ferb.device import permissions
        return permissions.screen_recording() is permissions.Status.GRANTED
    except Exception:
        return True


class CallHooks:
    """The gateway's hooks around a call (`gw` is the gateway: its locks, redactor, stores and events)."""

    def __init__(self, gw: Any) -> None:
        from ferb.daemon.task_preview import TaskPreviews
        self.gw = gw
        # a preview looks alongside a call that only reads (DMN-406); its problems go to the journal, bounded
        self.previews = TaskPreviews(self._frame_sanitized,
                                     lambda keys, holder: self.gw.locks.try_hold(keys, holder, during_reads=True),
                                     _screen_recording_allowed, note=self._note_preview)
        self.looked: dict[tuple[str, str], set[str]] = {}   # (task, worker) -> apps whose window it has as evidence
        self._sampled: set[str] = set()                    # calls sampled before they ran (their span ids)

    def forget(self, task_ids: set[str]) -> None:
        self.previews.remove(task_ids)
        for key in [k for k in self.looked if k[0] in task_ids]:
            del self.looked[key]

    async def screen_before(self, grant: RunGrant, span_id: str, name: str, arguments: dict[str, Any],
                            cap: Any) -> None:
        """DMN-5: the desktop is sampled before a call only when it can change the owner's screen."""
        sentinel = self.gw.disturbance
        if sentinel is not None and not getattr(cap, "observation", False) and hasattr(sentinel, "call_started"):
            sentinel.call_started(grant.task_id, span_id, name)   # HAND-207: a sound change near it is noted
        if sentinel is not None and on_screen(cap):
            self._sampled.add(span_id)
            await sentinel.before_action(grant.task_id, span_id, name, arguments, True)

    async def screen_after(self, span_id: str) -> dict[str, Any]:
        """What FERB could have changed on the screen during the call ({} when not sampled or nothing changed)."""
        sentinel = self.gw.disturbance
        if sentinel is not None and hasattr(sentinel, "call_ended"):
            sentinel.call_ended(span_id)
        if sentinel is None or span_id not in self._sampled:
            return {}
        self._sampled.discard(span_id)
        try:
            return dict(await sentinel.after_action(span_id) or {})
        except asyncio.CancelledError:
            raise
        except Exception as e:                           # a failed sample never fails the call
            return {"error": error_name(e)}

    def shadow(self, tool: str, spec: Any, target: dict[str, Any], code_questions: list[str], task_id: str,
               owner_words: str, span_id: str) -> None:
        """D23 shadow: advice only; scheduling, completion and answers can't authorize or refuse the call."""
        shadow = self.gw.shadow_action
        if shadow is None:
            return
        self.gw._in_background(shadow({"tool": tool, "kind": spec.kind, "target": target, "content": spec.content,
                                       "code_questions": code_questions, "task_id": task_id,
                                       "owner_words": owner_words}, "span:" + span_id), "shadow")

    async def redact_image(self, result: Any) -> tuple[Any, dict[str, Any]]:
        """The tool's image through the F5 sanitizer (L13); facts only when that fails."""
        from ferb.sanitize.images import ImageRedactor
        redactor = self.gw.redact if isinstance(self.gw.redact, Redactor) else Redactor()
        try:
            image = await asyncio.to_thread(ImageRedactor(redactor), result.image)
            return dataclasses.replace(result, image=image), {}
        except asyncio.CancelledError:
            raise
        except Exception:
            return dataclasses.replace(result, image=None), {"image_withheld": "image redaction unavailable"}

    def preview(self, cap: Any, args: Any, grant: RunGrant, rec: Any, locks: list[str],
                observed: dict[str, Any] | None = None) -> None:
        """The live preview's surface for this call, as the capability declares it (`preview_target(args, owner,
        observed)`, beside `point` and `on_screen`; never a list of tool names or arguments here). Before the call, what
        it works on (none: a call on no surface, which freezes the preview on its last frame); after a successful call,
        what its result shows (`observed`, e.g. a snapshot's current window), when that says more. Nothing is resolved
        or captured now (the first frame request does that), so it never delays the call; a declaration that fails is
        no surface, and never touches the call or its result (kb area app)."""
        declare = getattr(cap, "preview_target", None)
        target = None
        try:
            if callable(declare):
                target = declare(args, grant.task_id, observed)
        except Exception:
            target = None
        if observed is not None and target is None:
            return                                       # the result adds nothing: the call's own surface stands
        root = (getattr(rec, "tree_id", None) if rec is not None else None) or grant.task_id.split(".", 1)[0]
        self.previews.declare(root, grant.task_id, target, tuple(locks))

    def _note_preview(self, kind: str, data: dict[str, Any]) -> None:
        """A preview problem (preview.slow / preview.failed: surface kind, phase or error class, never a frame, title
        or URL) in the daemon's journal; TaskPreviews bounds how often."""
        note = getattr(self.gw, "journal_problem", None)
        if callable(note):
            note(kind, data)

    async def _frame_sanitized(self, image: bytes) -> bytes:
        """A preview frame through the same masking as every image (L13): its codes, card numbers and known secrets
        painted over by text recognition, off the loop."""
        from ferb.sanitize.images import ImageRedactor
        redactor = self.gw.redact if isinstance(self.gw.redact, Redactor) else Redactor()
        return await asyncio.to_thread(ImageRedactor(redactor), image)

    async def point(self, cap: Any, args: Any, grant: RunGrant) -> None:
        """L33: the element this action is about to touch, for FERB's cursor (geometry only, `cursor.point`), named by
        the step's root task, so a task keeps one pointer across its steps and the app ends it with that task's
        `task.result`. Never delays or fails the action: skipped after POINT_TIMEOUT_S or on any error."""
        from ferb.act.point import POINT_TIMEOUT_S, Point
        if not hasattr(cap, "point"):
            return
        try:
            p = await asyncio.wait_for(cap.point(args, grant), POINT_TIMEOUT_S)
        except asyncio.CancelledError:
            raise
        except Exception:
            return
        if isinstance(p, Point):
            rec = self.gw.tasks.get(grant.task_id)
            self.gw.publish("cursor.point", p.payload((rec.tree_id if rec is not None else None) or grant.task_id,
                                                   grant.worker_id))

    async def initial_image(self, name: str, cap: Any, args: Any, spec: Any, grant: RunGrant, result: Any) -> Any:
        """D6/D26: first successful working-app observation supplies one image in this task's tool reply.
        Selection comes from the actual app/tab reference; no desktop or unrelated browser tab substitution.
        """
        if grant.observe_only or not (name.startswith("mac.") or name in {"browser.snapshot", "ferb_browser.snapshot"}):
            return result
        looked = self.looked.setdefault((grant.task_id, "initial"), set())
        if looked or (name.startswith("mac.") and (not spec.target.site or self.gw.app_window is None)):
            return result
        looked.add("attempted")  # reserve before awaiting, so concurrent calls cannot emit two initial pictures
        try:
            if name.startswith("mac."):
                shot = await self.gw.app_window(spec.target.site, grant.scopes)
                if shot is None:
                    return result
                image = shot.image
                if image is not None and shot.rects:
                    from ferb.act.evidence.mask import mask_secret_fields
                    image = await asyncio.to_thread(mask_secret_fields, image, shot.rects)
                return dataclasses.replace(result, image=image, content={**result.content, "initial_image": shot.facts},
                                           read_sources=tuple(dict.fromkeys(result.read_sources + shot.read_sources)))
            tab = cap.driver.owned_by(args.tab, grant.task_id)
            # screenshot() uses the existing snapshot generation, preserving the element ids returned by this call.
            image = await cap.driver.screenshot(tab)
            shot, width, height = None, None, None
            masking = getattr(cap.driver, "masking", lambda ref: "")(tab)   # a page the extension couldn't read (B6)
            return dataclasses.replace(result, image=image, content={**result.content,
                "initial_image": {"shot": shot, "width": width, "height": height}
                | ({"masking": masking} if masking else {})})
        except NoTarget:
            looked.clear()  # no window yet; first actual working window can still supply the initial image
            return result
        except Withheld as e:                            # its secret fields couldn't be painted over (L13): FERB's words
            return dataclasses.replace(result, content={**result.content, "image_withheld": str(e)})
        except Exception:
            return dataclasses.replace(result, content={**result.content, "image_withheld": "initial window capture unavailable"})

    def initial_image_given(self, task_id: str) -> None:
        """L12 (D26 clarified): this task's worker was handed the owner's front window at send as its start-of-task
        screenshot, with its first model request; its first working-app observation takes no second one."""
        self.looked.setdefault((task_id, "initial"), set()).add("front window at send")

    async def watch_changes(self, cap: Any, args: Any, spec: Any, grant: RunGrant, rec: Any) -> dict[str, Any]:
        """L39: before a call that can change files runs, the change record copies each existing file it names and
        lists their folders (ferb/act/fs/changes.py): a file tool's declared files (what it writes or removes), and
        every path a shell command or AppleScript writes out (`changes.named`, from the folder it runs in). The step's
        checker then reads an old-vs-new diff of what changed. Facts of the call's text, never a judgment of it (rule
        6). Best effort: a failure is recorded on the call and never stops it. Returns the counts for its record."""
        if self.gw.changes is None or spec.kind not in CHANGE_KINDS:
            return {}
        paths: list[Path] = []
        try:
            declared = getattr(cap, "file_paths", None)
            if declared is not None:
                paths += [owner_path(p, self.gw.roots.workspace) for p, access in declared(args) if access != "read"]
            if spec.kind in (Kind.RUN_SHELL, Kind.APPLESCRIPT) and spec.content:
                here = spec.target.path if spec.kind == Kind.RUN_SHELL and spec.target.path else self.gw.roots.workspace
                paths += file_changes.named(str(spec.content), owner_path(str(here), self.gw.roots.workspace))
            if not paths:
                return {}
            return await asyncio.to_thread(self.gw.changes.before, grant.task_id, rec.tree_id or grant.task_id, paths)
        except asyncio.CancelledError:
            raise
        except Exception as e:                            # never stops the call: its check gets no diff for it
            return {"error": error_name(e)}

    def _looked_at(self, grant: RunGrant) -> set[str]:
        return self.looked.setdefault((grant.task_id, grant.worker_id), set())

    async def keep_image(self, name: str, spec: Any, grant: RunGrant, span_id: str, content: dict[str, Any],
                          result: Any, origins: set[str]) -> dict[str, str]:
        """V1: the image a tool produced is kept as evidence the checker can re-inspect (on the owner's Mac for
        retention.evidence_days, 7); the span's output names it. It is the image the model got: its secret fields
        were painted over where it was captured and it passed the F5 sanitizer (L13), wherever it came from (its
        origin label never names an excluded host). Facts only for an image already withheld. Never fatal: the result
        reaches the worker anyway. The file is written in a worker thread."""
        initial = content.get("initial_image")
        facts = dict(initial) if isinstance(initial, dict) else image_facts(name, content)
        if content.get("image_withheld"):
            facts["image_withheld"] = content["image_withheld"]
        kept: bytes | None = result.image
        try:
            kind = "app_window" if isinstance(initial, dict) and name.startswith("mac.") else evidence_kind(name)
            eid = await self.gw.evidence.put_async(grant.task_id, span_id, kind, image=kept, data=facts,
                                                origin=origin.join(origins))
        except asyncio.CancelledError:
            raise
        except Exception as e:
            return {"evidence_error": error_name(e)}
        if name.startswith("mac.") and spec.target.site:            # this image (kept or facts only) was the
            self._looked_at(grant).add(spec.target.site.lower())    # worker's first look at that app
        return {"evidence": eid}

    def started(self, grant: RunGrant, name: str, args: dict[str, Any], kind: str, site: str | None) -> None:
        """Tells the observer a call starts (the task's live line, L8). The observer notes its own failures; one that
        still escapes goes to the daemon's log, by type only, and never stops the call."""
        try:
            self.gw.observer.tool_started(grant, name, args, kind, site)
        except Exception as e:
            logging.getLogger(__name__).warning("FERB live line failed: %s", error_name(e))
