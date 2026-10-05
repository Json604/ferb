"""Owns shared error types. Must not infer intent, choose a target or claim semantic success from transport facts.

Errors shared across FERB."""


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


class DriverError(FerbError):
    """A browser driver failed (connection lost, command error). Never carries command arguments."""
