"""Whether this Mac has a network route at all: a fact for telling the owner why a model service couldn't be reached.
Must not send anything, wait, or judge a service.

A UDP socket's connect() only asks the kernel's routing table for a route to the address: no packet leaves the Mac.
With no route for IPv4 or IPv6 (Wi-Fi off, cable out, no interface up), the Mac is offline; anything else (a route,
or an error that says nothing about routes) isn't called offline."""
import errno
import socket
from typing import Any

# Public resolver addresses, used only as route-lookup targets (nothing is sent to them).
_TARGETS = ((socket.AF_INET, ("1.1.1.1", 53)), (socket.AF_INET6, ("2606:4700:4700::1111", 53)))
_NO_ROUTE = {errno.ENETUNREACH, errno.EHOSTUNREACH, errno.EADDRNOTAVAIL, errno.ENETDOWN}


def _route(make: Any = socket.socket) -> bool | None:
    """True: some route exists. False: no route for either family. None: the lookup said nothing about routes."""
    unknown = False
    for family, address in _TARGETS:
        try:
            s = make(family, socket.SOCK_DGRAM)
        except OSError:
            unknown = True
            continue
        try:
            s.connect(address)
            return True
        except OSError as e:
            unknown = unknown or e.errno not in _NO_ROUTE
        finally:
            s.close()
    return None if unknown else False


def has_route() -> bool | None:
    return _route()
