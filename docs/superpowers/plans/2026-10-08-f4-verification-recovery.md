# F4 Verification and Recovery Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** F4 makes FERB's "done" trustworthy and its actions recoverable:
- a task is **verified**, **done but unverified**, or **failed** — three distinct statuses, each with the real reason (V4, V5);
- the final checker is read-only, **enforced by code**, not by its prompt (V3, W4);
- every acting tool reads back what changed, and a mismatch is a fact for the worker (V2);
- brief states (a toast, a ringing call) are captured to an **evidence store** the checker can re-inspect, kept 7 days (V1, D20, D26, D29);
- a consequential action whose outcome is unknown is **never retried** without the owner (P19);
- FERB's own file changes and deletions are **recoverable** (clone copies, Trash with a durable record, a snapshot before bulk work) (S1);
- a success condition that names a target is met only by **evidence about that target**, never a substitute (D34);
- a browser download becomes **facts in FERB's workspace**, never a dialog on the owner's screen (D35).

**Architecture:**
- **Evidence store.** A new store (`ferb/act/evidence/`) persists redacted media (accessibility snapshots, recordings, and app-window screenshots with secret fields masked) under `~/.ferb/evidence/`, indexed in a new table. The gateway persists the image a tool already produced instead of discarding it. Retention purges media older than 7 days.
- **Read-only checker.** The verifier's run grant is marked observation-only; the gateway refuses any non-observation capability for it. This replaces the prompt-only rule.
- **Statuses.** A new `done_unverified` status is distinct from `failed`. Composers, stats, baseline, chat and the trace tell them apart.
- **Ledger for recovery only.** Read-only AppleScripts stop creating rows; `ledger_done` is dropped as a success check. An unresolved-unknown consequential action blocks its own retry until the owner approves (P19).
- **Recovery.** Before FERB overwrites, edits or trashes a file, code keeps a clone copy (APFS `clonefile`, fallback copy) and a durable record; a snapshot is taken before bulk or destructive work; `ferb restore` lists and restores.
- **Downloads.** The extension intercepts downloads (suppressing the native dialog) and routes the bytes to FERB's workspace; the Python side records the file as facts with an origin label.

**Tech Stack:** Python 3.12; SQLite (migrations 0020–0025); macOS `NSFileManager` Trash, APFS `clonefile(2)` via `ctypes`/`Foundation`, `tmutil localsnapshot`; the Chromium extension (`extensions/chromium/`, protocol bump 9 → 10); pytest.

**Spec:** `docs/0.27_PLAN.md`:
- the "Final build order" F4 line and B4;
- the "Verification audit" and "P16. Verification design (draft)" (V1–V8);
- D6, D20, D22, D23 (recoverability clause), D26, D28, D29, D31, D34, D35, P19, S1, W1, W2, W4;
- R5Q2 and P27 (owner-facing wording).

Registry: `docs/0.27_DECISIONS.toml` gives D6, D20, D26, D29, P16, P19, S1, D34, D35 milestone F4.

## Global Constraints

- Python 3.12. Before every commit, run the focused tests, then `uv run pytest -q`, `uv run mypy ferb/core` and `git diff --check`.
- New schema goes only in new migration files. The current head is `0019`. The migration runner (`ferb/store/db.py:22-30`) tracks a single `MAX(version)` high-water mark and applies only files whose number is greater than it, so **migrations must be numbered in task-execution order** — a lower number added in a later task is skipped forever. The numbers are: `0020_evidence` (Task 1, runs first), `0021_task_status_unverified` (Task 4), `0022_ledger_retry_approved` (Task 7, added during execution), `0023_recovery` (Task 8), `0024_recovery_trash_identity` (Task 8 fix round 1), `0025_download` (Task 10). Never edit an existing migration.
- **Code enforces boundaries, never a model (CLAUDE.md rule 5).** V3 (read-only checker) and P19 (no silent retry of an unknown action) are code boundaries. A prompt that merely asks the model to behave is not enough; a test must show the gateway refusing.
- **Code never judges intent (rule 6).** The read-only gate keys on a capability's declared action kind (W2), never on a keyword or content list. P19 keys on the ledger's recorded outcome, never on what the action "looks like".
- **Secrets never reach a trace, log, the database or a new store (rule 14).** Stored evidence passes through the existing redactor for text; an app-window screenshot has its accessibility-known secret fields masked before it is written. Full screenshot redaction (OTP digits, notification-matched codes, CSS-masked fields) is F5; F4's masking is the interim floor and is documented as such. Evidence files live only on the owner's Mac and are never sent anywhere the live model-send didn't already go.
- **S1 recoverability (rule 8, D23).** A file FERB overwrites, edits or deletes must be recoverable for 7 days (a clone copy plus, for deletes, the Trash). F5's destructive-action confirmations treat an action made recoverable by S1 as not destructive, so S1 must exist and be correct here.
- **Background first (rule 17).** None of this brings a window forward, moves the cursor or changes the owner's Space. Screenshots and recordings cover only the app FERB is working on, never the whole desktop (D29). Downloads never open a dialog (D35).
- **Verification shape (R5Q2, P27, D20/V4/V5).** The checker's verdict decides the first owner-facing line; a worker's claim is shown as fact only for a verified condition; "unverified" appears only when a check actually ran and could not confirm. Owner-facing text uses the owner's names, never internal ones (`worker`, `checker`, `task.evidence`, `Profile 2`).
- **Commit messages** cite decision IDs and end with `Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>`.
- Do not install or restart the running daemon (the owner runs `ferb setup --update`), and do not install or restart the owner's personal browsers. Never read, search or reuse `vibed/`. Offline tests never mark themselves `real`. Shared test helpers go in `tests/support/`.
- **Extension protocol changes update three things together (rule, D35):** the manifest version, the JavaScript protocol constant and the Python protocol/min-version constants.

## Review Focus

1. **The checker tries to click.** A verifier run must not be able to send, submit, type, delete, write or run a shell command, even if its prompt is bypassed by a prompt injection in the page it inspects. It returns "unknown" instead. Test: Task 2 `test_the_checker_cannot_act_even_when_asked`.
2. **A retry of an unknown payment or message.** After a consequential action whose outcome is unknown (a cancel or a restart mid-call), the same action must not run again without the owner. Test: Task 7 `test_an_unknown_consequential_action_is_not_retried_without_the_owner`.
3. **A screenshot with a visible password.** An app-window screenshot stored as evidence must have its accessibility-known secret fields masked before it touches disk. Test: Task 1 `test_a_stored_screenshot_masks_secret_fields`.
4. **Overwriting the wrong file.** When FERB overwrites or edits a file, the previous contents must be recoverable for 7 days; a clone onto a different volume, where `clonefile` fails, must fall back to a copy and still be recoverable. Tests: Task 8 `test_an_overwrite_keeps_a_recoverable_copy` and `test_recovery_falls_back_when_clone_is_unavailable`.
5. **A substitute reported as the named target.** A condition that names a target the worker could not reach (the "Code Astra" / Copilot case) must verify as unverified or failed, never "done", when the evidence is about a different target. Test: Task 9 `test_a_substitute_does_not_satisfy_a_named_target_condition`.

---

### Task 1: The evidence store, app-window screenshots, and 7-day media retention (V1, D20, D26, D29)

