"""Writes hooks.json: the real Python `CallHooks` (ferb/daemon/call_hooks.py) driven op by op over recording fakes of
every part it drives (the disturbance sentinel, the live previews, the evidence store, the change record, the image
sanitizer, the app-window picture and its masking, the shadow advisor, the event bus), for the Rust port
(rust/crates/ferb-gateway/src/call_hooks.rs, ENG-3). Each case records every call a fake received, in order, and
what each op returned. Two runs give the same bytes.

Run from the repo root:  uv run python rust/fixtures/gateway/make_hooks.py
"""

import asyncio
import base64
import json
import os
import sys
from pathlib import Path
from types import SimpleNamespace
from typing import Any

REPO = Path(__file__).resolve().parents[3]
HERE = Path(__file__).resolve().parent
os.environ["HOME"] = "/HOME/user"                     # paths are text here: nothing is read or written
sys.path.insert(0, str(REPO))

from ferb.act.point import Point, PreviewTarget  # noqa: E402
from ferb.core.errors import NoTarget, Withheld  # noqa: E402
from ferb.core.types import ActionSpec, Identity, Target, ToolResult  # noqa: E402
from ferb.daemon import call_hooks as hooks_mod  # noqa: E402
from ferb.daemon.auth import RunGrant  # noqa: E402
from ferb.daemon.gateway import _screen_told  # noqa: E402
from ferb.sanitize.redact import Redactor  # noqa: E402

WORKSPACE = "/HOME/ferb/workspace"


def raise_of(spec: dict[str, Any]) -> BaseException:
    """The exception a fake raises; its `error_name` is the spec's `name` (what the Rust fake returns)."""
    cls = {"OSError": OSError, "RuntimeError": RuntimeError, "ValueError": ValueError}[spec["class"]]
    e = cls(spec["errno"], "boom") if "errno" in spec else cls("boom")
    assert hooks_mod.error_name(e) == spec["name"], (hooks_mod.error_name(e), spec["name"])
    return e


def b(text: str | None) -> bytes | None:
    return None if text is None else text.encode()


def s(data: bytes | None) -> str | None:
    return None if data is None else data.decode()


