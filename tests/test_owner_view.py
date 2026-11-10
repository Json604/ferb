"""HAND-497: the payment card's one-time, owner-only picture of the page. It is captured unmasked through a driver
method no tool reaches, while every model-visible capture of the same page stays masked (GATE-206)."""

import ast
import json
import pickle
from pathlib import Path
from typing import Any

import pytest

from ferb.act.browser.ext_bridge import CaptureUnavailable, OwnerOnlyImage
from ferb.act.evidence import mask
from tests.browserkit import OWNER, bd, call, pair, task  # noqa: F401 (fixture)
from tests.support.images import black, solid_jpeg, white

ROOT = Path(__file__).parents[1]


# decision: HAND-497, GATE-206
def test_the_owner_only_picture_is_unmasked_while_the_tools_screenshot_stays_masked(bd: Any) -> None:  # noqa: F811
    d, launched = bd
    ext = pair(d, launched, "Work")
    ext.wait_for(lambda: ext.gen is not None)
    ext.shot_jpeg = solid_jpeg(1280, 900)                 # white; the fake's password field sits at 0,0 10x10
    g = task(d, "pay", OWNER, {"research"})
    tab_id = call(d, g, "browser.open", url="https://shop.example/login")["content"]["tab"]
    router = d.daemon.router
    tab = router.owned_by(tab_id, g.task_id)
    masked = d.run(router.screenshot(tab))                 # what every model-visible capture goes through
    owner = d.run(router.owner_view(tab))
    assert black(mask.pixel(masked, 5, 5)) and white(mask.pixel(masked, 400, 400))
    assert isinstance(owner, OwnerOnlyImage) and white(mask.pixel(owner.jpeg(), 5, 5))   # unmasked, for the owner only
    assert "bytes>" in repr(owner) and str(len(owner.jpeg())) in str(owner)
    with pytest.raises(TypeError):
        json.dumps({"image": owner})
    with pytest.raises(TypeError):
        pickle.dumps(owner)
    ext.debugger_refused[max(ext.tabs)] = "Cannot access a chrome-extension:// URL of different extension"
    with pytest.raises(CaptureUnavailable) as gone:
        d.run(router.owner_view(tab))
    assert str(gone.value).startswith("couldn't capture the page")


# decision: HAND-497
def test_no_tool_and_nothing_but_the_driver_method_reaches_the_owner_only_capture() -> None:
    """Only `_TabDriver.owner_view` sends the op; no tool, gateway or store code names either (the payment card's
    builder is the one caller to come)."""
    senders, callers = [], []
    for path in (ROOT / "ferb").rglob("*.py"):
        src = path.read_text()
        if "owner_view" not in src:
            continue
        for node in ast.walk(ast.parse(src)):
            if isinstance(node, ast.Constant) and node.value == "owner_view":
                senders.append(path.relative_to(ROOT).as_posix())
            if isinstance(node, ast.Attribute) and node.attr == "owner_view":
                callers.append(path.relative_to(ROOT).as_posix())
    assert set(senders) <= {"ferb/act/browser/ext_bridge.py", "ferb/act/browser/ext_protocol.py"}, senders
    assert not [c for c in callers if c.startswith(("ferb/act/", "ferb/registry", "ferb/runtimes/", "ferb/store/",
                                                    "ferb/trace/"))], callers
    tools = (ROOT / "ferb/act/browser/tools.py").read_text() + (ROOT / "ferb/registry.py").read_text()
    assert "owner_view" not in tools
