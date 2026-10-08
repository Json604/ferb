"""Owns shared error types. Must not infer intent, choose a target or claim semantic success from transport facts.

Errors shared across FERB."""


from typing import Any


class FerbError(Exception):
    """Base class for FERB errors."""


class Unsupported(FerbError):
    """A guarantee this component can't provide; it fails explicitly instead of approximating."""


class AuthError(FerbError):
    """A connection or run token failed authentication or asked for a method its role doesn't allow."""


class NotRunning(FerbError):
    """The daemon isn't running, or went away mid-request."""


class AlreadyRunning(FerbError):
    """Another daemon is already answering on the socket."""


class Refused(FerbError):
    """A safety check said no. The message is safe to show a worker (never contains a secret)."""


class StaleRef(Refused):
    """A tab or element reference from an older snapshot, navigation or connection generation."""


class NoTarget(Refused):
    """What was asked for isn't there right now (e.g. an app with no window yet); nothing was attempted."""


class DriverError(FerbError):
    """A driver failed or needs an explicit route; optional facts contain observations, not command arguments."""

    def __init__(self, message: str, *, facts: dict[str, Any] | None = None) -> None:
        super().__init__(message)
        self.facts = facts or {}