class Case:
    def __init__(self, cfg: dict[str, Any]) -> None:
        self.cfg, self.calls, self.events, self.journal = cfg, [], [], []
        log = self.calls

        class Sentinel:
            def call_started(self, task: str, span: str, name: str) -> None:
                log.append(["sentinel.call_started", task, span, name])

            async def before_action(self, task: str, span: str, name: str, args: dict[str, Any], on: bool) -> None:
                log.append(["sentinel.before_action", task, span, name, args, on])
                if cfg.get("before_raises"):
                    raise raise_of(cfg["before_raises"])

            def call_ended(self, span: str) -> None:
                log.append(["sentinel.call_ended", span])

            async def after_action(self, span: str) -> dict[str, Any]:
                log.append(["sentinel.after_action", span])
                if cfg.get("after_raises"):
                    raise raise_of(cfg["after_raises"])
                return dict(cfg.get("after", {}))

        class Evidence:
            async def put_async(self, task: str, span: str, kind: str, *, image: bytes | None, data: dict[str, Any],
                                origin: str) -> str:
                log.append(["evidence.put", task, span, kind, s(image), data, origin])
                if cfg.get("evidence_raises"):
                    raise raise_of(cfg["evidence_raises"])
                return "ev-1"

        class Changes:
            def before(self, task: str, tree: str, paths: list[Path]) -> dict[str, int]:
                log.append(["changes.before", task, tree, [str(p) for p in paths]])
                if cfg.get("changes_raises"):
                    raise raise_of(cfg["changes_raises"])
                return {"copied": len(paths)}

        async def app_window(app: str, scopes: frozenset[str]) -> Any:
            log.append(["app_window", app, sorted(scopes)])
            w = cfg.get("app_window")
            if w == "NoTarget":
                raise NoTarget("no window")
            if isinstance(w, str) and w.startswith("Withheld:"):
                raise Withheld(w[len("Withheld:"):])
            if w == "Failed":
                raise RuntimeError("capture failed")
            if w is None:
                return None
            return SimpleNamespace(image=b(w.get("image")), rects=[tuple(r) for r in w.get("rects", [])],
                                   facts=w.get("facts", {}), read_sources=tuple(tuple(x) for x in w.get("reads", [])))

        async def shadow(call: dict[str, Any], ref: str) -> None:
            log.append(["shadow", call, ref])
            if cfg.get("shadow_raises"):
                raise raise_of(cfg["shadow_raises"])

        tree = cfg.get("tree", {})
        self.gw = SimpleNamespace(
            disturbance=Sentinel() if cfg.get("sentinel", True) else None,
            evidence=Evidence() if cfg.get("evidence", True) else None,
            changes=Changes() if cfg.get("changes", True) else None,
            app_window=app_window if cfg.get("app_windows", True) else None,
            shadow_action=shadow if cfg.get("shadow", True) else None,
            roots=SimpleNamespace(workspace=WORKSPACE), redact=Redactor(),
            journal_problem=lambda kind, data: self.journal.append([kind, data]),
            publish=lambda topic, data: self.events.append([topic, data]),
            tasks=SimpleNamespace(get=lambda t: SimpleNamespace(tree_id=tree[t]) if t in tree else None),
            locks=SimpleNamespace(try_hold=lambda keys, holder, during_reads=False: None),
            _background=set())

        def in_background(work: Any, what: str) -> None:
            async def run() -> None:
                try:
                    await work
                except Exception as e:
                    self.gw.journal_problem("gateway.background_failed", {"what": what, "error": hooks_mod.error_name(e)})
            self.background.append(asyncio.ensure_future(run()))
        self.background: list[Any] = []
        self.gw._in_background = in_background
        self.hooks = hooks_mod.CallHooks(self.gw)

        class Previews:
            def declare(self, root: str, task: str, target: Any, locks: tuple[str, ...]) -> None:
                log.append(["previews.declare", root, task, None if target is None else target.scope, list(locks)])

            def remove(self, task_ids: set[str]) -> None:
                log.append(["previews.remove", sorted(task_ids)])
        self.hooks.previews = Previews()


class Cap:
    """A capability from a spec: what the hooks read off it (declarations, its point, its tab's picture)."""

    def __init__(self, spec: dict[str, Any]) -> None:
        self.spec = spec
        self.name = spec.get("name", "probe.tool")
        if spec.get("on_screen"):
            self.on_screen = True
        if spec.get("observation"):
            self.observation = True
        if "preview" in spec:
            def preview_target(args: Any, owner: str, observed: Any = None) -> Any:
                p = spec["preview"]
                if p == "raise":
                    raise RuntimeError("declaration failed")
                if isinstance(p, dict):
                    p = p.get("observed" if observed is not None else "before")
                return None if p is None else PreviewTarget(p, "the app", None)
            self.preview_target = preview_target
        if "point" in spec:
            async def point(args: Any, grant: RunGrant) -> Any:
                p = spec["point"]
                if p == "raise":
                    raise RuntimeError("no element")
                if p == "slow":
                    await asyncio.sleep(5)
                return None if p is None else Point(p["kind"], tuple(p["rect"]), p["display"])
            self.point = point
        if "file_paths" in spec:
            self.file_paths = lambda args: [tuple(x) for x in spec["file_paths"]]
        if "tab_picture" in spec:
            tp = spec["tab_picture"]

            class Driver:
                def owned_by(self, tab: str, owner: str) -> str:
                    return tab

                async def screenshot(self, ref: str) -> bytes:
                    if tp == "NoTarget":
                        raise NoTarget("no tab")
                    if isinstance(tp, str) and tp.startswith("Withheld:"):
                        raise Withheld(tp[len("Withheld:"):])
                    if tp == "Failed":
                        raise RuntimeError("tab gone")
                    return tp["image"].encode()

                def masking(self, ref: str) -> str:
                    return tp.get("masking", "") if isinstance(tp, dict) else ""
            self.driver = Driver()


