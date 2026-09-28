"""Per-site account extractors (section 9.3): before a consequential action in an owner profile, FERB checks which
account is signed in, not just "signed in". The extractors themselves run in the page (extensions/chromium/
content.js); this is the daemon's side of the decision.

- Site with an extractor and a profile whose account FERB knows (Local State): check at run time; a different
  account refuses the action (safety test 12); signed out refuses too (FERB never signs in as the owner).
- Otherwise the account can't be checked: policy asks the owner once per site + profile and remembers the answer
  (learned trust, 10.4).
"""

from ferb.core.errors import Refused

EXTRACTOR_SITES = frozenset({"mail.google.com", "www.youtube.com"})     # keep in step with content.js ACCOUNT


def trust_key(host: str, browser: str, folder: str) -> str:
    return f"{host} @ {browser}/{folder}"


def unverified_key(host: str | None, browser: str, folder: str, profile_email: str) -> str | None:
    """None when the account can be checked at run time; otherwise the key policy asks about once."""
    if not host:
        return None
    if host in EXTRACTOR_SITES and profile_email:
        return None
    return trust_key(host, browser, folder)


def check(result: dict[str, object], host: str, expected: str, profile: str) -> None:
    """The extractor's answer vs the account FERB resolved for this profile. Raises Refused on any doubt."""
    if not result.get("supported"):
        return                                                          # policy already asked for this site
    account = result.get("account")
    if account is None:
        raise Refused(f"not signed in on {host} in {profile}; FERB never signs in as you")
    if account == "signed_in_unknown":
        raise Refused(f"couldn't read which account is signed in on {host} in {profile}")
    if str(account).lower() != expected.lower():
        raise Refused(f"wrong account on {host} in {profile}: signed in as {account}, expected {expected}")
