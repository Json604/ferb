"""Upload incident regressions: transport facts must retain the exact source, never filename equivalence.

These use disposable bytes and fake browser transport. They do not open a personal profile or submit a form.
"""

import asyncio
import hashlib
from pathlib import Path
from types import SimpleNamespace
from typing import Any

from ferb.act.browser.ext_bridge import _TabDriver
from ferb.act.browser.profiles import BrowserProfile, resolve


def test_local_upload_receipt_keeps_source_identity_when_transport_refuses(tmp_path: Path) -> None:
    requested = tmp_path / "local" / "assignment.pdf"
    requested.parent.mkdir()
    requested.write_bytes(b"Capital budgeting: Project A")
    alternate = tmp_path / "drive-copy" / "assignment.pdf"
    alternate.parent.mkdir()
    alternate.write_bytes(b"Financial analysis of an unrelated company")
    calls: list[tuple[str, dict[str, Any]]] = []

    class Transport:
        async def call(self, operation: str, args: dict[str, Any], **kwargs: Any) -> dict[str, Any]:
            calls.append((operation, args))
            return {"ok": False, "detail": "file input cannot be reached in this frame",
                    "reason": "upload_route_unavailable", "frame_id": 985}

    driver = _TabDriver(SimpleNamespace(), lambda: [])  # type: ignore[arg-type]
    tab = SimpleNamespace(bridge=Transport(), chrome_id=73)
    receipt = asyncio.run(driver._upload(tab, 985, 9, 8, (str(requested),), "upload-test"))

    assert not receipt.ok
    assert len(calls) == 1 and calls[0][0] == "upload"
    assert calls[0][1]["paths"] == [str(requested)]
    facts = getattr(receipt, "facts", {})
    assert facts.get("reason") == "upload_route_unavailable", "Do not collapse the structured platform fact into prose"
    assert facts.get("source") == "local", "A refusal still identifies the attempted source"
    assert len(facts.get("files", [])) == 1
    identity = facts["files"][0]
    assert identity["path"] == str(requested)
    assert identity["size"] == requested.stat().st_size
    assert identity["sha256"] == hashlib.sha256(requested.read_bytes()).hexdigest()
    assert facts.get("transferred") is False


def test_profile_display_name_does_not_hide_a_different_matching_account() -> None:
    profiles = [BrowserProfile("chrome", "Profile 1", "school", "personal@example.test"),
                BrowserProfile("chrome", "Profile 2", "Course", "school@example.test")]
    matches = resolve("school", profiles)
    assert matches.kind == "ambiguous"
    assert {p.folder for p in matches.candidates} == {"Profile 1", "Profile 2"}


def test_upload_receipt_compares_selected_bytes_to_the_requested_source(tmp_path: Path) -> None:
    requested=tmp_path/'assignment.pdf'
    requested.write_bytes(b'local assignment')
    class Transport:
        async def call(self,*args,**kwargs):
            return {'ok':True,'selected_files':[{'name':requested.name,'size':16,'sha256':'other-content'}]}
    driver=_TabDriver(SimpleNamespace(),lambda: [])
    tab=SimpleNamespace(bridge=Transport(),chrome_id=73)
    receipt=asyncio.run(driver._upload(tab,0,1,0,(str(requested),),'test'))
    assert receipt.facts['bytes_match'] is False
    assert receipt.facts['transferred'] is True
    assert not receipt.ok and receipt.facts['reason'] == 'uploaded_bytes_mismatch'


def test_search_pages_keep_one_candidate_inventory_even_if_the_live_page_changes() -> None:
    import json
    from tests.test_whose_browser import router
    driver,bridge=router()
    async def run():
        tab=await driver.attach('brave:Default:11','task')
        calls=0
        async def search(op,args,**kwargs):
            nonlocal calls
            calls+=1
            return {'count':1,'lines':1,'hits':[{'frame_id':7,'text':('original ' if calls==1 else 'changed ')*50}]}
        bridge.call=search
        text='';offset=0
        while offset is not None:
            part=await driver.find(tab,'candidate',offset,60);text+=part['text'];offset=part['next_offset']
        assert calls == 1
        assert json.loads(text)[0]['text'] == 'original '*50
    asyncio.run(run())


def test_ferb_browser_does_not_silently_ignore_a_requested_owner_profile() -> None:
    import pytest
    from ferb.act.browser.tools import BrowserOpen,FERB_BROWSER
    from ferb.core.errors import Refused
    tool=BrowserOpen(SimpleNamespace(),FERB_BROWSER)
    with pytest.raises(Refused,match='profile'):
        asyncio.run(tool._run(tool.Input(url='https://example.test',profile='Personal'),SimpleNamespace(task_id='t')))


def test_key_and_text_actions_require_explicit_input_instead_of_default_enter_or_clear():
    import pytest
    from pydantic import ValidationError
    from ferb.act.browser.tools import BrowserAct
    for action in ('type','select','press'):
        with pytest.raises(ValidationError,match='explicit text'):
            BrowserAct.Input(tab='t1',element='s1e1',action=action)
    assert BrowserAct.Input(tab='t1',element='s1e1',action='type',text='').text == ''
