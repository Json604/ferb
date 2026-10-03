"""Today's searchable JSON activity log. Long-term memory is stored separately."""

import json
import os
import re
from datetime import datetime
from pathlib import Path
from typing import Any, Callable, Iterator


def terms(text: str) -> set[str]:
    return {t for t in re.findall(r"\w+", text.casefold()) if len(t) > 2}


class DailyActivity:
    def __init__(self, path: Path, now: Callable[[], datetime]) -> None:
        self.path, self.now = path, now

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
            self.path.write_text('', encoding='utf-8')
        return today

    def record(self, event: dict[str, Any]) -> None:
        day = self._rotate()
        self.path.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
        row = dict(event, day=day, when=self.now().astimezone().isoformat())
        separator = ''
        if self.path.exists() and self.path.stat().st_size:
            with self.path.open('rb') as source:
                source.seek(-1, os.SEEK_END)
                separator = '\n' if source.read(1) != b'\n' else ''
        fd = os.open(self.path, os.O_APPEND | os.O_CREAT | os.O_WRONLY, 0o600)
        with os.fdopen(fd, 'a', encoding='utf-8') as out:
            out.write(separator + json.dumps(row, ensure_ascii=False) + '\n')

    def search(self, query: str, limit: int = 8) -> list[dict[str, Any]]:
        day, wanted = self._rotate(), terms(query)
        if not wanted:
            return []
        best: list[tuple[int, int, dict[str, Any]]] = []
        for sequence, row in enumerate(self._rows()):
            score = len(wanted & terms(json.dumps(row, ensure_ascii=False)))
            if row.get('day') == day and score:
                best.append((score, sequence, row))
                best.sort(key=lambda hit: hit[:2], reverse=True)
                del best[max(1, limit):]
        return [row for _, _, row in best]
