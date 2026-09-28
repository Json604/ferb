"""FERB's real seeded identity (pytest -m real), in the default mode: a real Chrome window started in the
background. The copied profile is still signed in as FERB's address (read-only: Gmail's Atom feed over the pipe),
the browser never became the frontmost app, and each configured secret exists in the login Keychain (checked
without reading its value). Uses ~/.ferb as installed; skips when it isn't set up."""

import asyncio
import threading
import time
from pathlib import Path

import pytest

from ferb.act.browser.cdp_identity import CdpIdentityDriver
from ferb.config import Config
from ferb.identity.secrets import KeychainSecretStore, specs_from_config

pytestmark = pytest.mark.real
HOME = Path.home() / ".ferb"


def test_seeded_session_in_a_background_window_and_keychain_items() -> None:
    from AppKit import NSWorkspace
    if not (HOME / "chrome" / "identity" / "Default").is_dir():
        pytest.skip("FERB's identity profile isn't seeded")
    cfg = Config(HOME)
    address = str(cfg.get("mailbox", "address", "") or "")
    assert address, "config.toml has no [mailbox] address"
    store = KeychainSecretStore(lambda: specs_from_config(cfg.load().get("secrets")))
    drv = CdpIdentityDriver(HOME / "chrome" / "identity", "dev", store, lambda v: None,
                            mailbox_address=lambda: address)
    assert drv.headless is False                                  # the default is a real window
    stole, stop = [], threading.Event()

    def watch() -> None:
        ws = NSWorkspace.sharedWorkspace()
        while not stop.is_set():
            app, pipe = ws.frontmostApplication(), drv.pipe
            if app is not None and pipe is not None and app.processIdentifier() == pipe.proc.pid:
                stole.append(time.monotonic())
            time.sleep(0.03)

    async def go() -> tuple[bool, list[bool]]:
        try:
            ok = await drv.session_ok()
        finally:
            await drv.shutdown()
        return ok, [await store.present(s.ref) for s in store.specs()]
    t = threading.Thread(target=watch)
    t.start()
    try:
        signed_in, present = asyncio.run(go())
    finally:
        stop.set()
        t.join()
    assert signed_in, f"not signed in as {address}: re-seed with `ferb setup --seed-identity`"
    assert not stole, "FERB's browser became the frontmost app"
    assert present and all(present)
