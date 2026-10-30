"""Owns the attempt count behind the check-in (owner decision 7, 2026-10-30). Must not judge whether an attempt
succeeded: the attempt loop's checker and next-attempt choice decide that; this only counts the attempts they chose to
retry.

Counted per task or plan step: the worker runs of one attempt loop. Model calls, tool calls, time and loop rounds don't
count (each round of a loop starts its own loop at 0). After CHECK_IN_AFTER failed attempts FERB pauses and asks
"keep going?": Yes allows CHECK_IN_AFTER more; No stops with what it found. A run nobody attends stops there and
reports."""

from dataclasses import dataclass

CHECK_IN_AFTER = 5          # the owner's number (changed from 10 on 2026-10-30); changing it is the owner's decision
KEEP_GOING, STOP_HERE = "Yes", "No"


@dataclass
class Attempts:
    failed: int = 0
    allowed: int = CHECK_IN_AFTER

    def failed_one(self) -> bool:
        """One more attempt failed and another is about to start: whether the check-in is due before it."""
        self.failed += 1
        return self.failed >= self.allowed

    def allow_more(self) -> None:
        """The owner said keep going: CHECK_IN_AFTER more attempts before the next check-in."""
        self.allowed = self.failed + CHECK_IN_AFTER


def check_in_question(failed: int, goal: str) -> str:
    """The check-in, in FERB's words (the goal is the task's, shown in a few words by the caller)."""
    return f"I've tried {failed} times and haven't got this done yet: {goal}. Keep going?"


def stopped_after(failed: int, *, unattended: bool) -> str:
    """Why work stopped at a check-in, for the answer's "Why work stopped" line."""
    if unattended:
        return (f"I tried {failed} times without getting it done, and nobody was there to ask whether to keep going, "
                "so I stopped here.")
    return f"You asked me to stop after {failed} attempts."
