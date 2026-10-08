"""Owns FERB's evidence store (V1, D20): captured state the final checker can re-inspect, kept 7 days. Must not hold a
secret value: text passes through the redactor, screenshots have their accessibility-known secret fields masked before
they are written (ferb/act/evidence/mask.py), and an image whose fields can't be masked is never written. Files live
under ~/.ferb/evidence, never sent anywhere; this is the owner's own Mac."""

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
            mime: str = "image/jpeg", origin: str = "ferb") -> str:
        """One capture: the image (masked first when `rects` name secret fields) under root/<day>/<id>.<ext>, and its
        row. An image that can't be masked is dropped and the row says so (data only)."""
        eid = str(self.ids.new_id())
        at = self.clock.now()
        if image is not None and rects:
            try:
                image = mask.mask_secret_fields(image, rects)       # opaque over AX-known secret fields
            except ValueError:
                image, data = None, {**(data or {}), "image_withheld": UNMASKABLE}
        rel = None
        if image is not None:
            day = at.astimezone().date().isoformat()
            (self.root / day).mkdir(mode=0o700, exist_ok=True)
            rel = f"{day}/{eid}.{EXT.get(mime, 'bin')}"
            fd = os.open(self.root / rel, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
            with os.fdopen(fd, "wb") as f:
                f.write(image)
        payload = json.dumps(self.redact(data), ensure_ascii=False, default=str) if data is not None else None
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
        return _meta(r), (self.root / r["path"]).read_bytes() if r["path"] else None

    def purge_before(self, cutoff_iso: str) -> int:
        """Delete captures older than the cutoff, file first: a file that can't be deleted keeps its row for the next
        pass, so no row ever points at a missing file. Empty day folders go too."""
        rows = self.db.conn.execute("SELECT id, path FROM evidence WHERE captured_at < ?", (cutoff_iso,)).fetchall()
        gone: list[str] = []
        for r in rows:
            if r["path"]:
                try:
                    (self.root / r["path"]).unlink(missing_ok=True)
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


def _meta(r: Any) -> dict[str, Any]:
    return {"id": r["id"], "task_id": r["task_id"], "span_id": r["span_id"], "kind": r["kind"], "mime": r["mime"],
            "origin": r["origin"], "captured_at": r["captured_at"], "has_image": r["path"] is not None,
            "data": json.loads(r["data_json"]) if r["data_json"] else None}
