"""HAND-434 (extension 0.19.6), the extension's own code under node with fake Chrome APIs: the tabs FERB holds are kept
awake (focus emulation when armed, before each read, and when Chrome reports one frozen), every page script goes in at
once, a command that timed out names its step and does no more, and a capture after a read that hung skips the page's
looks. The real-browser half (a task's first tab, a frame that never loads) is tests/test_browser_hang_real.py."""

import json
import subprocess

from tests.chromelab import EXT, _node, worker

# A tab FERB drives (8): every debugger command and every injected script is recorded; `hang` makes the next scripts
# wait until `release()` (a frozen page: executeScript never answers).
SETUP = '''
      gen = 1; const sent = [], cmds = [], scripts = []; send = async (type, body) => { if (type === "result") sent.push(body); };
      let hang = false, waiting = [];
      const release = () => { hang = false; for (const w of waiting.splice(0)) w(); };
      chrome.debugger = new Proxy({
        attach: async () => {}, detach: async () => {},
        sendCommand: async (target, method, params) => {
          cmds.push(method);
          if (method === "Page.captureScreenshot") return {data: "AAAA"};
          if (method === "Page.getFrameTree") return {frameTree: null};
          return {};
        },
        onEvent: chrome.debugger.onEvent, onDetach: chrome.debugger.onDetach}, {get: (o, k) => o[k]});
      const page = {elements: [], iframes: [], text: "", document_binding: "d", frames: [], rects: [], viewport: [0, 0, 10, 10]};
      chrome.scripting = {executeScript: async (inj) => {
        scripts.push({what: inj.files ? "files" : String(inj.func).slice(0, 40), immediately: inj.injectImmediately === true});
        if (hang) await new Promise(r => waiting.push(r));
        const f = String(inj.func || "");
        return [{frameId: 0, result: f.includes("probeSeen") || f.includes("=> true") || f.includes("probeArm") ? true : page}];
      }};
      chrome.tabs.get = async id => ({id, windowId: 9, url: "https://s.example/", status: "complete", title: "S"});
      chrome.windows = {get: async () => ({left: 0, top: 0, width: 800, height: 600, state: "normal", type: "normal"})};
      let n = 0;
      const command = async (op, args, timeout_ms) => {
        const id = "c" + (++n), from = cmds.length, s0 = scripts.length;
        const done = onCommand({gen: 1, id, op, args: {tab_id: 8, ...args}, timeout_ms});
        return {done, id, cmds: () => cmds.slice(from), scripts: () => scripts.slice(s0),
                body: () => sent.find(b => b.id === id)};
      };
'''


def run(body: str) -> dict:
    return worker("(async () => {" + SETUP + body + "})()")


# decision: HAND-434
def test_a_tab_is_kept_awake_when_armed_before_each_read_and_when_chrome_freezes_it() -> None:
    """Lab 2026-11-08: Chrome froze the task's first tab 2.0 s after its load (its collapsed group was made with it in
    it), and executeScript never answered: 20 of 20 first reads ran into the 30 s timeout."""
    out = run('''
      await arm(8);
      const armed = cmds.slice();
      const c = await command("snapshot", {snap: 1}, 30000); await c.done;
      const read = c.cmds();
      const before = cmds.length;
      wakeFrozen(8, {frozen: true}); await new Promise(r => setTimeout(r, 10));
      return {armed, read, frozen: cmds.slice(before), body: c.body()};
    ''')
    armed = out["armed"]
    assert "Emulation.setFocusEmulationEnabled" in armed
    assert armed.index("Emulation.setFocusEmulationEnabled") < armed.index("Page.addScriptToEvaluateOnNewDocument")
    assert out["read"][0] == "Emulation.setFocusEmulationEnabled"       # before anything else of the read
    assert out["frozen"] == ["Emulation.setFocusEmulationEnabled"]
    assert out["body"]["status"] == "ok" and set(out["body"]["value"]["steps"]) >= {"wake_ms", "inject_ms", "script_ms"}


# decision: HAND-434
def test_every_page_script_goes_in_at_once() -> None:
    """A frame that never finished loading held executeScript(allFrames) until it did (lab: 96.7 s)."""
    out = run('''
      await arm(8);
      await (await command("snapshot", {snap: 1}, 30000)).done;
      await (await command("screenshot", {snap: 1}, 30000)).done;
      await (await command("find", {text: "x"}, 30000)).done;
      return {scripts};
    ''')
    assert out["scripts"] and all(s["immediately"] for s in out["scripts"])


# decision: HAND-434, HAND-439
def test_a_read_that_times_out_names_its_step_and_does_no_more() -> None:
    out = run('''
      await arm(8);
      await (await command("snapshot", {snap: 1}, 30000)).done;          // attached and ready
      hang = true;
      const c = await command("snapshot", {snap: 2}, 80); await c.done;
      const body = c.body(), stuck = c.scripts().length;
      release(); await new Promise(r => setTimeout(r, 30));                 // the page answers late
      return {body, stuck, after: c.scripts().length};
    ''')
    assert out["body"]["status"] == "error" and out["body"]["error"] == "timeout"
    assert out["body"]["step"] == "inject" and out["body"]["outcome"] == "failed"
    assert out["stuck"] == 1 and out["after"] == 1                       # given up: no snapshot script after it


# decision: HAND-439
def test_a_capture_after_a_read_that_hung_skips_the_pages_looks() -> None:
    out = run('''
      await arm(8);
      const c = await command("screenshot", {snap: 1, looks: false}, 30000); await c.done;
      return {body: c.body(), scripts: c.scripts(), cmds: c.cmds()};
    ''')
    value = out["body"]["value"]
    assert out["scripts"] == [] and "Page.captureScreenshot" in out["cmds"]
    assert value["unread"] and value["mask"] is None and value["data"] == "AAAA"   # the daemon masks it (B6)


# decision: HAND-434
def test_the_accessibility_pass_may_push_its_nodes_together_and_nothing_else_widens() -> None:
    script = (f"const p = require({json.dumps(str(EXT / 'protocol.js'))}); const ok = (ids) => "
              "p.cdpAllowed('DOM.pushNodesByBackendIdsToFrontend', {backendNodeIds: ids}); "
              "console.log(JSON.stringify([ok([1]), ok([1, 2, 3]), ok(Array.from({length: 1000}, (_, i) => i + 1)), "
              "ok(Array.from({length: 1001}, (_, i) => i + 1)), ok([]), ok([1.5]), ok(['1']), "
              "p.cdpAllowed('DOM.pushNodesByBackendIdsToFrontend', {backendNodeIds: [1], extra: 1})]))")
    out = subprocess.run([_node(), "-e", script], capture_output=True, text=True, timeout=20)
    assert json.loads(out.stdout) == [True, True, True, False, False, False, False, False]
