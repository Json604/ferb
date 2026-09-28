"""Owner-assisted OTP check (pytest -m real, needs FERB_OTP_CODE): the owner emails a code to FERB's real inbox;
FERB's real browser (default: background window) reads it from the Atom feed and fills it into a local page's code
field, never returning it. A test-only SecretSpec allows only that local page; nothing is added to config."""

import asyncio
import http.server
import os
import threading
from datetime import datetime, timedelta, timezone
from pathlib import Path
from typing import Any

import pytest

from ferb.act.browser.cdp_identity import CdpIdentityDriver
from ferb.config import Config
from ferb.core.types import SecretRef, SecretSpec
from ferb.identity.secrets import KeychainSecretStore

pytestmark = pytest.mark.real
HOME = Path.home() / ".ferb"
OTP_PAGE = b"<form><input id=code name=otp autocomplete=one-time-code><button>Verify</button></form>"


def test_owner_emailed_code_is_filled_and_never_returned() -> None:
    code = os.environ.get("FERB_OTP_CODE")
    if not code:
        pytest.skip("owner-assisted: set FERB_OTP_CODE and email it to FERB")
    since = datetime.fromisoformat(os.environ["FERB_OTP_SINCE"])

    class H(http.server.BaseHTTPRequestHandler):
        def do_GET(self) -> None:  # noqa: N802
            self.send_response(200)
            self.send_header("content-type", "text/html")
            self.end_headers()
            self.wfile.write(OTP_PAGE)

        def log_message(self, *a: Any) -> None:
            pass
    srv = http.server.ThreadingHTTPServer(("127.0.0.1", 0), H)
    threading.Thread(target=srv.serve_forever, daemon=True).start()
    origin = f"http://localhost:{srv.server_port}"
    ref = SecretRef("test/otp-check")
    store = KeychainSecretStore(lambda: {ref.key: SecretSpec(ref, [origin], "otp check")})
    address = str(Config(HOME).get("mailbox", "address"))
    scrubbed: list[str] = []
    drv = CdpIdentityDriver(HOME / "chrome" / "identity", "dev", store, scrubbed.append, allow_local=True,
                            mailbox_address=lambda: address)

    async def go() -> tuple[Any, str]:
        try:
            tab = await drv.open(f"{origin}/otp")
            s = await drv.snapshot(tab)
            el = drv.element_ref(tab, next(e.id for e in s.elements if e.secret))
            r = await drv.fill_otp(el, ref, since, None, timeout=600, poll=5)
            t, session, node, _ = drv._resolve(el)
            value = str(await drv._call(t, session, el.frame_id, node, "function() { return this.value; }"))
            return r, value
        finally:
            await drv.shutdown()
    try:
        r, value = asyncio.run(go())
    finally:
        srv.shutdown()
    assert r.ok, r.detail
    assert value == code and code not in r.detail and scrubbed == [code]