def grant_of(op: dict[str, Any]) -> RunGrant:
    return RunGrant(op.get("task", "t1"), op.get("worker", "w1"), frozenset(op.get("scopes", ["probe"])),
                    Identity.OWNER, 1e9, observe_only=op.get("observe_only", False))


def spec_of(op: dict[str, Any]) -> ActionSpec:
    return ActionSpec(op.get("kind", "read"), Target(site=op.get("site"), path=op.get("path")),
                      content=op.get("content"))


async def run_op(c: Case, op: dict[str, Any]) -> Any:
    h, kind = c.hooks, op["op"]
    cap = Cap(op.get("tool", {}))
    args = SimpleNamespace(**op.get("args", {}))
    if kind == "screen_before":
        try:                                          # the gateway's: a failed sample never stops the call
            await h.screen_before(grant_of(op), op["span"], cap.name, op.get("args", {}), cap)
        except Exception:
            pass
        return None
    if kind == "screen_after":
        return await h.screen_after(op["span"])
    if kind == "told":
        return _screen_told(op["screen"])
    if kind == "shadow":
        h.shadow(op["name"], spec_of(op), op["target"], op["ids"], op.get("task", "t1"), op["words"], op["span"])
        await asyncio.gather(*c.background)
        return None
    if kind == "redact_image":
        out, facts = await h.redact_image(ToolResult({}, image=b(op["image"])))
        return {"image": s(out.image), "facts": facts}
    if kind == "preview":
        rec = SimpleNamespace(tree_id=op.get("tree"))
        h.preview(cap, args, grant_of(op), rec, op.get("locks", []), op.get("observed"))
        return None
    if kind == "point":
        await h.point(cap, args, grant_of(op))
        return None
    if kind == "initial_image":
        result = ToolResult(op.get("content", {"ok": True}), read_sources=tuple(tuple(x) for x in op.get("reads", [])))
        out = await h.initial_image(op["name"], cap, args, spec_of(op), grant_of(op), result)
        return {"content": out.content, "image": s(out.image), "reads": [list(x) for x in out.read_sources]}
    if kind == "initial_image_given":
        h.initial_image_given(op.get("task", "t1"))
        return None
    if kind == "image_provided":                      # the gateway's own mark (gateway.py:2059)
        h.looked.setdefault((op.get("task", "t1"), "initial"), set()).add("provided")
        return None
    if kind == "watch_changes":
        rec = SimpleNamespace(tree_id=op.get("tree"))
        return await h.watch_changes(cap, args, spec_of(op), grant_of(op), rec)
    if kind == "keep_image":
        result = ToolResult({}, image=b(op.get("image")))
        return await h.keep_image(op["name"], spec_of(op), grant_of(op), op["span"], op["content"], result,
                                  set(op.get("origins", [])))
    if kind == "forget":
        h.forget(set(op["tasks"]))
        return None
    if kind == "looked":
        return sorted(h.looked.get((op.get("task", "t1"), op.get("worker", "w1")), set()))
    raise ValueError(kind)


def mask_patch(c: Case) -> None:
    import ferb.act.evidence.mask as mask

    def fake(image: bytes, rects: list[Any]) -> bytes:
        c.calls.append(["mask", image.decode(), [list(r) for r in rects]])
        if c.cfg.get("mask_raises"):
            raise Withheld(c.cfg["mask_raises"])
        return image + b"+masked"
    mask.mask_secret_fields = fake


def images_patch(c: Case) -> None:
    import ferb.sanitize.images as images

    class Fake:
        def __init__(self, redactor: Any) -> None:
            pass

        def __call__(self, image: bytes) -> bytes:
            c.calls.append(["images.sanitize", image.decode()])
            if c.cfg.get("sanitize_raises"):
                raise RuntimeError("vision unavailable")
            return image + b"+clean"
    images.ImageRedactor = Fake


