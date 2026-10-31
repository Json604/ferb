"""This Mac's sockets as kernel facts (sockets.py), the source of local.py's and listeners.py's checks since netstat
went blank for FERB on macOS 27.2 (regression 2026-10-31). Fixtures recorded on macOS 27.2 (arm64): socket_fdinfo
answers for loopback sockets this test's recorder opened (the kernel's opaque handles and stat block zeroed), and a
nettop listing excerpt (the owner's public addresses replaced by documentation ones)."""
import json
import socket
import time
from pathlib import Path

import pytest

from ferb.act.shell import sockets
from ferb.act.shell.sockets import Socket

SUPPORT = Path(__file__).parent / "support"
RECORDED = json.loads((SUPPORT / "socket_fdinfo.json").read_text())
PORTS = RECORDED["ports"]


def record(name: str) -> bytes:
    return bytes.fromhex(RECORDED["records"][name])


def test_the_sdk_layout_matches_the_kernels_answer_size() -> None:
    assert sockets.SOCKET_FDINFO_SIZE == 792 == len(record("tcp4 client end"))


def test_recorded_socket_answers_decode_to_endpoints_state_queue_and_process() -> None:
    p = PORTS["listener"]
    decoded = {name: sockets.decode(record(name), 4242) for name in RECORDED["records"]}
    assert decoded == {
        "tcp4 listener with one connection queued": Socket("tcp", ("127.0.0.1", p), None, "LISTEN", 4242, 1),
        "tcp4 client end": Socket("tcp", ("127.0.0.1", PORTS["accepted_client"]), ("127.0.0.1", p), "ESTABLISHED",
                                  4242),
        "tcp4 server end, accepted": Socket("tcp", ("127.0.0.1", p), ("127.0.0.1", PORTS["accepted_client"]),
                                            "ESTABLISHED", 4242),
        "tcp6 listener on ::1": Socket("tcp", ("::1", PORTS["v6"]), None, "LISTEN", 4242),
        "tcp46 listener on any address": Socket("tcp", ("*", PORTS["any"]), None, "LISTEN", 4242),
        "udp4 bound, unconnected": Socket("udp", ("127.0.0.1", PORTS["udp"]), None, "", 4242),
        "udp4 connected": Socket("udp", ("127.0.0.1", PORTS["udp_client"]), ("127.0.0.1", PORTS["udp"]), "", 4242),
        # A v6 socket connected to ::ffff:127.0.0.1 is the kernel's IPv4 connection
        "tcp6 socket connected to a v4-mapped address": Socket("tcp", ("127.0.0.1", PORTS["mapped_client"]),
                                                               ("127.0.0.1", p), "ESTABLISHED", 4242),
        "unix socket": None}


def test_an_answer_of_another_size_is_refused() -> None:
    with pytest.raises(ValueError):
        sockets.decode(record("tcp4 client end")[:-8], 1)


def test_the_recorded_nettop_listing_parses_with_each_sockets_process() -> None:
    listed = sockets.parse_nettop((SUPPORT / "nettop_listing.txt").read_text())
    assert listed is not None
    assert [(pid, s.proto, s.local, s.remote, s.state) for pid, s in listed] == [
        (1, "tcp", ("127.0.0.1", 8021), None, "LISTEN"),
        (1, "tcp", ("::1", 8021), None, "LISTEN"),
        (1, "tcp", ("*", 22), None, "LISTEN"),
        (1, "tcp", ("*", 22), None, "LISTEN"),
        (566, "udp", ("*", 57830), None, ""),
        (573, "tcp", ("192.0.2.10", 61859), ("203.0.113.5", 5223), "ESTABLISHED"),
        (610, "udp", ("*", 5353), None, ""),                       # the quic4 flow and an unbound socket: left out
        (610, "udp", ("*", 5353), None, ""),
        (971, "tcp", ("*", 49152), None, "LISTEN"),
        (971, "tcp", ("*", 49152), None, "LISTEN"),
        (4250, "tcp", ("127.0.0.1", 56782), None, "LISTEN"),
        (4250, "tcp", ("127.0.0.1", 56782), ("127.0.0.1", 49726), "ESTABLISHED"),
        (4013, "tcp", ("192.0.2.10", 49597), ("203.0.113.9", 443), "CLOSE_WAIT")]


@pytest.mark.parametrize("text", [
    "",                                                                 # nothing printed (netstat's failure)
    "state\nlaunchd.1,,\n",                                             # another header
    ",state,\ntcp4 *:22<->*:*,Listen,\n",                               # a socket before any process
    ",state,\nevil\ntcp4 *:22<->*:*,Listen,.4242,,\n",                  # a program name holding a line break
    ",state,\nlaunchd.1,,\ntcp4 *:22<->*:*,Listen,extra,\n"])           # another column
def test_a_listing_in_any_other_shape_is_no_listing(text: str) -> None:
    assert sockets.parse_nettop(text) is None


def test_live_a_listener_queues_until_accepted_and_the_accepted_end_belongs_to_its_process() -> None:
    import os
    server = socket.socket(); server.bind(("127.0.0.1", 0)); server.listen(4)
    port = server.getsockname()[1]
    client = socket.create_connection(("127.0.0.1", port))
    source = client.getsockname()

    def mine() -> list[Socket]:
        got = sockets.inspectable()
        assert got is not None and os.getpid() in got[1]
        return [s for s in got[0] if s.pid == os.getpid() and port in (s.local[1], (s.remote or ("", 0))[1])]
    try:
        deadline = time.monotonic() + 5
        while not any(s.state == "LISTEN" and s.queued == 1 for s in mine()):
            assert time.monotonic() < deadline, "the queued connection never showed on its listener"
            time.sleep(0.01)
        assert not any(s.local == ("127.0.0.1", port) and s.remote == source for s in mine())   # held by no one
        accepted, _ = server.accept()
        assert Socket("tcp", ("127.0.0.1", port), source, "ESTABLISHED", os.getpid()) in mine()
        accepted.close()
    finally:
        client.close(); server.close()


def test_live_the_complete_listing_names_every_users_listeners_and_ours_by_process() -> None:
    import os
    server = socket.socket(); server.bind(("127.0.0.1", 0)); server.listen()
    try:
        every = sockets.complete()
        assert every is not None
        assert Socket("tcp", ("127.0.0.1", server.getsockname()[1]), None, "LISTEN", os.getpid()) in every
        assert all(s.pid != 1 for s in every)          # launchd's (root's) sockets are listed unnamed, never as pid 1
    finally:
        server.close()


def test_a_nettop_that_misses_a_known_listener_is_no_reading(monkeypatch: pytest.MonkeyPatch) -> None:
    """The netstat failure mode again: a listing that holds only some sockets, not an empty Mac."""
    server = socket.socket(); server.bind(("127.0.0.1", 0)); server.listen()
    try:
        monkeypatch.setattr(sockets, "_nettop", lambda: [])
        assert sockets.complete() is None
        monkeypatch.setattr(sockets, "_nettop", lambda: None)
        assert sockets.complete() is None
        hidden = Socket("tcp", ("*", 8021), None, "LISTEN", 1)
        known = sockets.inspectable()
        assert known is not None
        listing = [(s.pid or 0, s) for s in known[0]] + [(1, hidden)]
        monkeypatch.setattr(sockets, "_nettop", lambda: listing)
        every = sockets.complete()
        assert every is not None
        assert Socket("tcp", ("*", 8021), None, "LISTEN", None) in every          # root's listener: unnamed
    finally:
        server.close()
