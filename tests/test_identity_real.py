"""FERB's real seeded identity (pytest -m real): the copied Chrome profile is still signed in as FERB's address
(read-only: Gmail's Atom feed over the pipe, headless), and each configured secret exists in the login Keychain
(checked without reading its value). Uses ~/.ferb as installed; skips when it isn't set up."""

import asyncio
from pathlib import Path

import pytest

from ferb.act.browser.cdp_identity import CdpIdentityDriver
from ferb.config import Config
from ferb.identity.secrets import KeychainSecretStore, specs_from_config

pytestmark = pytest.mark.real
HOME = Path.home() / ".ferb"


def test_seeded_session_and_keychain_items() -> None:
    if not (HOME / "chrome" / "identity" / "Default").is_dir():
        pytest.skip("FERB's identity profile isn't seeded")
    cfg = Config(HOME)
    address = str(cfg.get("mailbox", "address", "") or "")
    assert address, "config.toml has no [mailbox] address"
    store = KeychainSecretStore(lambda: specs_from_config(cfg.load().get("secrets")))

    async def go() -> tuple[bool, list[bool]]:
        drv = CdpIdentityDriver(HOME / "chrome" / "identity", "dev", store, lambda v: None,
                                mailbox_address=lambda: address)
        try:
            ok = await drv.session_ok()
        finally:
            await drv.shutdown()
        return ok, [await store.present(s.ref) for s in store.specs()]
    signed_in, present = asyncio.run(go())
    assert signed_in, f"not signed in as {address}: re-seed with `ferb setup --seed-identity`"
    assert present and all(present)
