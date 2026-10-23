"""L31: loops. A loop step repeats an ordinary checked worker round; between rounds code reads the round's probe and
no model runs. Pacing follows the owner's numbers (2026-10-24); a loop with no stated end ends only by the owner's stop.
The runner gets everything it touches as callbacks, so the supervisor decides how a round, a probe read, a notice and a
status line happen."""
import json
from dataclasses import asdict, dataclass, field
from typing import Any

# The owner's numbers (2026-10-24: "yes numbers work"). Changing one is an owner decision.
PROBE_SECONDS = 10
BACKUP_SECONDS = 1800
NO_LASTING_FIX_SECONDS = 120
CHURN_ROUNDS = 3
CHURN_WINDOW_SECONDS = 600
FIRST_GAP_SECONDS = 120
MAX_GAP_SECONDS = 3600
RESET_SECONDS = 1800
RECENT_ROUNDS = 5                    # how many earlier rounds' one-line results the next round sees


@dataclass
class Pacer:
    """Churn back-off on the wall clock (epoch seconds). A round "didn't last" when the probe trips again within
    NO_LASTING_FIX_SECONDS of it ending; CHURN_ROUNDS of those within CHURN_WINDOW_SECONDS start an episode: the
    minimum gap between rounds becomes FIRST_GAP_SECONDS and doubles with each further short round, up to
    MAX_GAP_SECONDS. A fix that lasts RESET_SECONDS ends the episode."""
    gap: float = 0.0
    short: list[float] = field(default_factory=list)
    episode: bool = False
    last_end: float | None = None

    def ended(self, at: float) -> None:
        self.last_end = at

    def lasted(self, at: float) -> bool:
        if self.last_end is not None and at - self.last_end >= RESET_SECONDS:
            self.gap, self.short, self.episode = 0.0, [], False
            return True
        return False

    def tripped(self, at: float) -> bool:
        """The first probe trip after a round, at `at`. True when a back-off episode starts now (its one notice)."""
        self.lasted(at)
        if self.last_end is None or at - self.last_end > NO_LASTING_FIX_SECONDS:
            return False
        self.short = [t for t in self.short if at - t <= CHURN_WINDOW_SECONDS] + [self.last_end]
        if self.episode:
            self.gap = min(self.gap * 2, float(MAX_GAP_SECONDS))
            return False
        if len(self.short) >= CHURN_ROUNDS:
            self.episode, self.gap = True, float(FIRST_GAP_SECONDS)
            return True
        return False

    def start_at(self, at: float) -> float:
        if not self.gap or self.last_end is None:
            return at
        return max(at, self.last_end + self.gap)

    def to_json(self) -> str:
        return json.dumps(asdict(self))

    @classmethod
    def from_json(cls, text: str | None) -> 'Pacer':
        data = json.loads(text or '{}')
        return cls(gap=float(data.get('gap', 0.0)), short=[float(t) for t in data.get('short', [])],
                   episode=bool(data.get('episode', False)), last_end=data.get('last_end'))


class LoopStore:
    """`loop_state` rows (migration 0047)."""
    FIELDS = {'state', 'rounds', 'probe_json', 'baseline_json', 'reads', 'pacing_json', 'recent_json', 'note',
              'event_cursor', 'steer_seen'}

    def __init__(self, db: Any, clock: Any) -> None:
        self.db, self.clock = db, clock

    def load(self, task_id: str, step_id: str, *, steers: int = 0, cursor: int = 0) -> dict[str, Any]:
        """The loop's row, created on first use with the correction count and plan-event cursor at its start (so
        earlier corrections and events don't wake it)."""
        self.db.conn.execute('INSERT OR IGNORE INTO loop_state(task_id,step_id,steer_seen,event_cursor,updated_at) '
                             'VALUES(?,?,?,?,?)', (task_id, step_id, steers, cursor, self.clock.now().isoformat()))
        return dict(self.db.conn.execute('SELECT * FROM loop_state WHERE task_id=? AND step_id=?',
                                         (task_id, step_id)).fetchone())

    def save(self, task_id: str, step_id: str, **values: Any) -> None:
        unknown = set(values) - self.FIELDS
        if unknown:
            raise ValueError(f'unknown loop_state fields {sorted(unknown)}')
        sets = ', '.join(f'{k}=?' for k in values)
        self.db.conn.execute(f'UPDATE loop_state SET {sets}, updated_at=? WHERE task_id=? AND step_id=?',
                             (*values.values(), self.clock.now().isoformat(), task_id, step_id))

    def digest(self, task_id: str, step_id: str) -> dict[str, Any]:
        row = self.db.conn.execute('SELECT rounds, recent_json FROM loop_state WHERE task_id=? AND step_id=?',
                                   (task_id, step_id)).fetchone()
        return {'rounds': row['rounds'], 'recent': json.loads(row['recent_json'])} if row else {'rounds': 0, 'recent': []}

    def running(self) -> list[dict[str, Any]]:
        return [dict(r) for r in self.db.conn.execute(
            "SELECT * FROM loop_state WHERE state NOT IN ('finished','stopped') ORDER BY updated_at")]
