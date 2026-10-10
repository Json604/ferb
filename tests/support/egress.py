"""Shared support for the egress-proxy tests: a name only FERB's proxy knows.

The sandbox has no DNS, so the proxy resolves hosts itself (`ferb.act.shell.proxy.addresses`), and it refuses this
Mac's own addresses. Tests need a local "origin" that a contained command reaches only through the proxy, so the
daemon fixture maps TEST_HOST to loopback, where those origins listen. Nothing in production resolves this name."""
import socket
from typing import Any

from ferb.act.shell.proxy import addresses

TEST_HOST = "localtest.ferb"


def resolve(host: str, port: int) -> list[tuple[int, tuple[Any, ...]]]:
    if host == TEST_HOST:
        return [(socket.AF_INET, ("127.0.0.1", port))]
    return addresses(host, port)


def install(proxy: Any) -> None:
    """The daemon's proxy (None when its shell is unavailable) resolves TEST_HOST to loopback."""
    if proxy is not None:
        proxy.resolve = resolve
