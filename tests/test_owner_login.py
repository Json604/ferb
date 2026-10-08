import asyncio
import json
import pytest

from tests.chromelab import SHOP, _ready
from tests.browserkit import bd, call  # noqa: F401


@pytest.mark.parametrize("redirect", [False, True])
def test_owner_code_is_prompted_sealed_and_absent_from_model_and_logs(bd, monkeypatch, redirect) -> None:
    import tests.fake_extension as fake
    old_page = fake.page
    monkeypatch.setattr(fake, "page", lambda url: {"title": "Login", "text": "Enter code", "elements": [
        {"role": "textbox", "label": "Verification code", "secret": True, "input_type": "text"}]}
        if url.endswith("/code") else old_page(url))
    d, launched = bd
    ext, grant = _ready(d, launched)
    tab = call(d, grant, "browser.open", url=f"{SHOP}/code")["content"]["tab"]
    snap = call(d, grant, "browser.snapshot", tab=tab)["content"]
    result = asyncio.run_coroutine_threadsafe(d.daemon.gateway.call(grant, "browser.login_code",
        {"tab": tab, "element": snap["elements"][0]["id"], "effect": "edit", "why": "finish requested login"}), d.loop)
    ext.wait_for(lambda: any(a.kind == "owner_login_code" for a in d.daemon.state.approvals.pending()))
    ap = next(a for a in d.daemon.state.approvals.pending() if a.kind == "owner_login_code")
    assert SHOP in ap.reason
    if redirect:
        ext.tabs[d.daemon.router._tabs[tab].chrome_id] = "https://other.example/code"
    from ferb.cli.client import Client
    cli = Client(d.sock)
    try:
        cli.call("chat.answer", {"approval_id": ap.id, "text": "482913"})
    finally:
        cli.close()
    out = result.result(10)
    assert out["is_error"] is redirect, out
    assert ext.fills and ap.answer is None
    assert "482913" not in json.dumps(out)
    async def dump():
        return "\n".join(d.daemon.db.conn.iterdump())
    assert "482913" not in d.run(dump())


def test_cancelling_a_task_clears_its_pending_secret_prompt(bd, monkeypatch) -> None:
    import tests.fake_extension as fake
    monkeypatch.setattr(fake, "page", lambda url: {"title": "Login", "text": "Enter code", "elements": [
        {"role": "textbox", "label": "Verification code", "secret": True, "input_type": "text"}]})
    d, launched = bd
    ext, grant = _ready(d, launched)
    tab = call(d, grant, "browser.open", url=f"{SHOP}/code")["content"]["tab"]
    snap = call(d, grant, "browser.snapshot", tab=tab)["content"]

    async def run():
        job = asyncio.create_task(d.daemon.gateway.call(grant, "browser.login_code",
            {"tab": tab, "element": snap["elements"][0]["id"], "effect": "edit"}))
        for _ in range(100):
            if any(a.kind == "owner_login_code" for a in d.daemon.state.approvals.pending()):
                break
            await asyncio.sleep(.01)
        d.daemon.gateway.cancel_tasks({grant.task_id})
        result = await asyncio.gather(job, return_exceptions=True)
        assert isinstance(result[0], asyncio.CancelledError)
        assert not d.daemon.state.approvals.pending()
        assert not d.daemon.gateway.in_flight({grant.task_id})
        denied = await d.daemon.gateway.call(grant, "browser.open", {"url": SHOP})
        assert denied["is_error"] and "stopped" in denied["content"]["error"]
        assert all(r[0] == "unknown" for r in d.daemon.db.conn.execute(
            "SELECT state FROM action_ledger WHERE task_id = ?", (grant.task_id,)))
    d.run(run())
    assert ext.fills == []
