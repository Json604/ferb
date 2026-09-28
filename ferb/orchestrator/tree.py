"""Task tree (section 3). Build step 4: the running tasks the gateway needs (request id, start, deadline, span).
Persistence and the DAG arrive with the multi-worker orchestrator (build step 8)."""

from dataclasses import dataclass
from datetime import datetime

from ferb.core.types import Task


@dataclass
class TaskRecord:
    task: Task
    started_at: datetime
    span_id: str                   # the task's root span: tool spans hang under it


class TaskTree:
    def __init__(self) -> None:
        self._tasks: dict[str, TaskRecord] = {}

    def add(self, rec: TaskRecord) -> None:
        self._tasks[rec.task.id] = rec

    def get(self, task_id: str) -> TaskRecord | None:
        return self._tasks.get(task_id)

    def remove(self, task_id: str) -> None:
        self._tasks.pop(task_id, None)
