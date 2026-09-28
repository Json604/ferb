"""Pairing decisions (ARCH 9.6, safety test 21.16): unpaired installs can't act; tokens single-use + expiring;
forged or replayed messages refused. Fake clock, no browser."""

import base64
import hashlib
import hmac
import json

from pairing import TOKEN_TTL, Pairing


class Clock:
    t = 1000.0

    def __call__(self):
        return self.t


def signed(install_id: str, key_b64: str, seq: int, mtype: str, body: dict) -> dict:
    body_json = json.dumps(body)
    text = f"{install_id}|{seq}|{mtype}|{body_json}".encode()
    mac = base64.b64encode(hmac.new(base64.b64decode(key_b64), text, hashlib.sha256).digest()).decode()
    return {"install_id": install_id, "type": mtype, "body_json": body_json, "seq": seq, "mac": mac}


def paired(p: Pairing, install_id="A", folder="Profile 5") -> str:
    token = p.issue_token("chrome", folder)
    v = p.handle({"install_id": install_id, "type": "pair", "body_json": json.dumps({"token": token})})
    assert v.accepted and v.binding == ("chrome", folder)
    return v.reply["key"]


def test_unpaired_install_may_only_say_hello_and_pair():
    p = Pairing(Clock())
    assert p.handle({"install_id": "X", "type": "hello", "body_json": "{}"}).accepted
    for mtype in ("tab_event", "result", "paired_ack"):
        v = p.handle({"install_id": "X", "type": mtype, "body_json": "{}", "seq": 1, "mac": "x"})
        assert (v.accepted, v.reason) == (False, "unpaired_install")
    assert not p.may_route_action("X")


def test_pairing_binds_install_to_the_profile_the_token_was_issued_for():
    p = Pairing(Clock())
    paired(p, "A", "Profile 5")
    paired(p, "B", "Profile 6")
    assert (p.installs["A"].folder, p.installs["B"].folder) == ("Profile 5", "Profile 6")
    assert p.may_route_action("A") and p.may_route_action("B")


def test_token_is_single_use():
    p = Pairing(Clock())
    token = p.issue_token("chrome", "Profile 5")
    msg = {"type": "pair", "body_json": json.dumps({"token": token})}
    assert p.handle({"install_id": "A", **msg}).accepted
    v = p.handle({"install_id": "EVIL", **msg})
    assert (v.accepted, v.reason) == (False, "unknown_token")
    assert "EVIL" not in p.installs


def test_token_expires():
    clock = Clock()
    p = Pairing(clock)
    token = p.issue_token("chrome", "Profile 5")
    clock.t += TOKEN_TTL + 1
    v = p.handle({"install_id": "A", "type": "pair", "body_json": json.dumps({"token": token})})
    assert (v.accepted, v.reason) == (False, "expired_token")
    assert "A" not in p.installs


def test_signed_messages_are_attributed_and_ordered():
    p = Pairing(Clock())
    key = paired(p, "A", "Profile 5")
    v = p.handle(signed("A", key, 1, "tab_event", {"url": "https://example.com"}))
    assert (v.accepted, v.binding) == (True, ("chrome", "Profile 5"))
    assert p.handle(signed("A", key, 2, "tab_event", {})).accepted


def test_replayed_seq_is_refused():
    p = Pairing(Clock())
    key = paired(p)
    m = signed("A", key, 1, "tab_event", {})
    assert p.handle(m).accepted
    assert p.handle(m).reason == "replay"
    assert p.handle(signed("A", key, 1, "tab_event", {"other": 1})).reason == "replay"


def test_tampered_body_or_wrong_key_is_refused():
    p = Pairing(Clock())
    key_a = paired(p, "A", "Profile 5")
    key_b = paired(p, "B", "Profile 6")
    m = signed("A", key_a, 1, "tab_event", {"url": "https://a.example"})
    m["body_json"] = json.dumps({"url": "https://evil.example"})
    assert p.handle(m).reason == "bad_mac"
    # B's key can't speak for A (impersonating another profile)
    assert p.handle(signed("A", key_b, 5, "tab_event", {})).reason == "bad_mac"


def test_unsigned_message_from_paired_install_is_refused():
    p = Pairing(Clock())
    paired(p)
    v = p.handle({"install_id": "A", "type": "tab_event", "body_json": "{}"})
    assert (v.accepted, v.reason) == (False, "bad_mac")
