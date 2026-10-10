"""Owner notices: persisted before display, acknowledged only after the CLI prints them."""
import json
from typing import Any


class AttentionNotices:
    def __init__(self, db: Any, clock: Any, redact: Any) -> None:
        self.db, self.clock, self.redact = db, clock, redact

    def add(self, ident: str, refs: list[str], text: str) -> dict[str, Any]:
        self.db.conn.execute('INSERT OR IGNORE INTO attention_notices VALUES(?,?,?,?,?,NULL)',
            (ident, json.dumps(refs), self.redact(text), 'external:observation,inferred', self.clock.now().isoformat()))
        return dict(self.db.conn.execute('SELECT * FROM attention_notices WHERE id=?', (ident,)).fetchone())

    def list(self, *, pending: bool = True) -> list[dict[str, Any]]:
        return [dict(r) for r in self.db.conn.execute('SELECT * FROM attention_notices' +
            (' WHERE delivered_at IS NULL' if pending else ' WHERE delivered_at IS NOT NULL') + ' ORDER BY created_at')]

    def acknowledge(self, ident: str) -> dict[str, Any]:
        row = self.db.conn.execute('SELECT * FROM attention_notices WHERE id=?', (ident,)).fetchone()
        if row is None:
            raise ValueError('No such owner notice')
        self.db.conn.execute('UPDATE attention_notices SET delivered_at=COALESCE(delivered_at,?) WHERE id=?',
                             (self.clock.now().isoformat(), ident))
        return dict(row)