SCREEN_TOOL = {"name": "mac.click", "on_screen": True}
PIC = {"image": "PIXELS", "rects": [[1, 2, 3, 4]], "facts": {"app": "Notes", "window": "Notes"},
       "reads": [["app", "com.apple.Notes"], ["web", "https://x.test"]]}

CASES: list[dict[str, Any]] = [
    {"name": "screen_samples_only_calls_that_can_change_it", "config": {"after": {"frontmost_app": {
        "before": {"name": "Notes"}, "after": {"bundle_id": "com.apple.Safari"}}, "fullscreen": {"after": True}}},
     "ops": [{"op": "screen_before", "span": "s1", "tool": SCREEN_TOOL, "args": {"app": "Notes"}},
             {"op": "screen_after", "span": "s1"}, {"op": "screen_after", "span": "s1"},
             {"op": "screen_before", "span": "s2", "tool": {"name": "fs.write"}},
             {"op": "screen_after", "span": "s2"},
             {"op": "screen_before", "span": "s3", "tool": {"name": "fs.read", "observation": True}},
             {"op": "screen_after", "span": "s3"}]},
    {"name": "a_failed_sample_never_fails_the_call",
     "config": {"after_raises": {"class": "OSError", "errno": 5, "name": "OSError (errno 5)"},
                "before_raises": {"class": "RuntimeError", "name": "RuntimeError"}},
     "ops": [{"op": "screen_before", "span": "s1", "tool": SCREEN_TOOL}, {"op": "screen_after", "span": "s1"}]},
    {"name": "no_sentinel", "config": {"sentinel": False},
     "ops": [{"op": "screen_before", "span": "s1", "tool": SCREEN_TOOL}, {"op": "screen_after", "span": "s1"}]},
    {"name": "what_the_worker_is_told",
     "ops": [{"op": "told", "screen": {}}, {"op": "told", "screen": {"key_window": {"before": 1, "after": 2}}},
             {"op": "told", "screen": {"frontmost_app": {"before": "Notes", "after": {"name": "", "bundle_id": "b"}},
                                       "key_window": 1, "active_space_epoch": {"before": 1, "after": 2}}},
             {"op": "told", "screen": {"fullscreen": {"before": False}}},
             {"op": "told", "screen": {"error": "OSError (errno 5)"}}]},
    {"name": "shadow_advice_and_its_failure",
     "config": {"shadow_raises": {"class": "ValueError", "name": "ValueError"}},
     "ops": [{"op": "shadow", "name": "mail.send", "kind": "send", "site": "x.test", "content": "hi",
              "target": {"recipients": ["a@x.test"]}, "ids": ["c1"], "words": "send it", "span": "s9"}]},
    {"name": "shadow_advice", "ops": [{"op": "shadow", "name": "mail.send", "kind": "send", "content": None,
                                       "target": {}, "ids": [], "words": "", "span": "s1"}]},
    {"name": "no_shadow", "config": {"shadow": False},
     "ops": [{"op": "shadow", "name": "mail.send", "kind": "send", "target": {}, "ids": [], "words": "", "span": "s"}]},
    {"name": "images_sanitized_or_withheld",
     "ops": [{"op": "redact_image", "image": "PIXELS"}]},
    {"name": "images_withheld", "config": {"sanitize_raises": True},
     "ops": [{"op": "redact_image", "image": "PIXELS"}]},
    {"name": "previews_as_the_tool_declares",
     "ops": [{"op": "preview", "tool": {"preview": "window:Notes"}, "task": "t1.2", "tree": "t1", "locks": ["app:x"]},
             {"op": "preview", "tool": {}, "task": "t1.2", "locks": []},
             {"op": "preview", "tool": {"preview": "raise"}, "task": "t2"},
             {"op": "preview", "tool": {"preview": {"before": "tab:a", "observed": None}}, "observed": {"ok": True}},
             {"op": "preview", "tool": {"preview": {"before": None, "observed": "tab:b"}}, "observed": {"ok": True},
              "tree": "t9"},
             {"op": "forget", "tasks": ["t1", "t2"]}]},
    {"name": "ferbs_cursor_points",
     "config": {"tree": {"t1.2": "t1"}},
     "ops": [{"op": "point", "tool": {}}, {"op": "point", "tool": {"point": None}},
             {"op": "point", "tool": {"point": "raise"}},
             {"op": "point", "tool": {"point": {"kind": "click", "rect": [10, 20.5, 30, 40], "display": 1}},
              "task": "t1.2", "worker": "w7"},
             {"op": "point", "tool": {"point": {"kind": "type", "rect": [1, 2, 3, 4], "display": 2}}, "task": "t3"}]},
    {"name": "first_picture_of_a_mac_app", "config": {"app_window": PIC},
     "ops": [{"op": "initial_image", "name": "mac.snapshot", "site": "Notes", "content": {"elements": [1]},
              "reads": [["app", "com.apple.Notes"], ["app", "com.apple.Notes"]], "scopes": ["mac:read"]},
             {"op": "initial_image", "name": "mac.snapshot", "site": "Notes"},
             {"op": "looked", "worker": "initial"}]},
    {"name": "first_picture_skipped", "config": {"app_window": PIC},
     "ops": [{"op": "initial_image", "name": "mac.snapshot", "site": "Notes", "observe_only": True},
             {"op": "initial_image", "name": "fs.read"},
             {"op": "initial_image", "name": "mac.snapshot", "site": None},
             {"op": "initial_image_given", "task": "t5"},
             {"op": "initial_image", "name": "mac.snapshot", "site": "Notes", "task": "t5"},
             {"op": "image_provided", "task": "t6"},
             {"op": "initial_image", "name": "mac.snapshot", "site": "Notes", "task": "t6"}]},
    {"name": "first_picture_no_window_yet_then_one", "config": {"app_window": "NoTarget"},
     "ops": [{"op": "initial_image", "name": "mac.snapshot", "site": "Notes"}, {"op": "looked", "worker": "initial"}]},
    {"name": "first_picture_withheld", "config": {"app_window": "Withheld:its secret fields couldn't be painted over"},
     "ops": [{"op": "initial_image", "name": "mac.snapshot", "site": "Notes"},
             {"op": "initial_image", "name": "mac.snapshot", "site": "Notes"}]},
    {"name": "first_picture_failed", "config": {"app_window": "Failed"},
     "ops": [{"op": "initial_image", "name": "mac.act", "site": "Notes"}]},
    {"name": "first_picture_none", "config": {"app_window": None},
     "ops": [{"op": "initial_image", "name": "mac.act", "site": "Notes"}, {"op": "looked", "worker": "initial"}]},
    {"name": "first_picture_mask_withheld", "config": {"app_window": PIC, "mask_raises": "masking failed"},
     "ops": [{"op": "initial_image", "name": "mac.snapshot", "site": "Notes"}]},
    {"name": "first_picture_without_app_windows", "config": {"app_windows": False},
     "ops": [{"op": "initial_image", "name": "mac.snapshot", "site": "Notes"},
             {"op": "looked", "worker": "initial"}]},
    {"name": "first_picture_of_a_tab",
     "ops": [{"op": "initial_image", "name": "browser.snapshot", "args": {"tab": "t1"},
              "tool": {"tab_picture": {"image": "TAB", "masking": "the extension couldn't read this page"}}},
             {"op": "initial_image", "name": "ferb_browser.snapshot", "task": "t2", "args": {"tab": "t1"},
              "tool": {"tab_picture": {"image": "TAB2"}}},
             {"op": "initial_image", "name": "browser.snapshot", "task": "t3", "args": {"tab": "t1"},
              "tool": {"tab_picture": "NoTarget"}},
             {"op": "initial_image", "name": "browser.snapshot", "task": "t4", "args": {"tab": "t1"},
              "tool": {"tab_picture": "Withheld:the page's secret fields couldn't be painted over"}},
             {"op": "initial_image", "name": "browser.snapshot", "task": "t5", "args": {"tab": "t1"},
              "tool": {"tab_picture": "Failed"}}]},
    {"name": "change_record_copies_what_a_call_names",
     "ops": [{"op": "watch_changes", "kind": "write_file", "tree": "t1",
              "tool": {"file_paths": [["notes.md", "write"], ["~/a.txt", "read"], ["~/b.txt", "remove"],
                                      ["/abs/c.txt", "metadata"]]}},
             {"op": "watch_changes", "kind": "run_shell", "content": "cd sub && echo hi > out.txt; cp ~/x ./y",
              "path": "/HOME/user/proj"},
             {"op": "watch_changes", "kind": "run_shell", "content": "touch new.txt"},
             {"op": "watch_changes", "kind": "applescript", "content": 'do shell script "rm ~/old.txt"',
              "path": "/ignored"},
             {"op": "watch_changes", "kind": "read", "tool": {"file_paths": [["x", "write"]]}},
             {"op": "watch_changes", "kind": "write_file", "tool": {"file_paths": [["x", "read"]]}},
             {"op": "watch_changes", "kind": "trash", "tool": {}}]},
    {"name": "change_record_failure_is_recorded",
     "config": {"changes_raises": {"class": "OSError", "errno": 13, "name": "PermissionError (errno 13)"}},
     "ops": [{"op": "watch_changes", "kind": "write_file", "tool": {"file_paths": [["x", "write"]]}}]},
    {"name": "no_change_record", "config": {"changes": False},
     "ops": [{"op": "watch_changes", "kind": "write_file", "tool": {"file_paths": [["x", "write"]]}}]},
    {"name": "evidence_kept_and_what_the_worker_looked_at",
     "ops": [{"op": "keep_image", "name": "mac.snapshot", "site": "Notes", "span": "s1", "image": "IMG",
              "content": {"initial_image": {"app": "Notes"}, "image_withheld": ""},
              "origins": ["external:app:Notes", "private"]},
             {"op": "keep_image", "name": "browser.screenshot", "span": "s2", "image": "IMG2",
              "content": {"url": "https://x.test", "tab": "", "title": None, "width": 10, "elements": [1, 2],
                          "image_withheld": "image redaction unavailable"}},
             {"op": "keep_image", "name": "recording.stop", "span": "s3", "content": {"path": "/r.mov"}},
             {"op": "keep_image", "name": "mac.snapshot", "site": "Safari", "span": "s4", "content": {"app": "S"},
              "worker": "w2"},
             {"op": "keep_image", "name": "browser.snapshot", "span": "s5", "content": {"initial_image": {}}},
             {"op": "looked"}, {"op": "looked", "worker": "w2"}, {"op": "forget", "tasks": ["t1"]},
             {"op": "looked", "worker": "w2"}]},
    {"name": "evidence_failure_is_named",
     "config": {"evidence_raises": {"class": "OSError", "errno": 28, "name": "OSError (errno 28)"}},
     "ops": [{"op": "keep_image", "name": "mac.snapshot", "site": "Notes", "span": "s1", "image": "IMG",
              "content": {}}, {"op": "looked"}]},
]


async def one(case: dict[str, Any]) -> dict[str, Any]:
    c = Case(case.get("config", {}))
    mask_patch(c)
    images_patch(c)
    results = []
    for op in case["ops"]:
        results.append(await run_op(c, op))
    return {**case, "results": results, "calls": c.calls, "events": c.events, "journal": c.journal}


async def main() -> int:
    out = [await one(case) for case in CASES]
    for case in out:
        json.dumps(case)                                   # every recorded value is plain JSON
    text = json.dumps({"about": "The real Python CallHooks over recording fakes; see make_hooks.py.", "cases": out},
                      ensure_ascii=False, indent=1) + "\n"
    (HERE / "hooks.json").write_text(text, encoding="utf-8")
    return len(out)


if __name__ == "__main__":
    print(f"hooks.json: {asyncio.run(main())} cases")
    _ = base64
