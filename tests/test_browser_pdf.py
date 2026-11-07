"""A page as a PDF (T-1065, HAND-427, HAND-428): the browser prints it through extension 0.19.3's `pdf`; an HTML file
FERB wrote is shown through a one-time local page where nothing runs or loads; the PDF is always a new file under
the protection lists; only the PDF needs the newer extension, never the connection."""

import asyncio
import base64
import urllib.error
import urllib.request
from pathlib import Path
from types import SimpleNamespace
from typing import Any

import pytest

from ferb.act.browser.ext_bridge import _TabDriver
from ferb.act.browser.tools import OWNER_BROWSER, PDF_PAGE_POLICY, BrowserPdf, _serve_once
from ferb.core.errors import Refused
from ferb.core.types import Identity, ToolContext

PDF = b"%PDF-1.4 resume"


def _ctx(tmp: Path, closed: Any = None) -> ToolContext:
    return ToolContext("task", "w", "r", Identity.OWNER, frozenset({"browser:read", "fs:write"}), "device", None,
                       str(tmp / "workspace"), (str(tmp),), (), path_closed=closed)


class FakeDriver:
    """The driver calls the tool makes: open a tab for an address, print a tab, close it."""
    def __init__(self) -> None:
        self.opened: list[str] = []
        self.closed: list[Any] = []
        self.fetched: bytes = b""

    async def open_for(self, url: str, owner: str, profile: str | None = None) -> Any:
        self.opened.append(url)
        self.fetched = await asyncio.to_thread(lambda: urllib.request.urlopen(url, timeout=5).read())
        return SimpleNamespace(tab_id="t9")

    async def pdf(self, tab: Any, landscape: bool = False) -> bytes:
        return PDF

    async def close(self, tab: Any) -> dict[str, Any]:
        self.closed.append(tab)
        return {}

    def owned_by(self, tab: str, owner: str) -> Any:
        return SimpleNamespace(tab_id=tab)

    def tab_url(self, tab: str) -> str:
        return "https://jobs.example/apply"


# decision: HAND-427
def test_an_html_file_prints_through_a_one_time_page_and_saves_a_new_pdf(tmp_path: Path) -> None:
    html = tmp_path / "resume.html"
    html.write_text("<h1>Kartikey</h1><p>Backend engineer.</p>")
    driver = FakeDriver()
    tool = BrowserPdf(driver, OWNER_BROWSER)                         # type: ignore[arg-type]
    out = tmp_path / "resume.pdf"
    r = asyncio.run(tool.run(tool.Input(path=str(out), html_path=str(html)), _ctx(tmp_path)))
    assert not r.is_error and out.read_bytes() == PDF and r.content["bytes"] == len(PDF)
    assert driver.fetched == html.read_bytes()                      # the browser got exactly the file
    assert driver.opened[0].startswith("http://127.0.0.1:") and len(driver.closed) == 1   # its tab closed after
    with pytest.raises(urllib.error.URLError):                     # the one-time address is gone
        urllib.request.urlopen(driver.opened[0], timeout=2)
    again = asyncio.run(tool.run(tool.Input(path=str(out), html_path=str(html)), _ctx(tmp_path)))
    assert again.is_error and "never overwrites" in again.content["error"] and out.read_bytes() == PDF


# decision: HAND-427
def test_the_one_time_page_runs_nothing_and_answers_only_its_own_address() -> None:
    async def go() -> tuple[str, str, int]:
        url, server = await _serve_once(b"<script>alert(1)</script>")
        try:
            def fetch(u: str) -> tuple[str, int]:
                try:
                    with urllib.request.urlopen(u, timeout=5) as resp:
                        return resp.headers["Content-Security-Policy"], resp.status
                except urllib.error.HTTPError as e:
                    return "", e.code
            policy, ok = await asyncio.to_thread(fetch, url)
            _, other = await asyncio.to_thread(fetch, url.rsplit("/", 1)[0] + "/guess.html")
            return policy, url, ok * 1000 + other
        finally:
            server.close()
    policy, url, codes = asyncio.run(go())
    assert policy == PDF_PAGE_POLICY and "script-src" not in policy and "default-src 'none'" in policy
    assert url.startswith("http://127.0.0.1:") and codes == 200 * 1000 + 404


# decision: HAND-427
def test_a_closed_folder_refuses_before_anything_is_written(tmp_path: Path) -> None:
    driver = FakeDriver()
    tool = BrowserPdf(driver, OWNER_BROWSER)                         # type: ignore[arg-type]
    out = tmp_path / "private" / "x.pdf"
    out.parent.mkdir()
    r = asyncio.run(tool.run(tool.Input(path=str(out), tab="t1"),
                             _ctx(tmp_path, closed=lambda p, access: "a protected folder" if "private" in str(p) else None)))
    assert r.is_error and not out.exists()


def test_the_input_takes_exactly_one_source_and_a_pdf_path() -> None:
    with pytest.raises(ValueError):
        BrowserPdf.Input(path="a.pdf")
    with pytest.raises(ValueError):
        BrowserPdf.Input(path="a.pdf", tab="t1", html_path="a.html")
    with pytest.raises(ValueError):
        BrowserPdf.Input(path="a.txt", tab="t1")


class Bridge:
    def __init__(self, version: str, replies: list[dict[str, Any]]) -> None:
        self.version, self.replies, self.calls = version, replies, []

    async def call(self, op: str, args: dict[str, Any], timeout: float = 30) -> dict[str, Any]:
        self.calls.append((op, args))
        return self.replies.pop(0)


def _driver(bridge: Bridge) -> Any:
    d = _TabDriver.__new__(_TabDriver)
    d._tab = lambda ref: SimpleNamespace(bridge=bridge, chrome_id=7)         # type: ignore[attr-defined]
    return d


# decision: HAND-428
def test_only_a_pdf_needs_the_newer_extension_and_parts_and_withholding_are_honoured() -> None:
    old = Bridge("0.19.2", [])
    with pytest.raises(Refused, match="0.19.3"):
        asyncio.run(_driver(old).pdf("t1"))
    assert old.calls == []                                          # nothing was asked of the older extension
    data = base64.b64encode(PDF).decode()
    split = Bridge("0.19.3", [{"parts": 2, "size": len(data)}, {"data": data[:6]}, {"data": data[6:]}])
    assert asyncio.run(_driver(split).pdf("t1", True)) == PDF
    assert split.calls[0] == ("pdf", {"tab_id": 7, "landscape": True}) and [c[0] for c in split.calls[1:]] == ["pdf_part"] * 2
    withheld = Bridge("0.19.3", [{"withheld": "the page has a password, code or card field"}])
    with pytest.raises(Refused, match="no PDF of this page"):
        asyncio.run(_driver(withheld).pdf("t1"))
    garbage = Bridge("0.19.3", [{"data": base64.b64encode(b"<html>").decode()}])
    with pytest.raises(Refused, match="didn't return a PDF"):
        asyncio.run(_driver(garbage).pdf("t1"))
