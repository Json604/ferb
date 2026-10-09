"""Owns FERB's evidence store (V1, D20): captured state the final checker can re-inspect, kept 7 days. Must not hold a
secret value: text passes through the redactor, screenshots have their accessibility-known secret fields masked before
they are written (ferb/act/evidence/mask.py), and an image whose fields can't be masked is never written. Files live
under ~/.ferb/evidence, never sent anywhere; this is the owner's own Mac."""

import asyncio
import json
import os
from pathlib import Path
from typing import Any, Callable

from ferb.act.evidence import mask

EXT = {"image/jpeg": "jpg", "video/mp4": "mp4"}
UNMASKABLE = "its secret fields couldn't be masked"


class EvidenceStore:
    def __init__(self, root: Path, clock: Any, ids: Any, redact: Callable[[Any], Any], db: Any) -> None:
        self.root, self.clock, self.ids, self.redact, self.db = root, clock, ids, redact, db
        self.root.mkdir(mode=0o700, parents=True, exist_ok=True)

    def put(self, task_id: str, span_id: str | None, kind: str, *, image: bytes | None = None,
            data: dict[str, Any] | None = None, rects: list[mask.Box] | None = None,
            mime: str = "image/jpeg", origin: str) -> str:
        """One capture: the image (masked first when `rects` name secret fields) under root/<day>/<id>.<ext>, and its
        row. An image that can't be masked is dropped and the row says so (data only). Every row says where its
        content came from (`origin`, ferb/core/origin.py: MP2), so it has no default. On the event loop, use
        `put_async`: masking decodes and re-encodes the image, and the file is written to disk."""
        if image is not None and rects:
            try:
                image = mask.mask_secret_fields(image, rects)       # opaque over AX-known secret fields
            except ValueError:
                image, data = None, {**(data or {}), "image_withheld": UNMASKABLE}
        eid, at, payload, rel = self._plan(image, data, mime)
        if rel is not None and image is not None:
            _write_file(self.root / rel, image)
        return self._insert(eid, at, task_id, span_id, kind, mime, rel, payload, origin)

    async def put_async(self, task_id: str, span_id: str | None, kind: str, *, image: bytes | None = None,
                        data: dict[str, Any] | None = None, rects: list[mask.Box] | None = None,
                        mime: str = "image/jpeg", origin: str) -> str:
        """`put` for the daemon's event loop: the masking (decode, fill, encode) and the file write run in a worker
        thread; the row is written on the loop, which owns the database connection, right after the write (no await
        between them). Cancelled during the write, it waits for the thread (which writes on regardless), removes the
        file and re-raises: never a file without its row, which retention could never find."""
        if image is not None and rects:
            try:
                image = await asyncio.to_thread(mask.mask_secret_fields, image, rects)
            except ValueError:
                image, data = None, {**(data or {}), "image_withheld": UNMASKABLE}
        eid, at, payload, rel = self._plan(image, data, mime)
        if rel is not None and image is not None:
            path = self.root / rel
            write = asyncio.ensure_future(asyncio.to_thread(_write_file, path, image))
            try:
                await asyncio.shield(write)
            except asyncio.CancelledError:
                await _ended(write)
                path.unlink(missing_ok=True)
                raise
        return self._insert(eid, at, task_id, span_id, kind, mime, rel, payload, origin)

    def _plan(self, image: bytes | None, data: dict[str, Any] | None,
              mime: str) -> tuple[str, Any, str | None, str | None]:
        """The capture's id, time, redacted facts and file name. Everything that can fail before the row is written
        runs before the file is (no file without its row)."""
        eid = str(self.ids.new_id())
        at = self.clock.now()
        payload = json.dumps(self.redact(data), ensure_ascii=False, default=str) if data is not None else None
        rel = f"{at.astimezone().date().isoformat()}/{eid}.{EXT.get(mime, 'bin')}" if image is not None else None
        return eid, at, payload, rel

    def _insert(self, eid: str, at: Any, task_id: str, span_id: str | None, kind: str, mime: str, rel: str | None,
                payload: str | None, origin: str) -> str:
        try:
            self.db.conn.execute(
                "INSERT INTO evidence (id, task_id, span_id, kind, mime, path, data_json, origin, captured_at) "
                "VALUES (?,?,?,?,?,?,?,?,?)",
                (eid, task_id, span_id, kind, mime if rel else None, rel, payload, origin, at.isoformat()))
        except BaseException:
            if rel:                                                 # never a file without its row
                (self.root / rel).unlink(missing_ok=True)
            raise
        return eid

    def list(self, task_id: str) -> list[dict[str, Any]]:
        rows = self.db.conn.execute(
            "SELECT * FROM evidence WHERE task_id = ? ORDER BY captured_at, rowid", (task_id,))
        return [_meta(r) | {"path": r["path"]} for r in rows]

    def read(self, evidence_id: str) -> tuple[dict[str, Any], bytes | None]:
        r = self.db.conn.execute("SELECT * FROM evidence WHERE id = ?", (evidence_id,)).fetchone()
        if r is None:
            raise KeyError(evidence_id)
        path = self._inside(r["path"])
        return _meta(r), path.read_bytes() if path is not None else None

    def purge_before(self, cutoff_iso: str) -> int:
        """Delete captures older than the cutoff, file first: a file that can't be deleted keeps its row for the next
        pass, so no row ever points at a missing file. Empty day folders go too."""
        rows = self.db.conn.execute("SELECT id, path FROM evidence WHERE captured_at < ?", (cutoff_iso,)).fetchall()
        gone: list[str] = []
        for r in rows:
            path = self._inside(r["path"])
            if path is not None:                                    # a path outside the store is never deleted
                try:
                    path.unlink(missing_ok=True)
                except OSError:
                    continue
            gone.append(r["id"])
        for i in range(0, len(gone), 500):
            chunk = gone[i:i + 500]
            self.db.conn.execute(f"DELETE FROM evidence WHERE id IN ({','.join('?' * len(chunk))})", chunk)
        try:                                                        # tidying only: never fails the pass
            for day in self.root.iterdir():
                if day.is_dir() and not any(day.iterdir()):
                    day.rmdir()
        except OSError:
            pass
        return len(gone)

    def _inside(self, rel: str | None) -> Path | None:
        """The row's file, only when it resolves inside the store (symlinks followed); else None."""
        if not rel:
            return None
        path = (self.root / rel).resolve()
        return path if path.is_relative_to(self.root.resolve()) else None


async def _ended(job: "asyncio.Future[None]") -> None:
    """Wait until a worker-thread job has ended, whatever cancels the waiter meanwhile (a thread can't be stopped)."""
    while not job.done():
        try:
            await asyncio.wait({job})
        except asyncio.CancelledError:
            continue
    if not job.cancelled():
        job.exception()                                             # retrieved: the caller is being cancelled anyway


def _write_file(path: Path, image: bytes) -> None:
    """The capture's file, created new (never over another), owner-only, in an owner-only day folder."""
    path.parent.mkdir(mode=0o700, exist_ok=True)
    fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    with os.fdopen(fd, "wb") as f:
        f.write(image)


def _meta(r: Any) -> dict[str, Any]:
    return {"id": r["id"], "task_id": r["task_id"], "span_id": r["span_id"], "kind": r["kind"], "mime": r["mime"],
            "origin": r["origin"], "captured_at": r["captured_at"], "has_image": r["path"] is not None,
            "data": json.loads(r["data_json"]) if r["data_json"] else None}
