"""Errors shared across FERB."""


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
