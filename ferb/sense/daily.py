"""Assemble recorded activity for daily context. Must not turn observations into authorization or hide retrieval limits.

Today's searchable JSON activity log. Long-term memory is stored separately."""

import json
import os
import re
import tempfile
import threading
from array import array
from collections import Counter
from dataclasses import dataclass, field
from datetime import datetime
from pathlib import Path
from typing import Any, Callable, Iterator


def terms(text: str) -> set[str]:
    return {t for t in re.findall(r"\w+", text.casefold()) if len(t) > 2}


@dataclass
class _Index:
    """Terms of each row of today's log, read once as rows are appended (L28): a search looks terms up instead of
    decoding and re-encoding every row. Rebuilt when the log is replaced, rotated or rewritten."""
    inode: int
    generation: int
    offset: int = 0                                          # bytes of complete lines indexed so far
    rows: "array[int]" = field(default_factory=lambda: array("Q"))     # each row's byte offset
    days: list[str] = field(default_factory=list)
    postings: dict[str, "array[int]"] = field(default_factory=dict)


def without_queries(node: Any) -> Any:
    """A row as the planner reads it: every web address without its query string or fragment (live 2026-10-28: a
    login redirect 322,577 characters long, recorded twice in each of two rows, stopped every matching message). The
    log keeps the row as recorded."""
    if isinstance(node, dict):
        return {key: without_queries(value) for key, value in node.items()}
    if isinstance(node, list):
        return [without_queries(item) for item in node]
    if isinstance(node, str) and node.startswith(("http://", "https://")):
        return node.split("#", 1)[0].split("?", 1)[0]
    return node


class DailyActivity:
    def __init__(self, path: Path, now: Callable[[], datetime]) -> None:
        self.path, self.now = path, now
        self._index: _Index | None = None
        self._generation = 0                                 # bumped by every rotation or rewrite of the log
        self._lock = threading.Lock()                        # searches may run off the event loop (L28)

    def _rows(self) -> Iterator[dict[str, Any]]:
        if not self.path.exists():
            return
        with self.path.open(encoding='utf-8') as source:
            for line in source:
                try:
                    row = json.loads(line)
                except ValueError:
                    continue
                if isinstance(row, dict):
                    yield row

    def _rotate(self) -> str:
        today = self.now().astimezone().date().isoformat()
        first = next(self._rows(), None)
        if first is not None and first.get('day') != today:
            with self._lock:                                 # never under a search reading rows by offset
                self._generation += 1
                self.path.write_text('', encoding='utf-8')
        return today

    def record(self, event: dict[str, Any]) -> None:
        day = self._rotate()
        self.path.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
        row = dict(event, day=day, when=self.now().astimezone().isoformat())
        if event.get('id'):
            row['source_id'] = event['id']
        separator = ''
        if self.path.exists() and self.path.stat().st_size:
            with self.path.open('rb') as source:
                source.seek(-1, os.SEEK_END)
                separator = '\n' if source.read(1) != b'\n' else ''
        fd = os.open(self.path, os.O_APPEND | os.O_CREAT | os.O_WRONLY, 0o600)
        with os.fdopen(fd, 'a', encoding='utf-8') as out:
            out.write(separator + json.dumps(row, ensure_ascii=False) + '\n')

    def _rewrite(self, transform: Callable[[dict[str, Any]], dict[str, Any] | None]) -> None:
        """Atomically update derived observations; never leave a second raw copy after replacement. Waits for a search
        in progress, so no search reads a row offset into the replaced file."""
        with self._lock:
            self._rewrite_locked(transform)

    def _rewrite_locked(self, transform: Callable[[dict[str, Any]], dict[str, Any] | None]) -> None:
        if not self.path.exists():
            return
        fd, temporary = tempfile.mkstemp(prefix='.activity-', dir=self.path.parent)
        try:
            with os.fdopen(fd, 'w', encoding='utf-8') as out:
                for row in self._rows():
                    updated = transform(row)
                    if updated is not None:
                        out.write(json.dumps(updated, ensure_ascii=False) + '\n')
                out.flush()
                os.fsync(out.fileno())
            self._generation += 1
            os.replace(temporary, self.path)
        finally:
            Path(temporary).unlink(missing_ok=True)

    def forget_sources(self, ids: set[str]) -> None:
        if ids:
            self._rewrite(lambda row: None if row.get('source_id', row.get('id')) in ids else row)

    def redact_source(self, source_id: str, data: dict[str, Any]) -> None:
        def update(row: dict[str, Any]) -> dict[str, Any]:
            if row.get('source_id', row.get('id')) == source_id:
                return dict(row, source_id=source_id, data=data, where=data)
            return row
        self._rewrite(update)

    def search(self, query: str, limit: int = 8) -> list[dict[str, Any]]:
        """Today's rows sharing the most terms with the query, best first (ties: the later row). Read-only: the log
        rotates when the next row is recorded, so a search that finds yesterday's rows only filters them out."""
        day, wanted = self.now().astimezone().date().isoformat(), terms(query)
        if not wanted:
            return []
        with self._lock:
            index = self._refresh()
            if index is None:
                return []
            scores: Counter[int] = Counter()
            for term in wanted:
                scores.update(index.postings.get(term, ()))
            best = sorted(((score, row) for row, score in scores.items() if index.days[row] == day), reverse=True)
            offsets = [index.rows[row] for _, row in best[:max(1, limit)]]
            found: list[dict[str, Any]] = []
            try:
                with self.path.open('rb') as source:  # under the lock: a rotation or rewrite waits for these reads
                    for offset in offsets:
                        source.seek(offset)
                        try:
                            row = json.loads(source.readline())
                        except ValueError:
                            continue
                        if isinstance(row, dict):
                            found.append(row)
            except FileNotFoundError:                 # removed outside FERB since the lookup
                return []
        return found

    def _refresh(self) -> _Index | None:
        """Index the complete rows appended since the last search; start over after a rotation or rewrite."""
        try:
            stat = os.stat(self.path)
        except FileNotFoundError:
            self._index = None
            return None
        index = self._index
        if (index is None or index.inode != stat.st_ino or index.generation != self._generation
                or stat.st_size < index.offset):
            index = self._index = _Index(stat.st_ino, self._generation)
        if stat.st_size > index.offset:
            with self.path.open('rb') as source:
                source.seek(index.offset)
                data = source.read(stat.st_size - index.offset)
            end = data.rfind(b'\n') + 1                     # a partial last line waits for its newline
            position = index.offset
            for line in data[:end].splitlines(keepends=True):
                try:
                    row = json.loads(line)
                except ValueError:
                    row = None
                if isinstance(row, dict):
                    number = len(index.rows)
                    index.rows.append(position)
                    index.days.append(str(row.get('day')))
                    for term in terms(json.dumps(row, ensure_ascii=False)):
                        index.postings.setdefault(term, array('I')).append(number)
                position += len(line)
            index.offset += end
        return index
