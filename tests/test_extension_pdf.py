"""The extension's `pdf` (HAND-427, extension 0.19.3): a tab printed to PDF by Chrome, in the extension's own code
under node. Nothing paints over a PDF, so a page with a secret field anywhere, or one whose secret fields can't be
found, is withheld; Chrome is asked with exactly the allowed settings; a big PDF comes in parts; the protocol stays 21,
so every installed FERB keeps working with the new extension."""

import json
import subprocess

from tests.chromelab import EXT, _node, worker


# decision: HAND-427
def test_a_page_prints_to_pdf_only_when_no_secret_field_is_anywhere_in_it() -> None:
    out = worker('''(async () => {
      arm = async () => {}; const asked = [];
      cdp = async (tab, method, params) => { asked.push([method, params]); return {data: "JVBERi0xLjQ="}; };
      maskState = async () => ({items: [], whole: true});
      const clean = await commands.pdf({tab_id: 1, _timeout_ms: 1000});
      maskState = async () => ({items: [], whole: false});           // a card field below the fold
      const secret = await commands.pdf({tab_id: 1, _timeout_ms: 1000});
      maskState = async () => ({withheld: "the page can't be read, so its secret fields can't be found", unread: true});
      const unread = await commands.pdf({tab_id: 1, landscape: true, _timeout_ms: 1000});
      return {clean, secret, unread, asked};
    })()''')
    assert out["clean"] == {"data": "JVBERi0xLjQ="}
    assert "withheld" in out["secret"] and "data" not in out["secret"]
    assert "withheld" in out["unread"] and "data" not in out["unread"]
    assert out["asked"] == [["Page.printToPDF", {"printBackground": True, "preferCSSPageSize": True,
                                                 "landscape": False}]]          # only the clean page was printed


# decision: HAND-427
def test_a_big_pdf_comes_in_parts_and_then_is_gone() -> None:
    out = worker('''(async () => {
      arm = async () => {}; maskState = async () => ({items: [], whole: true});
      const big = "A".repeat(SHOT_PART + 10);
      cdp = async () => ({data: big});
      const head = await commands.pdf({tab_id: 2, _timeout_ms: 1000});
      const first = await commands.pdf_part({tab_id: 2, part: 0});
      const last = await commands.pdf_part({tab_id: 2, part: 1});
      let after = null;
      try { await commands.pdf_part({tab_id: 2, part: 0}); } catch (e) { after = e.message; }
      return {parts: head.parts, size: head.size, first: first.data.length, last: last.data.length, after};
    })()''')
    assert out["parts"] == 2 and out["first"] + out["last"] == out["size"]
    assert out["after"] == "no pdf to continue"


# decision: HAND-427
def test_chrome_is_asked_only_with_the_three_settings_and_the_protocol_stays_21() -> None:
    script = f"""
const p = require({json.dumps(str(EXT / "protocol.js"))});
console.log(JSON.stringify({{
  ok: p.cdpAllowed("Page.printToPDF", {{printBackground: true, preferCSSPageSize: true, landscape: true}}),
  stream: p.cdpAllowed("Page.printToPDF", {{printBackground: true, preferCSSPageSize: true, landscape: false,
                                            transferMode: "ReturnAsStream"}}),
  header: p.cdpAllowed("Page.printToPDF", {{printBackground: true, preferCSSPageSize: true, landscape: false,
                                            displayHeaderFooter: true, headerTemplate: "<img src=x>"}}),
  protocol: p.FERB_PROTOCOL, ops: p.OPS.includes("pdf") && p.OPS.includes("pdf_part"),
  reads: p.READ_OPS.has("pdf") && p.READ_OPS.has("pdf_part")}}));
"""
    out = json.loads(subprocess.run([_node(), "-e", script], capture_output=True, text=True, timeout=20).stdout)
    assert out == {"ok": True, "stream": False, "header": False, "protocol": 21, "ops": True, "reads": True}
    manifest = json.loads((EXT / "manifest.json").read_text())
    from ferb.act.browser.ext_protocol import PDF_EXTENSION_VERSION, version_ok
    assert PDF_EXTENSION_VERSION == "0.19.3" and version_ok(manifest["version"], PDF_EXTENSION_VERSION)   # pdf since 0.19.3
