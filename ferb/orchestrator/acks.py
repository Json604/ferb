"""Owns FERB's acknowledgement on receipt of a message (TQ3, P25). Must not claim what FERB will do, how long it
takes or that it will succeed: every phrase fits every message, so none is chosen by reading the message."""
import random

# docs/0.27_PLAN.md P25, verbatim and in order, without the starred "On it, one sec." (it implies speed).
PHRASES = (
    "On it.", "Got it.", "On it now.", "Okay, on it.", "Got it, on it.", "Working on it.", "Okay, working on it.",
    "Starting on it.", "Starting now.", "Okay, starting.", "Sure.", "Sure, on it.", "Sure, starting now.", "Alright.",
    "Alright, on it.", "Alright, starting.", "Noted.", "Noted, on it.", "Noted, starting now.", "Received.",
    "Received, on it.", "Copy that.", "Understood.", "Understood, on it.", "Okay.", "Okay, noted.", "Will do.",
    "Got it, starting.", "Let me see.",
)
RECENT = 8          # P25: never one of the last 8 used


def pick(recent: list[str]) -> str:
    """A phrase at random, never one of the last eight said (`recent` is oldest first)."""
    said = set(recent[-RECENT:])
    return random.choice([p for p in PHRASES if p not in said])