**Files:**
- Create: `ferb/act/evidence/__init__.py`, `ferb/act/evidence/store.py`, `ferb/act/evidence/mask.py`
- Create: `ferb/store/migrations/0020_evidence.sql`
- Modify: `ferb/daemon/gateway.py` (persist a tool's image to the store instead of only attaching it to the reply; `ferb/daemon/gateway.py:305-306`)
- Modify: `ferb/orchestrator/worker.py` (capture an app-window screenshot when a worker first acts on a GUI app)
- Modify: `ferb/store/retention.py` and `ferb/daemon/main.py` `_purge_pass` (purge evidence older than `retention.evidence_days`)
- Modify: `ferb/config.py` (default `retention.evidence_days = 7`)
- Modify: `ferb/orchestrator/evidence.py` `TaskEvidence` (a verifier can list and read stored evidence of this task)
- Test: `tests/test_evidence_store.py`

**Interfaces:**
- Consumes: `origin.*` (F3), the redactor (`gateway.redact`), `ferb/act/mac/driver.py` screenshot (`screenshot.py`), `ferb/act/browser/snapshot.py` element `secret`/`rect` fields.
- Produces:
  - `EvidenceStore(root: Path, clock, ids, redact)` with:
    - `put(task_id, span_id, kind: str, *, image: bytes | None = None, data: dict | None = None, mime="image/jpeg", origin: str) -> str` returning an evidence id; writes the file under `root/<day>/<id>.<ext>` and a row in `evidence`;
    - `list(task_id) -> list[dict]` (id, kind, captured_at, origin, path, has_image);
    - `read(evidence_id) -> tuple[dict, bytes | None]`;
    - `purge_before(cutoff_iso) -> int` (deletes rows and files).
  - `mask.mask_secret_fields(image: bytes, rects: list[tuple[int,int,int,int]]) -> bytes` — returns the JPEG with those rectangles filled opaque.
  - The `evidence` table.
  - `EvidenceStore` is attached to the gateway as `self.evidence`, and a `screen` capture helper the worker calls.

- [ ] **Step 1: Migration**

`ferb/store/migrations/0020_evidence.sql`:

```sql
-- F4 (V1, D20): captured state (accessibility snapshots, recordings, app-window screenshots) the checker can
-- re-inspect, kept 7 days (retention.evidence_days). Media files live under ~/.ferb/evidence; this indexes them.
-- Screenshots are stored with accessibility-known secret fields masked (ferb/act/evidence/mask.py); full secret
-- redaction is F5. Nothing here holds a secret value.
CREATE TABLE evidence (
    id          TEXT PRIMARY KEY,
    task_id     TEXT NOT NULL,
    span_id     TEXT,
    kind        TEXT NOT NULL,        -- app_window | snapshot | recording | screenshot
    mime        TEXT,                 -- image/jpeg, video/mp4; NULL for a data-only row
    path        TEXT,                 -- file on disk, relative to the evidence root; NULL for a data-only row
    data_json   TEXT,                 -- small structured facts (title, url, element count), redacted
    origin      TEXT NOT NULL,        -- ferb/core/origin.py
    captured_at TEXT NOT NULL
);
CREATE INDEX evidence_task ON evidence(task_id);
CREATE INDEX evidence_time ON evidence(captured_at);
```

- [ ] **Step 2: Write the failing tests**

```python
# tests/test_evidence_store.py
"""Captured state the checker can re-inspect, kept 7 days (V1, D20); app-window screenshots mask secret fields."""
from datetime import timedelta

from ferb.act.evidence import mask
from ferb.act.evidence.store import EvidenceStore
from tests.conftest import RunningDaemon, chat, done, install, sup, task


def test_put_and_read_round_trip(daemon: RunningDaemon) -> None:
    store = daemon.daemon.evidence
    eid = daemon.run(_put(store, "t1", "s1", "snapshot", data={"title": "Inbox"}, origin="external:web:mail.google.com"))
    rows = store.list("t1")
    assert len(rows) == 1 and rows[0]["kind"] == "snapshot" and rows[0]["origin"] == "external:web:mail.google.com"
    meta, blob = store.read(eid)
    assert meta["data"]["title"] == "Inbox" and blob is None


def test_a_stored_screenshot_masks_secret_fields(daemon: RunningDaemon) -> None:
    store = daemon.daemon.evidence
    img = _solid_jpeg(60, 40)                              # a known all-white image
    eid = daemon.run(_put(store, "t1", "s1", "app_window", image=img,
                          rects=[(10, 10, 20, 20)], origin="external:web:x"))
    _, blob = store.read(eid)
    assert mask.pixel(blob, 15, 15) == (0, 0, 0)          # inside the masked rect: opaque
    assert mask.pixel(blob, 2, 2) != (0, 0, 0)            # outside: untouched


def test_retention_purges_media_older_than_seven_days(daemon: RunningDaemon) -> None:
    store = daemon.daemon.evidence
    eid = daemon.run(_put(store, "t1", "s1", "snapshot", data={"a": 1}, origin="ferb"))
    path = daemon.daemon.home / "evidence"
    old = (daemon.daemon.clock.now() - timedelta(days=8)).isoformat()
    daemon.run(_age(daemon, old))
    assert store.purge_before((daemon.daemon.clock.now() - timedelta(days=7)).isoformat()) == 1
    assert store.list("t1") == []


def test_the_checker_can_read_the_tasks_evidence(daemon: RunningDaemon) -> None:
    # A snapshot stored during the worker is listed back through task.evidence for the checker.
    store = daemon.daemon.evidence
    daemon.run(_put(store, "probe", "s1", "snapshot", data={"title": "Order placed"}, origin="external:web:x"))
    assert [r["kind"] for r in store.list("probe")] == ["snapshot"]


def test_secret_text_in_evidence_data_is_redacted(daemon: RunningDaemon) -> None:
    store = daemon.daemon.evidence
    eid = daemon.run(_put(store, "t1", "s1", "snapshot",
                          data={"note": "my key is sk-ant-api03-abcdefghijklmnopqrstuvwxyz0123456789"}, origin="ferb"))
    _, _ = store.read(eid)
    rows = daemon.run(_raw(daemon, eid))
    assert "sk-ant-api03" not in rows["data_json"]


async def _put(store, task_id, span_id, kind, *, image=None, data=None, rects=None, origin="ferb"):
    return store.put(task_id, span_id, kind, image=image, data=data, rects=rects, origin=origin)


async def _age(daemon, at):
    daemon.daemon.db.conn.execute("UPDATE evidence SET captured_at = ?", (at,))


async def _raw(daemon, eid):
    return dict(daemon.daemon.db.conn.execute("SELECT data_json FROM evidence WHERE id = ?", (eid,)).fetchone())


def _solid_jpeg(w, h):
    from PIL import Image
    import io
    buf = io.BytesIO()
    Image.new("RGB", (w, h), (255, 255, 255)).save(buf, "JPEG", quality=95)
    return buf.getvalue()
```

Check whether Pillow is a dependency (`grep -i pillow pyproject.toml`, and how `ferb/act/mac/screenshot.py` encodes JPEG). If Pillow isn't available, implement `mask` and `mask.pixel` with the same image library the screenshot code already uses; do not add a dependency without the owner. If no image library is present, mask by decoding to raw RGB with `Quartz`/`CoreImage` the way `screenshot.py` does and re-encoding; keep `mask.pixel` as a test-only helper that decodes one pixel. Adjust the test to the real encoder.

- [ ] **Step 3: Run the tests to see them fail**

Run: `uv run pytest tests/test_evidence_store.py -q`
Expected: FAIL with `ModuleNotFoundError: No module named 'ferb.act.evidence'`.

- [ ] **Step 4: Implement the store and masking**

`ferb/act/evidence/store.py`:

```python
"""Owns FERB's evidence store (V1, D20): captured state the final checker can re-inspect, kept 7 days. Must not hold a
secret value: text passes through the redactor, screenshots have their accessibility-known secret fields masked before
they are written (ferb/act/evidence/mask.py). Files live under ~/.ferb/evidence, never sent anywhere; this is the
owner's own Mac."""

import json
from datetime import datetime
from pathlib import Path
from typing import Any, Callable

from ferb.act.evidence import mask

EXT = {"image/jpeg": "jpg", "video/mp4": "mp4"}


class EvidenceStore:
    def __init__(self, root: Path, clock: Any, ids: Any, redact: Callable[[dict[str, Any]], dict[str, Any]],
                 db: Any) -> None:
        self.root, self.clock, self.ids, self.redact, self.db = root, clock, ids, redact, db
        self.root.mkdir(parents=True, exist_ok=True)

    def put(self, task_id: str, span_id: str | None, kind: str, *, image: bytes | None = None,
            data: dict[str, Any] | None = None, rects: list[tuple[int, int, int, int]] | None = None,
            mime: str = "image/jpeg", origin: str = "ferb") -> str:
        eid = self.ids.new_id()
        at = self.clock.now()
        rel = None
        if image is not None:
            if rects:
                image = mask.mask_secret_fields(image, rects)       # opaque over AX-known secret fields
            day = at.astimezone().date().isoformat()
            (self.root / day).mkdir(parents=True, exist_ok=True)
            rel = f"{day}/{eid}.{EXT.get(mime, 'bin')}"
            (self.root / rel).write_bytes(image)
        payload = json.dumps(self.redact(data)) if data is not None else None
        self.db.conn.execute(
            "INSERT INTO evidence (id, task_id, span_id, kind, mime, path, data_json, origin, captured_at) "
            "VALUES (?,?,?,?,?,?,?,?,?)",
            (eid, task_id, span_id, kind, mime if image is not None else None, rel, payload, origin, at.isoformat()))
        return eid

    def list(self, task_id: str) -> list[dict[str, Any]]:
        rows = self.db.conn.execute(
            "SELECT id, kind, mime, path, origin, captured_at, data_json FROM evidence WHERE task_id = ? "
            "ORDER BY captured_at", (task_id,))
        return [{"id": r["id"], "kind": r["kind"], "mime": r["mime"], "origin": r["origin"],
                 "captured_at": r["captured_at"], "has_image": r["path"] is not None,
                 "data": json.loads(r["data_json"]) if r["data_json"] else None} for r in rows]

    def read(self, evidence_id: str) -> tuple[dict[str, Any], bytes | None]:
        r = self.db.conn.execute("SELECT * FROM evidence WHERE id = ?", (evidence_id,)).fetchone()
        if r is None:
            raise KeyError(evidence_id)
        meta = {"id": r["id"], "kind": r["kind"], "mime": r["mime"], "origin": r["origin"],
                "captured_at": r["captured_at"], "data": json.loads(r["data_json"]) if r["data_json"] else None}
        blob = (self.root / r["path"]).read_bytes() if r["path"] else None
        return meta, blob

    def purge_before(self, cutoff_iso: str) -> int:
        rows = self.db.conn.execute("SELECT id, path FROM evidence WHERE captured_at < ?", (cutoff_iso,)).fetchall()
        for r in rows:
            if r["path"]:
                (self.root / r["path"]).unlink(missing_ok=True)
        self.db.conn.execute("DELETE FROM evidence WHERE captured_at < ?", (cutoff_iso,))
        return len(rows)
```

`ferb/act/evidence/mask.py`: implement `mask_secret_fields(image, rects)` by decoding the JPEG, filling each `(x, y, w, h)` rectangle with black, and re-encoding, using the same image path as `ferb/act/mac/screenshot.py`. Add `pixel(image, x, y) -> tuple[int,int,int]` for the tests. Clamp rectangles to the image bounds.

- [ ] **Step 5: Wire it into the daemon and the gateway**

- In `ferb/daemon/main.py`, build `self.evidence = EvidenceStore(self.home / "evidence", self.clock, self.ids, self.redact, self.db)` where the other stores are built, and pass it to the gateway.
- In `ferb/daemon/gateway.py` `_execute`, where a tool returns an image (`gateway.py:305-306`): before attaching it to the reply, persist it. The origins are the `origins` already computed (F3). Use kind `"snapshot"` for a snapshot image and `"screenshot"` for a screenshot; read the capability's name to tell them apart. Secret-field rectangles come from the result: a browser/mac snapshot lists elements with `secret: True`; map those to rects when the result carries element rects. If the result has no rects, store the image unmasked ONLY when the capability is a pure screenshot of a non-secret context; otherwise, when a `secret` element is present without a rect, do not store the image (store a data-only row noting "image withheld: secret field without a maskable rect"). Record which you did in the report.
- The persisted evidence id goes into the span output as `"evidence": eid`, so the trace can point at it.

- [ ] **Step 6: App-window screenshot when a worker starts on a GUI app (D26, D29)**

In `ferb/orchestrator/worker.py`, when a worker's first GUI action targets an app (the first `mac.*` or `browser.*` tool call for that worker), capture that app's window once to the evidence store with kind `app_window`. The capture reuses the existing app-window screenshot path (`ferb/act/mac/tools.py:164-178` builds a marked window image when the `screen` scope and `snapshot_image` are on). Mask secret fields from the accessibility tree. Never capture the whole desktop (D29). If the worker never acts on a GUI app (a shell-only task), capture nothing. Keep this out of the hot path: one capture per app per worker, best-effort, and a failure is logged, never fatal.

Implementers: decide the cleanest insertion point — the gateway already sees every tool call and the app bundle id, so a gateway-level "first GUI touch per worker" hook may be cleaner than the worker. Pick one, say which, and keep it background-only.

- [ ] **Step 7: Retention and the checker's read access**

- In `ferb/config.py`, add `("retention", "evidence_days", 7)` to the privacy defaults.
- In `ferb/daemon/main.py` `_purge_pass` (and the sync `_purge_records`), after the span/decision purge, call `self.evidence.purge_before((now - timedelta(days=evidence_days)).isoformat())` and add the count to the `retention.purged` journal row as `evidence`.
- In `ferb/orchestrator/evidence.py` `TaskEvidence`, add a branch so the checker can `list` the task's evidence and `read` a `data_json` row's facts (not image bytes; the model receives images only when it captures a fresh one). Keep its origin pass-through (F3). A verifier reads evidence of the task it is checking, resolved through `_inspection_parent` (`main.py:1333-1345`).

- [ ] **Step 8: Run the tests**

Run: `uv run pytest tests/test_evidence_store.py tests/test_tool_origins.py tests/test_retention.py -q`, then `uv run pytest -q` and `uv run mypy ferb/core`.
Expected: PASS.

- [ ] **Step 9: Commit**

```bash
git add ferb/act/evidence ferb/store/migrations/0020_evidence.sql ferb/daemon/gateway.py ferb/orchestrator/worker.py ferb/orchestrator/evidence.py ferb/store/retention.py ferb/daemon/main.py ferb/config.py tests/test_evidence_store.py
git commit -m "F4 (V1, D20, D26, D29): evidence store with 7-day media; app-window screenshots mask secret fields; checker re-inspects

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

### Task 2: The final checker is read-only, enforced by code (V3, W4)

**Files:**
- Modify: `ferb/daemon/auth.py` (`RunGrant` carries `observe_only: bool`; `RunTokens.issue(..., observe_only=False)` records it — `RunGrant` and `RunTokens` live here, `auth.py:57-82`, not in `ferb/core/types.py`)
- Modify: `ferb/daemon/gateway.py` (`tools_for` and `_call` refuse a non-observation capability for an observe-only grant)
- Modify: `ferb/orchestrator/worker.py` (`ferb/orchestrator/worker.py:117`) (the verifier's token is issued observe-only)
- Modify: every capability class, to set a class attribute `observation = True` where it only reads (reads, snapshots, screenshots, scrolling, opening a URL, listing); the gateway reads it with `getattr(cap, "observation", False)`, so no shared base change is needed
- Test: `tests/test_checker_readonly.py`

**Interfaces:**
- Consumes: the verifier `Task(role="verifier")` path (`ferb/orchestrator/supervisor.py:1003-1007`), `gateway.tools_for` (`gateway.py:121-133`), `RunGrant` (fields `task_id, worker_id, scopes, identity, expires_at`; `auth.py:57-62`).
- Produces:
  - capability class attribute `observation: bool` (default `False`, read via `getattr`);
  - `RunGrant.observe_only: bool = False`;
  - `RunTokens.issue(..., observe_only: bool = False)`;
  - `Gateway.tools_for(grant)` returns only observation capabilities when `grant.observe_only`;
  - `_call` refuses a non-observation capability for an observe-only grant with a clear error, before the capability runs.

- [ ] **Step 1: Write the failing tests**

```python
# tests/test_checker_readonly.py
"""The final checker is read-only, enforced by code, not its prompt (V3, W4). A click, send, write or shell is refused
even if the checker is told to do it; it returns unknown instead."""
import json

from tests.conftest import RunningDaemon, FakeCap, gateway_setup, gateway_tool_call


def _verifier_grant(daemon: RunningDaemon, *caps):
    grant, token = gateway_setup(daemon, {"probe"}, None, list(caps))
    return daemon.run(_observe(daemon, grant))


async def _observe(daemon, grant):
    # Re-issue the same grant as observe-only, the way the verifier's token is issued.
    return daemon.daemon.tokens.validate(
        daemon.daemon.tokens.issue(grant.task_id, grant.worker_id, grant.scopes, grant.identity, ttl_s=600,
                                   observe_only=True))


def test_observation_tools_are_offered_acting_tools_are_not(daemon: RunningDaemon) -> None:
    read = FakeCap("probe.read", "read", {"probe"})
    read.observation = True
    send = FakeCap("probe.send", "send", {"probe"})
    grant = _verifier_grant(daemon, read, send)
    names = {t["name"] for t in daemon.daemon.gateway.tools_for(grant)}
    assert "probe.read" in names and "probe.send" not in names


def test_the_checker_cannot_act_even_when_asked(daemon: RunningDaemon) -> None:
    send = FakeCap("probe.send", "send", {"probe"})       # observation defaults False
    grant = _verifier_grant(daemon, send)
    r = gateway_tool_call(daemon, grant, "probe.send", to="x@y.com")
    assert r["is_error"] and "read-only" in r["content"]["error"].lower()
    assert send.calls == 0                                 # the capability never ran


def test_a_normal_worker_still_acts(daemon: RunningDaemon) -> None:
    send = FakeCap("probe.send", "send", {"probe"})
    grant, _ = gateway_setup(daemon, {"probe"}, None, [send])   # not observe-only
    r = gateway_tool_call(daemon, grant, "probe.send", to="x@y.com")
    assert not r["is_error"] and send.calls == 1
```

`FakeCap` is in `tests/conftest.py:375`. `tokens.issue` is in `ferb/daemon/auth.py`; read its real signature and add the `observe_only` keyword. If `gateway_setup` doesn't expose the worker id on the grant, read what `RunGrant` carries (`ferb/core/types.py`) and adapt.

- [ ] **Step 2: Run the tests to see them fail**

Run: `uv run pytest tests/test_checker_readonly.py -q`
Expected: FAIL. `issue()` has no `observe_only`, and the send is not refused.

- [ ] **Step 3: Declare observation capabilities**

Add `observation: bool = False` to the capability protocol/base in `ferb/core/types.py`. Set `observation = True` on every capability that only reads or observes. From the survey and registry, these include: `fs.read`, `fs.list`, `fs.stat`; `shell.read`, `shell.wait`, `shell.ps` (not `shell.run`/`shell.start`/`shell.send`/`shell.kill`); `trail.query`, `trail.days`; `actions.search`; `task.evidence`, `chat.history`, `memory.read`; `browser.snapshot`, `browser.screenshot`, `browser.tabs`, `browser.profiles`, `browser.wait`, and a scroll/open-url read if present; `mac.snapshot`, `mac.screenshot`, `mac.apps`; `recording.inspect`. Opening a URL for inspection: W4 allows it, but opening a tab is `OPEN_TAB`; include `browser.open`/`ferb_browser.open` and `mac.open` as observation ONLY if the owner's W4 ("opening a URL") is meant to include it — the controller's ruling: include `ferb_browser.open` (FERB's own browser) and `browser.navigate` for inspection, but NOT `mac.open` (launching an app is a side effect). List every capability you set, with its kind, in the report, and flag any you are unsure about; an unmarked capability is simply unavailable to the checker, which is the safe default.

- [ ] **Step 4: Carry `observe_only` on the grant and enforce it**

- `RunGrant` (`ferb/core/types.py`) gains `observe_only: bool = False`.
- `Tokens.issue(..., observe_only: bool = False)` records it; `validate` returns it on the grant. Read `ferb/daemon/auth.py` for the exact token record shape.
- In `ferb/daemon/gateway.py`:
  - `tools_for(grant)`: when `grant.observe_only`, filter to `getattr(cap, "observation", False)` in addition to the scope check.
  - `_call`: after resolving `cap` and before running it, `if grant.observe_only and not getattr(cap, "observation", False): return reply({"error": "the final check is read-only: this tool acts and is not available to a checker; report what you could not confirm as unknown"}, True, {origin.FERB})`. Put this next to the existing scope refusal (`gateway.py:204-207`). This is the V3 code boundary: a prompt-injected checker still cannot act.
- In `ferb/orchestrator/worker.py:117`, when `task.role == "verifier"`, issue the token with `observe_only=True`.

- [ ] **Step 5: Update the stale tests that assert the checker gets acting tools**

`tests/test_supervisor.py:128-129` asserts `{"fs.read","fs.write","mac.act","task.evidence"} <= vtools` and `tests/test_inspection_recovery.py:61-76` has the checker call `mac.act`. These encode the old behavior. Change them: the verifier's offered tools must be observation-only (`fs.read` and `task.evidence` yes; `fs.write` and `mac.act` no), and a checker that needs a click returns unknown (which the worker then gathers). Preserve each test's unique intent; where a test needed the checker to click to prove a point, rewrite it so the checker returns unknown and a follow-up worker gathers the evidence (that is the W4 flow). List every test you change and why in the report.

- [ ] **Step 6: Run the tests**

Run: `uv run pytest tests/test_checker_readonly.py tests/test_supervisor.py tests/test_inspection_recovery.py -q`, then `uv run pytest -q` and `uv run mypy ferb/core`.
Expected: PASS.

- [ ] **Step 7: Commit**

```bash
git add ferb/core/types.py ferb/daemon/auth.py ferb/daemon/gateway.py ferb/orchestrator/worker.py tests/test_checker_readonly.py
git add -u ferb/act tests
git commit -m "F4 (V3, W4): the final checker is read-only, enforced by code; acting tools refused for an observe-only grant

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

### Task 3: After-action reads and expect comparison (V2)

**Files:**
- Modify: `ferb/act/mac/driver.py`, `ferb/act/browser/tools.py` / `ext_bridge.py` (every acting result carries a uniform `after` block)
- Modify: `ferb/orchestrator/schemas.py` (a plan step / condition may carry an `expect` the worker compares)
- Create: `ferb/act/aftercheck.py` (compare an `expect` against an `after` read; produce a fact)
- Test: `tests/test_after_action.py`

**Interfaces:**
- Consumes: the existing after-reads the survey found: `mac.act` type/key/edit/set_value/select/scroll return `observed_change` / `value` / `value_matches` (`driver.py:501-638`); `mac.pointer` returns `observed_change` (`driver.py:371-378`); `browser.act` upload compares sha256 (`ext_bridge.py:647-667`), other browser actions return only `{ok, detail}` (`background.js:542-579`).
- Produces:
  - every acting tool's result includes `after: {read: "ax"|"none"|"value", changed: bool|null, value: str|null, note: str}`, a uniform shape over the existing per-action facts;
  - `aftercheck.compare(expect: dict, after: dict) -> dict` returning `{met: bool|null, evidence: str}`;
  - a worker-visible fact when an action's `after` does not match its `expect`.

- [ ] **Step 1: Write the failing tests**

```python
# tests/test_after_action.py
"""After every acting tool, the result says what changed (V2); a procedure's expect line is compared with it."""
from ferb.act import aftercheck


def test_after_block_is_uniform() -> None:
    # A mac type action's per-action facts become the uniform after block.
    facts = {"method": "ax", "delivered": True, "observed_change": True, "outcome_verified": False}
    after = aftercheck.after_of(facts)
    assert after == {"read": "ax", "changed": True, "value": None, "note": ""}


def test_value_read_back() -> None:
    facts = {"method": "ax_value", "accepted": True, "value_matches": True, "value": "done"}
    assert aftercheck.after_of(facts) == {"read": "value", "changed": True, "value": "done", "note": ""}


def test_no_read_back() -> None:
    facts = {"method": "ax", "accepted": True, "outcome_verified": False}   # a press: nothing read after
    assert aftercheck.after_of(facts) == {"read": "none", "changed": None, "value": None, "note": ""}


def test_expect_comparison() -> None:
    assert aftercheck.compare({"value_contains": "Order placed"},
                              {"read": "ax", "changed": True, "value": "Order placed #123", "note": ""}
                              )["met"] is True
    assert aftercheck.compare({"value_contains": "Order placed"},
                              {"read": "ax", "changed": True, "value": "Add to cart", "note": ""})["met"] is False
    assert aftercheck.compare({"changed": True}, {"read": "none", "changed": None, "value": None, "note": ""}
                              )["met"] is None      # nothing was read back: unknown, never a false pass
```

- [ ] **Step 2: Run the tests to see them fail**

Run: `uv run pytest tests/test_after_action.py -q`
Expected: FAIL with `ModuleNotFoundError: No module named 'ferb.act.aftercheck'`.

- [ ] **Step 3: Implement `ferb/act/aftercheck.py`**

```python
"""Owns the uniform after-action read (V2): what an acting tool observed changed, and whether a procedure's expect
matched. Must not claim a change it did not read: a tool that read nothing back reports changed=None (unknown), never
a false pass."""

from typing import Any

_READ = {"ax_value": "value", "value_matches": "value"}


def after_of(facts: dict[str, Any]) -> dict[str, Any]:
    """The acting tool's per-action facts → a uniform after block."""
    if "value_matches" in facts or facts.get("method") == "ax_value":
        return {"read": "value", "changed": bool(facts.get("value_matches", facts.get("accepted"))),
                "value": facts.get("value"), "note": str(facts.get("note", ""))}
    if "observed_change" in facts:
        return {"read": "ax", "changed": bool(facts["observed_change"]), "value": facts.get("value"),
                "note": str(facts.get("note", ""))}
    return {"read": "none", "changed": None, "value": None, "note": str(facts.get("note", ""))}


def compare(expect: dict[str, Any], after: dict[str, Any]) -> dict[str, Any]:
    if "value_contains" in expect:
        if after.get("value") is None:
            return {"met": None, "evidence": "no value was read back after the action"}
        ok = str(expect["value_contains"]).casefold() in str(after["value"]).casefold()
        return {"met": ok, "evidence": f"after value {'contains' if ok else 'does not contain'} the expected text"}
    if "changed" in expect:
        if after.get("changed") is None:
            return {"met": None, "evidence": "nothing was read back after the action"}
        ok = bool(after["changed"]) == bool(expect["changed"])
        return {"met": ok, "evidence": f"the screen {'changed' if after['changed'] else 'did not change'} as "
                                       f"{'expected' if ok else 'not expected'}"}
    return {"met": None, "evidence": "no expectation to compare"}
```

- [ ] **Step 4: Surface the `after` block on every acting result**

In the gateway or each acting tool, add `result.content["after"] = aftercheck.after_of(result.content.get("facts") or result.content)` for acting kinds (not reads). The cleanest single point: the gateway `_execute`, after the capability runs, for a non-REVERSIBLE kind, compute and attach `after`. Verify the real `facts` location per tool against the survey (`mac.act` nests under `facts`; `browser.act` spreads `r.facts`). For `browser.act` trusted-path results that carry only `{ok, detail}`, `after_of` yields `read="none"`, which is honest: the extension's trusted click reads nothing back.

The `expect` comparison runs in the worker/plan step layer (F6 owns plan steps). For F4, make `aftercheck.compare` available and have the worker, when a success condition carries a `check` of a new kind `after_change` or `after_value` (add to `CHECK_KINDS`), record the comparison as a fact. Keep F4's scope to the uniform `after` block plus the `compare` helper and one condition kind; the full procedure "expect" lines are F6/F8.

- [ ] **Step 5: Run the tests**

Run: `uv run pytest tests/test_after_action.py tests/test_mac_shell.py tests/test_browser_tools.py -q`, then `uv run pytest -q` and `uv run mypy ferb/core`.
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add ferb/act/aftercheck.py ferb/daemon/gateway.py ferb/orchestrator/schemas.py tests/test_after_action.py
git add -u ferb/act
git commit -m "F4 (V2): every acting tool reports a uniform after-action read; expect comparison helper

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

### Task 4: Three distinct statuses — verified / done-but-unverified / failed (V4, V5)

**Files:**
- Create: `ferb/store/migrations/0021_task_status_unverified.sql`
- Modify: `ferb/orchestrator/supervisor.py` `_final` (`supervisor.py:1024-1070`)
- Modify: `ferb/cli/chat.py` (`chat.py:108-119`), `ferb/cli/admin.py` (`ferb stats`/`baseline`, `admin.py:451-550`), `ferb/trace/render.py` (`render.py:10,23-25`), `ferb/orchestrator/episodes.py` / `ferb/trace/compose.py`
- Test: `tests/test_statuses.py`

**Interfaces:**
- Consumes: `_final`'s condition results (`ConditionResult.met`), the worker `Outcome.status`.
- Produces:
  - a new task status `done_unverified`;
  - `_final` returns `done` only when every condition is verified true; `done_unverified` when the worker finished but a check ran and could not confirm; `failed`/`needs_user` otherwise;
  - "unverified" wording appears only when a check actually ran (V5);
  - stats, baseline, chat, episodes and trace distinguish the three.

- [ ] **Step 1: Migration**

`ferb/store/migrations/0021_task_status_unverified.sql`:

```sql
-- F4 (V4): "done but unverified" is its own status, distinct from "failed", so stats and the owner can tell "didn't
-- do it" from "couldn't check it". SQLite can't alter a CHECK in place; rebuild the tasks table's constraint.
-- (Follow the existing 0004_tasks.sql column list exactly; add 'done_unverified' to the CHECK.)
```

Implement the table rebuild: create `tasks_new` with the same columns and indexes as `0004_tasks.sql` but with the status CHECK list extended by `'done_unverified'`, `INSERT INTO tasks_new SELECT * FROM tasks`, drop `tasks`, rename, recreate the `tasks_tree` index. Read `0004_tasks.sql` and any later migration that touched `tasks` so the rebuilt table matches the current shape exactly. Guard with `PRAGMA foreign_keys` as the migration runner requires (check how other table-rebuild migrations do it, if any; otherwise the standard 12-step is fine within one migration).

- [ ] **Step 2: Write the failing tests**

```python
# tests/test_statuses.py
"""done / done_unverified / failed are distinct (V4); 'unverified' only when a check ran (V5)."""
import json

from tests.conftest import RunningDaemon, chat, cond, done, install, sup, task, outcome


def _run(daemon, worker_final, conds):
    install(daemon, [sup("task", "On it.", task("do it", conds=conds))], {"worker": worker_final})
    return outcome(chat(daemon, "do it"))


def test_all_conditions_verified_is_done(daemon: RunningDaemon) -> None:
    evs = _run(daemon, done("did it"), (cond("the file exists"),))
    assert evs["status"] == "done" and evs["verified"] is True


def test_finished_but_a_check_could_not_confirm_is_done_unverified(daemon: RunningDaemon) -> None:
    # The scripted checker returns met=None for the condition (see conftest Script verify branch).
    install(daemon, [sup("task", "On it.", task("do it", conds=(cond("the email was sent"),)))],
            {"worker": done("sent it"), "worker.verify": json.dumps({"results": [{"index": 1, "met": None,
                                                                                  "evidence": "could not open the Sent folder"}]})})
    evs = outcome(chat(daemon, "do it"))
    assert evs["status"] == "done_unverified" and evs["verified"] is False
    assert "unverified" in evs["summary"] and "could not open the Sent folder" in evs["summary"]


def test_a_failed_worker_is_failed_not_unverified(daemon: RunningDaemon) -> None:
    evs = _run(daemon, json.dumps({"status": "failed", "summary": "couldn't find it", "artifacts": [], "evidence": []}),
               (cond("the file exists"),))
    assert evs["status"] == "failed" and "unverified" not in evs["summary"]


def test_no_conditions_is_unverified_only_because_nothing_could_be_checked(daemon: RunningDaemon) -> None:
    evs = _run(daemon, done("done"), ())
    assert evs["status"] in ("done_unverified", "failed")   # never a bare "done ✓"
```

Read the conftest `Script` verifier branch (`tests/conftest.py`), which auto-answers the verify step; this test supplies an explicit `worker.verify` to force `met=None`. Confirm the outcome dict keys (`status`, `verified`, `summary`) match what `chat` streams.

- [ ] **Step 3: Rework `_final`**

In `ferb/orchestrator/supervisor.py` `_final` (`supervisor.py:1024-1070`), split the single `failed` outcome:
- `verified` (unchanged): `out.status == "done"` and conditions and all met True → status `done`.
- **New:** the worker finished (`out.status == "done"`) but at least one condition is `met is None` (a check ran and could not confirm) and none is `met is False` → status `done_unverified`. Owner lines: "I did it, but couldn't confirm everything:" then one line per unconfirmed condition, "— unverified: <evidence>". This is V5: unverified wording only here.
- `failed`: any condition `met is False`, or the worker did not reach `done`, or `tree.stop` (not the owner's stop) → status `failed`, with the real reason; never the word "unverified" unless a check actually ran.
- `needs_user`: unchanged.
- `cancelled`: the owner's stop, unchanged.
- No conditions: if the worker finished, `done_unverified` with "Completion is unverified (no checkable conditions)"; this is honest (a check could not run because there was nothing to check).

Keep R5Q2/P27: the checker's verdict decides the first line; a worker's claim is shown as fact only for a verified condition.

- [ ] **Step 4: Thread the status through the readers**

- `ferb/cli/chat.py:108-119`: show `done ✓`, `done — unverified`, `failed ✗`, distinctly.
- `ferb/cli/admin.py` `stats` (`:515-550`) and `baseline` (`:451-512`): count `done_unverified` separately from `done` and `failed`; `done %` counts only `done`; add an `unverified %`.
- `ferb/trace/render.py` `STATUS` (`render.py:10,23-25`): `done_unverified` → "done (unverified)"; keep "done ✓ verified" for verified.
- `ferb/trace/compose.py` / `episodes`: the episode `status` stores `done_unverified`; `verified` stays the boolean.
- Check `ferb/daemon/main.py` `_task_result` (`:977-986`) and the scheduled-run `ok = status == "done"` (`:1143-1144`): a scheduled follow-up should treat `done_unverified` as not fully done (do not mark the schedule satisfied on unverified). Decide and state it.

- [ ] **Step 5: Run the tests**

Run: `uv run pytest tests/test_statuses.py tests/test_completion_reporting.py tests/test_baseline.py tests/test_trace_render.py tests/test_episodes.py -q`, then `uv run pytest -q` and `uv run mypy ferb/core`.
Expected: PASS. Several existing tests assert the old `failed`-for-unverified behavior; update them to the three-status model and list each.

- [ ] **Step 6: Commit**

```bash
git add ferb/store/migrations/0021_task_status_unverified.sql ferb/orchestrator/supervisor.py ferb/cli/chat.py ferb/cli/admin.py ferb/trace/render.py ferb/trace/compose.py ferb/orchestrator/episodes.py ferb/daemon/main.py tests/test_statuses.py
git add -u tests
git commit -m "F4 (V4, V5): done / done-but-unverified / failed are distinct; unverified only when a check ran

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

### Task 5: Owner-phrased conditions; drop ledger_done as a success check (V6)

**Files:**
- Modify: `ferb/orchestrator/schemas.py` (`CHECK_KINDS`, `:16`), `ferb/orchestrator/verify.py` (`:39-77`), `ferb/orchestrator/prompts/supervisor.md` (`:61-72`), `ferb/orchestrator/prompts/verifier.md`
- Test: `tests/test_conditions.py`

**Interfaces:**
- Consumes: `Checks.code_checkable` / `Checks.run` (`verify.py:38-85`).
- Produces: `ledger_done` is removed from `CHECK_KINDS` and from `Checks`; a condition describes an observable outcome; the planner is told to write conditions in the owner's terms and how they can be observed from the background.

- [ ] **Step 1: Write the failing tests**

```python
# tests/test_conditions.py
"""Conditions describe an observable outcome; ledger_done is gone (V6, V7)."""
import pytest

from ferb.orchestrator.schemas import CHECK_KINDS
from ferb.orchestrator.verify import Checks


def test_ledger_done_is_not_a_check_kind() -> None:
    assert "ledger_done" not in CHECK_KINDS


def test_an_old_ledger_done_condition_is_treated_as_an_observation_not_proof() -> None:
    checks = Checks(ledger=None, urls_of=lambda t: [])
    # A condition that slips through with kind ledger_done (from an older plan) is never code-checked true.
    results = checks.run("t1", [{"text": "the message was sent", "check": {"kind": "ledger_done"}}])
    assert all(r.met is not True for r in results)
```

- [ ] **Step 2: Run the tests to see them fail**

Run: `uv run pytest tests/test_conditions.py -q`
Expected: FAIL: `ledger_done` is still in `CHECK_KINDS`.

- [ ] **Step 3: Remove `ledger_done`**

- `CHECK_KINDS` (`schemas.py:16`): drop `"ledger_done"`. Keep `none`, `file_exists`, `file_contains`, `tab_url`, `account`, and add `after_change`/`after_value` from Task 3 if not already added.
- `Checks.code_checkable` / `run` (`verify.py:39-77`): remove the `ledger_done` branch. An unknown kind (including a stale `ledger_done`) is never code-checkable and goes to the verifier as unknown; it is never returned `met=True` by code.
- `supervisor.md:61-72` and `verifier.md`: rewrite the condition guidance — conditions describe the outcome the owner wants, in the owner's words, and how it can be observed from the background (a file exists, a page shows X, the Sent folder has the message); drop the mention of `ledger_done`. The ledger is for recovery, not proof (Task 6).
- Search for other `ledger_done` references (`test_rework_fixes.py:76`, `test_supervisor.py:194-197`, `test_acceptance_one_worker.py:216`) and update them: a task that used `ledger_done` to "verify" now uses an observable condition or is verified by the checker; preserve each test's intent.

- [ ] **Step 4: Run the tests**

Run: `uv run pytest tests/test_conditions.py tests/test_verify.py tests/test_rework_fixes.py -q` (skip any that don't exist), then `uv run pytest -q` and `uv run mypy ferb/core`.
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add ferb/orchestrator/schemas.py ferb/orchestrator/verify.py ferb/orchestrator/prompts/supervisor.md ferb/orchestrator/prompts/verifier.md tests/test_conditions.py
git add -u tests
git commit -m "F4 (V6, V7): conditions describe observable outcomes; ledger_done dropped as a success check

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

### Task 6: The ledger is for recovery only (V7)

**Files:**
- Modify: `ferb/act/mac/tools.py` `MacAppleScript.describe` (`:376-377`) — a read-only AppleScript is `Kind.READ`, not `Kind.APPLESCRIPT`, so it creates no ledger row
- Create: `ferb/orchestrator/reconcile.py` and an `action.reconcile` observation capability, or extend `TaskEvidence` — a worker can read unresolved-unknown rows and resolve one with evidence
- Modify: `ferb/orchestrator/ledger.py` and `ferb/daemon/main.py` start-up reconcile (`:245-247`)
- Test: `tests/test_ledger_recovery.py`

**Interfaces:**
- Consumes: `Ledger` (`ferb/orchestrator/ledger.py`), the start-up `mark_unknown_after_restart` path (`main.py:245-247`).
- Produces:
  - a read-only AppleScript records no ledger row (its kind is `READ`);
  - `Ledger.unresolved(task_tree) -> list[rows]` and a capability the worker/checker uses to see unresolved-unknown actions;
  - the ledger is documented and used as a recovery record, not proof of success (that is already true after Task 5 drops `ledger_done`).

Note: how a read-only AppleScript is told from a writing one is the hard part. AppleScript can do anything; code cannot parse intent (rule 6). The controller's ruling: keep `MacAppleScript` as `Kind.APPLESCRIPT` (consequential, ledgered) by default, because code cannot prove a script is read-only; instead add an explicit, separate read-only entry point only if one already exists in the codebase. If the survey's "read-only AppleScripts create rows" refers to scripts the agent declared read-only, honor that declared flag; otherwise leave AppleScript consequential and record in the report that V7's "read-only AppleScripts stop creating rows" is deferred because intent can't be code-proven. Do not add a keyword/allowlist that guesses intent.

- [ ] **Step 1: Write the failing tests**

```python
# tests/test_ledger_recovery.py
"""The ledger records what may have happened for recovery, not proof of success (V7)."""
from tests.conftest import RunningDaemon, gateway_setup, gateway_tool_call, FakeCap


def test_unresolved_unknown_rows_are_listed_for_recovery(daemon: RunningDaemon) -> None:
    boom = FakeCap("probe.send", "send", {"probe"}, boom=True)   # raises after "sending": outcome unknown
    grant, _ = gateway_setup(daemon, {"probe"}, None, [boom])
    r = gateway_tool_call(daemon, grant, "probe.send", to="x@y.com")
    assert r["is_error"] and "outcome unknown" in r["content"]["error"]
    rows = daemon.daemon.ledger.unresolved(grant.task_id.split(".", 1)[0])
    assert len(rows) == 1 and rows[0]["state"] == "unknown"
```

The AppleScript-kind behavior is asserted only if the codebase has a declared read-only flag; otherwise write a test documenting that AppleScript stays consequential and skip the kind change. `FakeCap(..., boom=True)` raises `ConnectionError` (conftest).

- [ ] **Step 2: Run the tests to see them fail**

Run: `uv run pytest tests/test_ledger_recovery.py -q`
Expected: FAIL: `Ledger.unresolved` does not exist.

- [ ] **Step 3: Implement**

- Add `Ledger.unresolved(task_tree_id) -> list[dict]`: rows of that tree in state `unknown` or `executing`, for the recovery/P19 path.
- Wire it so the worker and the dispatch see unresolved-unknown actions (the "Uncertain prior actions" context block already does this at `supervisor.py:886-895`; confirm it reads from the ledger, and add a tool to read full rows if needed).
- AppleScript: per the ruling above — only change `describe` if a declared read-only flag exists; otherwise document the deferral.

- [ ] **Step 4: Run the tests**

Run: `uv run pytest tests/test_ledger_recovery.py tests/test_ledger.py -q`, then `uv run pytest -q` and `uv run mypy ferb/core`.
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add ferb/orchestrator/ledger.py ferb/daemon/main.py tests/test_ledger_recovery.py
git add -u ferb/act ferb/orchestrator
git commit -m "F4 (V7): the ledger lists unresolved-unknown actions for recovery; it is not proof of success

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

### Task 7: Never retry an unknown consequential action without the owner (P19)

**Files:**
- Modify: `ferb/daemon/gateway.py` `_execute` (the ledger `intend` call, `gateway.py:266-270`)
- Modify: `ferb/orchestrator/prompts/dispatch.md`, `worker.md` (inspect-and-ask guidance; prompt supports the code boundary, doesn't replace it)
- Test: `tests/test_unknown_retry.py`

**Interfaces:**
- Consumes: `Ledger.intend(..., reconcile_first=...)` and `DuplicateUnresolved` (`ledger.py:29-54`, `gateway.py:266-270`), `Ledger.unresolved` (Task 6).
- Produces:
  - a consequential call (`spec.kind not in REVERSIBLE`) whose same (task, capability, target, payload) already has an unresolved-unknown ledger row is **refused by code**; the error tells the agent to inspect evidence and ask the owner;
  - code never auto-retries; the only way past is the owner approving, which resolves the row.

- [ ] **Step 1: Write the failing tests**

```python
# tests/test_unknown_retry.py
"""A consequential action whose outcome is unknown is never retried without the owner (P19)."""
from tests.conftest import RunningDaemon, gateway_setup, gateway_tool_call, FakeCap


def test_an_unknown_consequential_action_is_not_retried_without_the_owner(daemon: RunningDaemon) -> None:
    boom = FakeCap("probe.pay", "pay", {"probe"}, boom=True)
    grant, _ = gateway_setup(daemon, {"probe"}, None, [boom])
    first = gateway_tool_call(daemon, grant, "probe.pay", to="x@y.com", text="pay $5")
    assert first["is_error"] and "outcome unknown" in first["content"]["error"]
    # The same payment, retried, is refused by code because the prior outcome is unknown.
    second = gateway_tool_call(daemon, grant, "probe.pay", to="x@y.com", text="pay $5")
    assert second["is_error"]
    e = second["content"]["error"].lower()
    assert "unknown" in e and ("owner" in e or "ask" in e or "inspect" in e)
    assert boom.calls == 1          # it never ran a second time


def test_a_reversible_action_is_not_blocked(daemon: RunningDaemon, tmp_path) -> None:
    # A read never gets a ledger row, so it's never blocked by this rule.
    f = daemon.daemon.home / "workspace" / "a.txt"
    f.parent.mkdir(parents=True, exist_ok=True)
    f.write_text("x")
    read = FakeCap("probe.read", "read", {"probe"})
    grant, _ = gateway_setup(daemon, {"probe"}, None, [read])
    assert not gateway_tool_call(daemon, grant, "probe.read")["is_error"]
    assert not gateway_tool_call(daemon, grant, "probe.read")["is_error"]


def test_a_different_target_is_allowed(daemon: RunningDaemon) -> None:
    boom = FakeCap("probe.pay", "pay", {"probe"}, boom=True)
    grant, _ = gateway_setup(daemon, {"probe"}, None, [boom])
    gateway_tool_call(daemon, grant, "probe.pay", to="x@y.com", text="pay $5")
    # A different recipient is a different payment: not the unknown one, so not blocked by P19 (F5 may still confirm).
    other = gateway_tool_call(daemon, grant, "probe.pay", to="z@y.com", text="pay $5")
    assert "outcome unknown" in other["content"]["error"]   # it ran (and raised), not blocked
```

`FakeCap.describe` builds a `Target` from `to`/`text`/`site`; confirm the payload hash differs by recipient (it should, since `payload_hash(action)` includes the target). If `probe.pay`'s kind "pay" isn't a real `Kind`, use a kind the gateway treats as consequential (not in `REVERSIBLE`); `send` works. Keep the name suggestive.

- [ ] **Step 2: Run the tests to see them fail**

Run: `uv run pytest tests/test_unknown_retry.py -q`
Expected: FAIL: the second call runs again (`boom.calls == 2`), because `reconcile_first=False` (`gateway.py:266`).

- [ ] **Step 3: Enforce the boundary**

In `ferb/daemon/gateway.py` `_execute`, change the consequential `intend` to re-enable the duplicate guard:

```python
        intent_id = None
        if spec.kind not in REVERSIBLE:                  # consequential: a ledger row BEFORE it runs (P19)
            try:
                intent_id = self.ledger.intend(grant.task_id.split(".", 1)[0], name, _target_dict(spec.target),
                                                payload_hash(action), reconcile_first=True).id
                self.ledger.mark(intent_id, "executing")
            except DuplicateUnresolved as e:
                error = ("a prior attempt at this exact action has an unknown outcome, so FERB will not repeat it on "
                         "its own (it could happen twice). Inspect the evidence (the order page, a confirmation, the "
                         "Sent folder) and, if it still needs doing, ask the owner before retrying.")
                self.spans.finish(span, error=error)
                self._observed(name, spec, action, grant, "blocked")
                return reply({"error": error, "unresolved": str(e)}, True, {origin.FERB})
```

Notes:
- Use the **tree id** (`grant.task_id.split(".", 1)[0]` or `rec.task.tree_id`) for the ledger key, so a retry in a new worker/verify sub-task still matches the prior unresolved row. Confirm `intend`'s existing `task_id` usage and keep it consistent across `intend`, `mark`, `unresolved` and the start-up reconcile.
- `payload_hash(action)` must already include the target and content, so a different recipient/amount is a different row (the third test). Confirm in `ferb/policy` / wherever `payload_hash` lives.
- The owner-approval path: when the owner approves a retry (via `owner.ask`), the orchestrator resolves the unknown row (`ledger.reconcile(id, evidence=["owner approved retry after inspection"])`) so the next `intend` is not a duplicate. Wire this into the dispatch/worker path minimally: a `reconcile` after an explicit owner "yes, retry". Keep the code boundary firm — only an explicit resolution clears it; nothing auto-clears.
- `dispatch.md` / `worker.md`: add that an unknown-outcome action is not retried by FERB; inspect evidence, then ask the owner. The prompt supports the code boundary; the code enforces it (rule 5).

- [ ] **Step 4: Run the tests**

Run: `uv run pytest tests/test_unknown_retry.py tests/test_ledger.py tests/test_gateway.py tests/test_time.py -q`, then `uv run pytest -q` and `uv run mypy ferb/core`.
Expected: PASS. `tests/test_ledger.py:33-49` already exercises `DuplicateUnresolved`; confirm it still passes. Any test that relied on a silent consequential retry must be updated to the owner-approval flow; list each.

- [ ] **Step 5: Commit**

```bash
git add ferb/daemon/gateway.py ferb/orchestrator/prompts/dispatch.md ferb/orchestrator/prompts/worker.md tests/test_unknown_retry.py
git commit -m "F4 (P19): code refuses to retry a consequential action whose outcome is unknown; the owner decides

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

### Task 8: Recovery copies, durable Trash record, and a snapshot before bulk work (S1)

**Files:**
- Create: `ferb/act/fs/recovery.py` (clone a file before FERB changes it; a durable record; restore)
- Create: `ferb/store/migrations/0023_recovery.sql`
- Modify: `ferb/act/fs/files.py` (`fs.write` overwrite `:167-188`, `fs.edit` `:191-214`, `fs.trash` `:217-237`)
- Modify: `ferb/cli/__init__.py` and `ferb/cli/admin.py` (a `ferb restore` command: list and restore)
- Modify: `ferb/store/retention.py` / `_purge_pass` (purge recovery copies older than `retention.recovery_days`)
- Modify: `ferb/config.py` (`retention.recovery_days = 7`)
- Test: `tests/test_recovery.py`

**Interfaces:**
- Consumes: `clonefile(2)` (via `ctypes` on `libSystem`, or `Foundation` `NSFileManager.copyItemAtURL` as the fallback), the Trash path (`NSFileManager.trashItemAtURL`, `files.py:237`).
- Produces:
  - `recovery.preserve(path: Path, *, reason: str, task_id: str, db, clock, ids, root: Path) -> str | None` — clones `path` into `root/<day>/<id><suffix>`, records a row, returns the copy path (None if the file didn't exist);
  - `recovery.record_trash(original: Path, trashed_to: Path, task_id, db, clock, ids)` — a durable record of a trashed file and where it went;
  - `recovery.restore(record_id, db, root) -> Path` — restore a clone or re-trash lookup;
  - `recovery.purge_before(cutoff, db, root) -> int`;
  - the `recovery` table.

- [ ] **Step 1: Migration**

`ferb/store/migrations/0023_recovery.sql`:

```sql
-- F4 (S1): FERB's own file changes are recoverable for 7 days. Before FERB overwrites, edits or trashes a file it
-- keeps a clone (recovery.recovery_days) and this record; a delete goes to the Trash with its original path kept
-- durably (the span that held it ages out at 30 days, so recovery must not depend on it). This is what makes an
-- overwrite or delete "not destructive" for F5's confirmations.
CREATE TABLE recovery (
    id           TEXT PRIMARY KEY,
    task_id      TEXT,
    kind         TEXT NOT NULL,       -- clone | trash
    original     TEXT NOT NULL,       -- the path FERB changed or deleted
    saved_to     TEXT,                -- the clone's path (clone), or the Trash path (trash)
    reason       TEXT,                -- overwrite | edit | trash
    at           TEXT NOT NULL
);
CREATE INDEX recovery_task ON recovery(task_id);
CREATE INDEX recovery_time ON recovery(at);
```

- [ ] **Step 2: Write the failing tests**

```python
# tests/test_recovery.py
"""FERB's file changes are recoverable for 7 days (S1): a clone before an overwrite/edit, the original path kept for a
trash, restore, and fallback when clonefile is unavailable."""
from datetime import timedelta

from ferb.act.fs import recovery
from tests.conftest import RunningDaemon, gateway_setup, gateway_tool_call


def _write(daemon, grant, path, text):
    return gateway_tool_call(daemon, grant, "fs.write", path=str(path), content=text, overwrite=True, effect="edit")


def test_an_overwrite_keeps_a_recoverable_copy(daemon: RunningDaemon) -> None:
    f = daemon.daemon.home / "workspace" / "notes.txt"
    f.parent.mkdir(parents=True, exist_ok=True)
    f.write_text("the original")
    grant = _fs_grant(daemon)
    _write(daemon, grant, f, "the replacement")
    assert f.read_text() == "the replacement"
    rec = _recovery_rows(daemon)[-1]
    assert rec["kind"] == "clone" and rec["original"] == str(f)
    restored = recovery.restore(rec["id"], daemon.daemon.db, daemon.daemon.home / "recovery")
    assert restored.read_text() == "the original"


def test_a_brand_new_file_keeps_no_copy(daemon: RunningDaemon) -> None:
    f = daemon.daemon.home / "workspace" / "fresh.txt"
    f.parent.mkdir(parents=True, exist_ok=True)
    grant = _fs_grant(daemon)
    _write(daemon, grant, f, "new")
    assert _recovery_rows(daemon) == []          # nothing was overwritten


def test_a_trash_keeps_the_original_path_durably(daemon: RunningDaemon) -> None:
    f = daemon.daemon.home / "workspace" / "old.txt"
    f.parent.mkdir(parents=True, exist_ok=True)
    f.write_text("bye")
    grant = _fs_grant(daemon)
    gateway_tool_call(daemon, grant, "fs.trash", path=str(f), effect="delete")
    rec = _recovery_rows(daemon)[-1]
    assert rec["kind"] == "trash" and rec["original"] == str(f) and rec["saved_to"]


def test_recovery_falls_back_when_clone_is_unavailable(daemon: RunningDaemon, monkeypatch) -> None:
    import ferb.act.fs.recovery as rec_mod
    monkeypatch.setattr(rec_mod, "_clone", lambda src, dst: (_ for _ in ()).throw(OSError("cross-device")))
    f = daemon.daemon.home / "workspace" / "x.txt"
    f.parent.mkdir(parents=True, exist_ok=True)
    f.write_text("before")
    grant = _fs_grant(daemon)
    _write(daemon, grant, f, "after")
    rec = _recovery_rows(daemon)[-1]
    restored = recovery.restore(rec["id"], daemon.daemon.db, daemon.daemon.home / "recovery")
    assert restored.read_text() == "before"       # the copy fallback preserved it


def test_recovery_purges_after_seven_days(daemon: RunningDaemon) -> None:
    f = daemon.daemon.home / "workspace" / "y.txt"
    f.parent.mkdir(parents=True, exist_ok=True)
    f.write_text("a")
    grant = _fs_grant(daemon)
    _write(daemon, grant, f, "b")
    daemon.run(_age_recovery(daemon, (daemon.daemon.clock.now() - timedelta(days=8)).isoformat()))
    n = recovery.purge_before((daemon.daemon.clock.now() - timedelta(days=7)).isoformat(),
                              daemon.daemon.db, daemon.daemon.home / "recovery")
    assert n == 1 and _recovery_rows(daemon) == []


def _fs_grant(daemon):
    # A grant with the fs scopes. Reuse the real fs capabilities, not a fake, so the tool paths run.
    scopes = set().union(*(c.scopes for c in daemon.daemon.gateway.caps.values()
                           if c.name in ("fs.write", "fs.trash")))
    grant, _ = gateway_setup(daemon, scopes, None, [])
    return grant


def _recovery_rows(daemon):
    return [dict(r) for r in daemon.run(_rows(daemon))]


async def _rows(daemon):
    return daemon.daemon.db.conn.execute("SELECT * FROM recovery ORDER BY at").fetchall()


async def _age_recovery(daemon, at):
    daemon.daemon.db.conn.execute("UPDATE recovery SET at = ?", (at,))
```

`_fs_grant` uses the real `fs.write`/`fs.trash` capabilities; check how `gateway_setup` registers capabilities vs. the daemon's own registry, and adapt so the real fs tools are reachable with the grant (the daemon already registers them; you may just need a grant with their scopes rather than registering fakes). If `fs.trash` is `Kind.TRASH` (reversible, no ledger), that's fine; the recovery record is separate from the ledger.

- [ ] **Step 3: Run the tests to see them fail**

Run: `uv run pytest tests/test_recovery.py -q`
Expected: FAIL: `ferb.act.fs.recovery` doesn't exist, and `fs.write` keeps no copy.

- [ ] **Step 4: Implement `ferb/act/fs/recovery.py`**

```python
"""Owns S1 recovery: before FERB overwrites, edits or deletes a file, keep a clone (or a copy, cross-volume) for 7
days, and a durable record of where a trashed file went. This is what makes FERB's file changes recoverable, so F5
treats them as not destructive. Must not lose the original: a clone failure falls back to a byte copy, never silently
skips."""

import ctypes
import shutil
from pathlib import Path
from typing import Any

_libc = ctypes.CDLL("libSystem.dylib", use_errno=True)


def _clone(src: Path, dst: Path) -> None:
    """APFS clonefile(2): a copy-on-write copy, instant and cheap on the same volume. Raises OSError otherwise."""
    rc = _libc.clonefile(str(src).encode(), str(dst).encode(), 0)
    if rc != 0:
        raise OSError(ctypes.get_errno(), "clonefile failed")


def preserve(path: Path, *, reason: str, task_id: str | None, db: Any, clock: Any, ids: Any, root: Path) -> str | None:
    if not path.exists() or path.is_dir():
        return None                                      # nothing to preserve (a brand-new file, or a directory)
    eid = ids.new_id()
    day = clock.now().astimezone().date().isoformat()
    (root / day).mkdir(parents=True, exist_ok=True)
    dst = root / day / f"{eid}{path.suffix}"
    try:
        _clone(path, dst)
    except OSError:
        shutil.copy2(path, dst)                          # cross-volume or non-APFS: a plain copy still recovers it
    db.conn.execute("INSERT INTO recovery (id, task_id, kind, original, saved_to, reason, at) VALUES (?,?,?,?,?,?,?)",
                    (eid, task_id, "clone", str(path), str(dst), reason, clock.now().isoformat()))
    return str(dst)


def record_trash(original: Path, trashed_to: str | None, *, task_id: str | None, db: Any, clock: Any, ids: Any) -> str:
    eid = ids.new_id()
    db.conn.execute("INSERT INTO recovery (id, task_id, kind, original, saved_to, reason, at) VALUES (?,?,?,?,?,?,?)",
                    (eid, task_id, "trash", str(original), trashed_to, "trash", clock.now().isoformat()))
    return eid


def restore(record_id: str, db: Any, root: Path) -> Path:
    r = db.conn.execute("SELECT * FROM recovery WHERE id = ?", (record_id,)).fetchone()
    if r is None:
        raise KeyError(record_id)
    original = Path(r["original"])
    if r["kind"] == "clone":
        shutil.copy2(Path(r["saved_to"]), original)      # put the preserved contents back
    else:                                                # trash: move it back from the Trash if still there
        saved = Path(r["saved_to"]) if r["saved_to"] else None
        if saved and saved.exists():
            shutil.move(str(saved), str(original))
    return original


def purge_before(cutoff_iso: str, db: Any, root: Path) -> int:
    rows = db.conn.execute("SELECT id, kind, saved_to FROM recovery WHERE at < ?", (cutoff_iso,)).fetchall()
    for r in rows:
        if r["kind"] == "clone" and r["saved_to"]:
            Path(r["saved_to"]).unlink(missing_ok=True)
    db.conn.execute("DELETE FROM recovery WHERE at < ?", (cutoff_iso,))
    return len(rows)
```

- [ ] **Step 5: Hook it into the fs tools**

`ToolContext` (`ferb/core/types.py:178-189`) carries no db/clock/ids, so the fs tools can't reach the recovery store directly. **Do the preserve in the gateway**, which already holds the stores: in `_execute`, before running a capability whose kind is `WRITE_FILE` or `TRASH` and whose `spec.target.path` exists, call `recovery.preserve`/`record_trash` keyed off `spec.kind` and `spec.target.path`. This keeps one seam for all file-changing tools and needs no `ToolContext` change.
- Write/edit (kind `WRITE_FILE`, `files.py:167-214`): if the target path exists and is a file, `recovery.preserve(path, reason="overwrite"|"edit", task_id=rec.task.tree_id, ...)` before the capability runs. (The reason can be the capability name: `fs.write` → overwrite, `fs.edit` → edit.) A brand-new file preserves nothing (`preserve` returns None).
- Trash (kind `TRASH`, `files.py:217-237`): after the call succeeds, read the result's `trashed_to` and call `recovery.record_trash(original_path, trashed_to, ...)`.
- The reply/span gains `"recoverable": <record_id>` so the trace and the owner can see it.
- Confirm `spec.kind` values against `ferb/policy/rules.py` `Kind` (`WRITE_FILE`, `TRASH`) and that `spec.target.path` is the normalized path.

- [ ] **Step 6: Snapshot before bulk or destructive work, retention, and `ferb restore`**

- Add `recovery.local_snapshot() -> str | None`: run `tmutil localsnapshot` and return the snapshot name, or None if it needs rights FERB doesn't have. This is a build-time experiment: run it, record whether it needs admin rights, and wire it to be taken once before a worker does bulk or destructive file work (the orchestrator can request it before a fan-out delete; keep it best-effort and background). Document the admin-rights finding in the report. If it needs admin rights FERB doesn't have, record that and rely on the per-file clones plus Trash, which already recover individual files.
- Retention: in `_purge_pass`, call `recovery.purge_before((now - timedelta(days=recovery_days)).isoformat(), ...)` and add the count to `retention.purged`.
- `ferb restore`: `ferb restore` lists recent recovery records (id, original, reason, when); `ferb restore <id>` restores it. Add to `ferb/cli/__init__.py` and `admin.py`, owner-facing wording (P27): "Restored ~/notes.txt to what it was before FERB changed it."

- [ ] **Step 7: Run the tests**

Run: `uv run pytest tests/test_recovery.py tests/test_fs.py -q` (adjust the fs test module name), then `uv run pytest -q` and `uv run mypy ferb/core`.
Expected: PASS.

- [ ] **Step 8: Commit**

```bash
git add ferb/act/fs/recovery.py ferb/store/migrations/0023_recovery.sql ferb/act/fs/files.py ferb/cli/__init__.py ferb/cli/admin.py ferb/store/retention.py ferb/daemon/main.py ferb/config.py tests/test_recovery.py
git commit -m "F4 (S1): clone a file before FERB changes it, keep the Trash path durably, restore; snapshot before bulk work

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

### Task 9: A named target is met only by evidence about that target (D34)

**Files:**
- Modify: `ferb/orchestrator/supervisor.py` (pass the named targets into the verifier context explicitly)
- Modify: `ferb/orchestrator/prompts/verifier.md` (tie each condition's evidence to the named target; a substitute is not the target)
- Modify: `ferb/orchestrator/verify.py` or `_final` (a condition that names a target FERB could not reach resolves unverified/failed, never done)
- Test: `tests/test_named_target.py`

**Interfaces:**
- Consumes: the task's `Targets` (`ferb/policy/targets.py`), the "What the owner named —" block (`supervisor.py:899-903`), the verifier run.
- Produces: the verifier receives the named targets; its result for a condition that names a target must cite evidence about that target; code keeps a substitute from satisfying the condition.

- [ ] **Step 1: Write the failing test**

```python
# tests/test_named_target.py
"""A success condition that names a target is met only by evidence about that target, never a substitute (D34).
The 'Code Astra' live case: a worker that used VS Code Copilot must not verify 'started Codex with the astra model'."""
import json

from tests.conftest import RunningDaemon, chat, cond, install, sup, task, outcome


def test_a_substitute_does_not_satisfy_a_named_target_condition(daemon: RunningDaemon) -> None:
    # The worker claims success; the checker, given the named target, reports the evidence is about a different tool.
    conds = (cond("a Codex session is running with the astra model"),)
    install(daemon, [sup("task", "On it.", task("start a codex astra session", apps=("codex",), conds=conds))],
            {"worker": json.dumps({"status": "done", "summary": "I started it in VS Code", "artifacts": [],
                                   "evidence": []}),
             "worker.verify": json.dumps({"results": [{"index": 1, "met": False,
                                                       "evidence": "the running agent is VS Code Copilot, not Codex "
                                                                   "with the astra model"}]})})
    evs = outcome(chat(daemon, "start a codex astra session"))
    assert evs["status"] == "failed" and evs["verified"] is not True
    assert "Codex" in evs["summary"] or "not" in evs["summary"]


def test_the_named_targets_reach_the_checker(daemon: RunningDaemon) -> None:
    conds = (cond("the message reached Asmit"),)
    script = install(daemon, [sup("task", "On it.", task("message Asmit", recipients=("Asmit",), conds=conds))],
                     {"worker": __import__("tests.conftest", fromlist=["done"]).done("sent")})
    chat(daemon, "message Asmit")
    verify_ctx = next(c for mode, key, c, _ in script.contexts if key.endswith(".verify"))
    assert "Asmit" in verify_ctx          # the checker was told the named target
```

Confirm `task(..., recipients=...)` and `apps=...` populate `Targets`, and that the verifier context (`Script.contexts`) is captured as F3's tests do.

- [ ] **Step 2: Run the tests to see them fail**

Run: `uv run pytest tests/test_named_target.py -q`
Expected: FAIL: the named targets are not in the verifier context, or a substitute still verifies.

- [ ] **Step 3: Implement**

- In `ferb/orchestrator/supervisor.py` `verify` / `_context` for the verifier: include the owner's named targets (apps, sites, people, accounts, files) explicitly, labeled "The owner named these; evidence must be about them, not a substitute." The "What the owner named —" block already exists (`:899-903`); make sure it reaches the verifier context (it is built in `_context`, so it should; confirm and add if missing).
- `verifier.md`: add that a condition naming a target is met only by evidence about that exact target; a familiar or similar tool/app/person is not the target; if the evidence is about a substitute, the condition is `met=False` with evidence naming what was actually used.
- Code tie-in: the existing flow already lets the checker return `met=False`; the test's main point is that (a) the checker is given the named targets and (b) a `met=False` substitute produces `failed`, not `done`. With Task 4's status split, a `met=False` is `failed`. No new code gate is needed beyond passing the targets; if the survey shows the named targets don't reach the verifier, wire them in. Do not add a keyword matcher (rule 6): the verifier judges; code only ensures it has the named targets and that a substitute can't slip through as `done`.

- [ ] **Step 4: Run the tests**

Run: `uv run pytest tests/test_named_target.py tests/test_supervisor.py -q`, then `uv run pytest -q` and `uv run mypy ferb/core`.
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add ferb/orchestrator/supervisor.py ferb/orchestrator/prompts/verifier.md ferb/orchestrator/verify.py tests/test_named_target.py
git commit -m "F4 (D34): the checker gets the owner's named targets; a substitute never satisfies a named-target condition

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

### Task 10: Browser downloads become facts, never a dialog (D35)

**Files:**
- Modify: `extensions/chromium/manifest.json` (version → `0.14.0`; add `downloads` permission if the chosen mechanism needs it), `extensions/chromium/background.js`, `extensions/chromium/protocol.js` (`FERB_PROTOCOL` → 10; allow `Browser.setDownloadBehavior` / `Page.setDownloadBehavior` if using CDP)
- Modify: `ferb/act/browser/ext_protocol.py` (`PROTOCOL_VERSION` → 10, `MIN_EXTENSION_VERSION` → `0.14.0`), `ferb/act/browser/ext_bridge.py` (receive a `download` event; take the file into the workspace with facts)
- Modify: `ferb/act/browser/tools.py` (a download appears to the worker as facts: path, size, sha256, source URL, origin; never a dialog)
- Create: `ferb/store/migrations/0025_download.sql` (optional: a downloads record; or reuse evidence/recovery — decide)
- Test: `tests/test_downloads.py` (offline, with a fake extension bridge), and extend the extension's own tests if present
- Modify: `README.md` / setup notes (the extension must be re-published; the owner runs `ferb setup --update`)

**Interfaces:**
- Consumes: the extension event path (`send("event", …)`, `Hub._handle`, `ext_bridge.py:320-334`), the untrusted `~/Downloads` handling (`main.py:438`), the origin vocabulary (F3; a download is `external:download`).
- Produces:
  - the extension suppresses the native download UI and streams the file to FERB;
  - `ext_bridge` receives a `download` event and writes the bytes into `~/.ferb/workspace/downloads/<id>-<name>`, records facts;
  - the worker sees `{path, name, bytes, sha256, source_url, origin: "external:download"}` as a tool result, no dialog;
  - protocol 10, min extension `0.14.0`.

Note: the mechanism (CDP `setDownloadBehavior` with a FERB directory and `Browser.downloadProgress`, vs the `chrome.downloads` API) is a build-time experiment (D35). Run the experiment, choose one, and record the choice and why in the report. The debugger/CDP path keeps all download handling inside the already-attached debugger session and needs no new broad permission; the `downloads` API needs the `downloads` permission but is simpler. Prefer the CDP path unless it can't suppress the native dialog reliably; document the finding. Either way, the file never prompts the owner (D35) and never lands in the owner's own `~/Downloads` where FERB would treat it as untrusted ambient data.

- [ ] **Step 1: Write the failing test (offline, fake bridge)**

```python
# tests/test_downloads.py
"""A browser download becomes facts in FERB's workspace, never a dialog on the owner's screen (D35)."""
import hashlib

from tests.conftest import RunningDaemon


def test_a_download_event_becomes_a_workspace_file_with_facts(daemon: RunningDaemon) -> None:
    payload = b"report body bytes"
    # Simulate the extension delivering a finished download to the bridge.
    rec = daemon.run(_deliver_download(daemon, name="report.pdf", data=payload, source_url="https://x.example/r.pdf"))
    from pathlib import Path
    saved = Path(rec["path"])
    assert saved.exists() and saved.read_bytes() == payload
    assert str(saved).startswith(str(daemon.daemon.home / "workspace" / "downloads"))
    assert rec["bytes"] == len(payload) and rec["sha256"] == hashlib.sha256(payload).hexdigest()
    assert rec["origin"] == "external:download" and rec["source_url"] == "https://x.example/r.pdf"
    assert rec["name"] == "report.pdf"


def test_a_download_never_lands_in_the_owner_Downloads(daemon: RunningDaemon) -> None:
    rec = daemon.run(_deliver_download(daemon, name="x.zip", data=b"z", source_url="https://x/x.zip"))
    from pathlib import Path
    assert "Downloads" not in str(Path(rec["path"]).parent) or ".ferb" in str(rec["path"])
```

`_deliver_download` calls the bridge's download handler directly with a fake event payload (the extension side is tested in the extension's own harness if one exists). Read `ext_bridge.py` for how events arrive and build the smallest real path: the handler that takes `{name, bytes_b64, source_url}`, writes the file, returns the facts. If the extension streams in chunks, model one chunk.

- [ ] **Step 2: Run the tests to see them fail**

Run: `uv run pytest tests/test_downloads.py -q`
Expected: FAIL: there is no download handler.

- [ ] **Step 3: Implement the FERB side**

- `ext_bridge.py`: handle a `download` event. Collect the bytes (base64 or a chunked transfer like `command_part`/`result_part`, `background.js`), write to `~/.ferb/workspace/downloads/<id>-<sanitized name>`, compute sha256, and record `{id, path, name, bytes, sha256, source_url, origin: "external:download", at}`. Expose it to the worker as a tool result (a `browser.downloads`/`download.take` capability, or as part of the navigation result that triggered it — decide the cleaner shape and state it). Sanitize the filename (no path separators, no leading dots).
- `ext_protocol.py`: `PROTOCOL_VERSION = 10`, `MIN_EXTENSION_VERSION = "0.14.0"`.
- Keep it background: no dialog, no window to the front.

- [ ] **Step 4: Implement the extension side**

- `protocol.js`: `FERB_PROTOCOL = 10`. If using CDP, add `Browser.setDownloadBehavior`/`Page.setDownloadBehavior` and `Browser.getDownloadProgress`-style events to the allowlist (`protocol.js:23-60`).
- `background.js`: on a navigation or click that starts a download, set the download behavior to deliver into FERB's handling (CDP `allowAndName` with a FERB directory, or intercept via `chrome.downloads.onCreated` + `onChanged` if using the API), suppress the native UI, read the finished file's bytes, and `send("event", {type:"download", name, source_url, bytes_b64 | chunked})`. Never show the Save dialog.
- `manifest.json`: version `0.14.0`; add `"downloads"` to permissions only if the chosen mechanism needs it.
- Update all three version numbers together (rule): manifest `version`, `protocol.js` `FERB_PROTOCOL`, Python `PROTOCOL_VERSION`/`MIN_EXTENSION_VERSION`. The hub's mismatch refusal (`ext_bridge.py:289-297`) then tells an old extension to update.

- [ ] **Step 5: Run the tests**

Run: `uv run pytest tests/test_downloads.py tests/test_ext_bridge.py tests/test_browser_tools.py -q`, then `uv run pytest -q` and `uv run mypy ferb/core`.
Expected: PASS. If there is a JS test harness for the extension, extend it; otherwise note that the extension side needs a live check (Task 11's live list covers it).

- [ ] **Step 6: Commit**

```bash
git add extensions/chromium ferb/act/browser/ext_protocol.py ferb/act/browser/ext_bridge.py ferb/act/browser/tools.py ferb/store/migrations/0025_download.sql README.md tests/test_downloads.py
git commit -m "F4 (D35): a browser download becomes workspace facts (path, size, hash, source, origin), never a dialog; protocol 10

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

### Task 11: Documentation, registry and the live-check list

**Files:**
- Modify: `ARCHITECTURE.md`, `README.md`, `CHANGELOG.md`, `docs/0.27_DECISIONS.toml`, `docs/OWNER_ACCEPTANCE.md`

- [ ] **Step 1: Write the docs, checking every name against the code at HEAD**

- **`ARCHITECTURE.md`**: a "Verification and recovery (F4)" section:
  - the three statuses and when each is set, and that "unverified" appears only when a check ran;
  - the read-only checker, code-enforced (the `observation` attribute and the observe-only grant);
  - after-action reads and the `after` block;
  - the evidence store (what it holds, 7-day retention, secret-field masking, that F5 adds full screenshot redaction);
  - app-window screenshots (D26, D29);
  - the ledger as a recovery record, `ledger_done` gone;
  - P19: code refuses to retry an unknown consequential action;
  - S1 recovery: clone copies, the durable Trash record, the snapshot, `ferb restore`;
  - named-target verification;
  - downloads as facts and the protocol bump.
  - Note the F5 residual: full screenshot redaction, and the destructive-action/sink confirmations that build on S1.
- **`README.md`**: the three statuses in `ferb trace`/`ferb stats`; `ferb restore`; the retention keys (`retention.evidence_days`, `retention.recovery_days`); that the extension must be re-published (`ferb setup --update`) for downloads.
- **`CHANGELOG.md`**: `## 0.27.0-dev — <date> — F4 verification and recovery`, above the F3 entry, one bullet per task with decision IDs.
- **`docs/0.27_DECISIONS.toml`** tests (only files that exist at HEAD):
  - D20 → `tests/test_evidence_store.py`, `tests/test_statuses.py`;
  - P16 → `tests/test_statuses.py`, `tests/test_checker_readonly.py`, `tests/test_after_action.py`, `tests/test_conditions.py`;
  - D26, D29 → `tests/test_evidence_store.py`;
  - P19 → `tests/test_unknown_retry.py`;
  - S1 → `tests/test_recovery.py`;
  - D34 → `tests/test_named_target.py`;
  - D35 → `tests/test_downloads.py`;
  - D6 → the perception-screenshot test in `tests/test_evidence_store.py`.
- **`docs/OWNER_ACCEPTANCE.md`**: "0.27 F4 — pending live checks" (the list below).

- [ ] **Step 2: Run the checks**

Run: `uv run pytest tests/test_decision_coverage.py -q`, then `uv run pytest -q`, `uv run mypy ferb/core` and `git diff --check`.
Expected: PASS.

- [ ] **Step 3: Commit**

```bash
git add ARCHITECTURE.md README.md CHANGELOG.md docs/0.27_DECISIONS.toml docs/OWNER_ACCEPTANCE.md
git commit -m "F4 (V1-V8, P19, S1, D26, D29, D34, D35): architecture, changelog, registry tests, live-check list

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

## Live acceptance (the controller with the owner, after the final review and `ferb setup --update`)

These are real scenarios from FERB's history (D17), on owner-controlled surfaces.

1. **Three statuses.** A task that clearly succeeds reads `done ✓ verified`. A task where the checker can't confirm reads `done — unverified` with the real reason, not `failed`. A task that fails reads `failed` with the real reason. Check `ferb trace` and `ferb stats`.
2. **Read-only checker.** A task whose check would need a click leaves the condition unverified (the checker cannot click); a follow-up worker gathers it. No send/write/click is ever made by the checker.
3. **Brief state captured.** A task that produces a toast or a short-lived confirmation (a WhatsApp "message yourself", a Spotify like) — the checker confirms it from stored evidence even though the toast is gone. `ferb trace --json` shows the evidence id.
4. **Recovery.** Have FERB overwrite a disposable file, then `ferb restore <id>` brings back the original. Have FERB trash a disposable file; the record shows the original path and the Trash location.
5. **Unknown-outcome.** Simulate an interrupted send (stop mid-send, or restart), then ask FERB to continue; it must not silently resend — it reports what's unknown and asks.
6. **Named target.** The "Code Astra" case: ask for something FERB can't find; it asks which, never reports a substitute as done.
7. **Download.** On a kartikey.xyz page, trigger a download; no Save dialog appears, and the worker reports the file in FERB's workspace with size and hash. (The extension was re-published by `ferb setup --update`.)
8. **Verdicts.** Record each with `ferb verdict TASK pass|fail`.

## Self-Review

**Spec coverage:**
- V1 → Task 1 (evidence store, 7-day media). V2 → Task 3 (after-action reads). V3 → Task 2 (code-enforced read-only checker). V4 → Task 4 (three statuses). V5 → Task 4 ("unverified" only when a check ran). V6 → Task 5 (owner-phrased conditions, drop `ledger_done`). V7 → Tasks 5 and 6 (ledger for recovery only). V8 → Task 1 + Task 4: memory counts a success only with the verified flag and stored evidence; the actual learning/procedure store is F8, so V8's consumer is F8 (noted; F4 provides the verified+evidence record episodes already carry).
- P19 → Task 7. S1 → Task 8. D6 → Task 1 (app-start capture; no continuous capture). D20 → Tasks 1, 4. D26, D29 → Task 1. D34 → Task 9. D35 → Task 10.
- B4 exit ("the Spotify and WhatsApp tasks verify correctly; fresh baseline") → live acceptance 1–3, plus `ferb baseline` after.

**Not in F4, on purpose:**
- Full screenshot redaction — OTP digits, notification-matched codes, CSS-masked fields (F5, D8/D28/P18/D13). F4's secret-field masking is the interim floor.
- The destructive-action confirmations and the sink rule (F5, D23/S4). F4 builds S1 recovery, which F5's "recoverable ⇒ not destructive" rule depends on, and P19, which is a separate code boundary.
- Plan step types, the full procedure "expect" comparison, standing permission (F6, D30).
- Procedures and learned skills, and counting a success for learning (F8). V8's record exists here; its consumer is F8.
- Clipboard/typed-text retention, media 7-day for recordings beyond what Task 1 covers (F7).

**Decisions baked in (flag these at handoff):**
- **Screenshots stored with interim secret-field masking in F4**, full redaction in F5. The alternative was to store only snapshots and recordings in F4 and defer all screenshots to F5. Chosen the former because D20 says screenshots are kept 7 days and the checker needs brief-state images; the masking bounds the exposure, and the files never leave the owner's Mac.
- **AppleScript stays consequential (ledgered)** unless a declared read-only flag already exists, because code cannot prove a script is read-only without judging intent (rule 6). V7's "read-only AppleScripts stop creating rows" is honored only for a declared flag; otherwise deferred with a note.
- **P19 is a code refusal plus owner-approved reconcile**, reusing the existing ledger duplicate guard and `owner.ask`, rather than F5's full confirmation framework. The two are different triggers (P19 = unknown-outcome retry; F5 = destructive/sink), so this is consistent with the plan placing P19 in F4 and confirmations in F5.
- **Downloads mechanism (CDP vs `chrome.downloads`)** is a build-time experiment in Task 10; the implementer records the choice.

**Placeholder scan:** code steps carry code. The points that depend on exact current shapes name the file:line to read and the rule (never guess): the `tasks` table rebuild (0004 + any later migration), the token/grant shape for `observe_only` (`ferb/daemon/auth.py`), each tool's `facts` location for the `after` block, `payload_hash`'s inputs for P19, `ToolContext`'s contents for the recovery seam, and the extension event/transfer path for downloads.

**Type consistency:**
- `EvidenceStore.put/list/read/purge_before` and `mask.mask_secret_fields/pixel` (Task 1);
- `RunGrant.observe_only`, capability `observation`, `Tokens.issue(observe_only=)` (Task 2);
- `aftercheck.after_of/compare` and the `after` block shape (Task 3);
- the `done_unverified` status string (Task 4), used identically in render/compose/stats/baseline/chat;
- `CHECK_KINDS` without `ledger_done` (Task 5);
- `Ledger.unresolved` (Task 6), consumed by Task 7;
- `recovery.preserve/record_trash/restore/purge_before/local_snapshot` (Task 8);
- the named-targets verifier context (Task 9);
- the download facts shape `{path, name, bytes, sha256, source_url, origin}` (Task 10).

**Review Focus:** each of the five lines has its test in the owning task (Tasks 1, 2, 7, 8, 9).
