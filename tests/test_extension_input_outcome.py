"""HAND-422/423, the extension's own code (background.js) under node with fake Chrome APIs: an acting command that
fails while its tab is being prepared, before any input reached the page, is a known failure; one that fails after its
first input event stays "unknown"; a frame session's setup error refuses input only while that frame is there, and one
that is still there makes the extension attach once more before it gives up."""

import pytest

from tests.chromelab import worker

GONE = '{"code":-32001,"message":"Session with given id not found."}'

# A tab FERB drives (8): every debugger command is recorded with its session; `failing` names what fails, by method on
# the tab's own session or by frame session id; `stuck` makes every new debugger session report a frame that fails.
SETUP = '''
      gen = 1; const sent = [], cmds = []; send = async (type, body) => { if (type === "result") sent.push(body); };
      let attaches = 0, detaches = 0, frames = 0, stuck = false;
      const failing = new Set();
      const frame = sid => chrome.debugger.onEvent.fire({tabId: 8}, "Target.attachedToTarget",
                                                         {sessionId: sid, targetInfo: {type: "iframe"}});
      chrome.debugger = new Proxy({
        attach: async () => { attaches++; },
        detach: async () => { detaches++; },
        sendCommand: async (target, method, params) => {
          cmds.push([target.sessionId || null, method, (params || {}).type]);
          if (method === "Input.dispatchMouseEvent" && params.type === "mouseMoved") lastMove = [params.x, params.y];
          if (failing.has(target.sessionId) || (!target.sessionId && failing.has(method))) throw new Error(GONE);
          if (method === "Target.setAutoAttach" && !target.sessionId && stuck) {
            const sid = "S" + (++frames); failing.add(sid); frame(sid);
          }
          return {};
        },
        onEvent: chrome.debugger.onEvent, onDetach: chrome.debugger.onDetach}, {get: (o, k) => o[k]});
      let lastMove = null, deafUntil = 0;   // the page sees each trusted move once the session is past deafUntil (HAND-486)
      chrome.scripting = {executeScript: async ({func}) =>
        [{frameId: 0, result: func && String(func).includes("rect(") ? {x: 50, y: 60}
                              : func && String(func).includes("pointerSeen()") ? (attaches > deafUntil ? lastMove || [] : [])
                              : true}]};
      chrome.tabs.get = async id => ({id, windowId: 9, url: "https://s.example/"});
      let n = 0;
      const click = async () => {
        const id = "c" + (++n), from = cmds.length;
        await onCommand({gen: 1, id, op: "act", args: {tab_id: 8, frame_id: 0, snap: 1, el: 3, kind: "click"}});
        const body = sent.find(b => b.id === id);
        return {outcome: body.outcome, error: body.error || null,
                input: cmds.slice(from).filter(([s, m]) => m.startsWith("Input.")).length,
                pressed: cmds.slice(from).filter(([s, m, type]) => type === "mousePressed").length};
      };
'''.replace("GONE", repr(GONE))


def run(body: str) -> dict:
    return worker("(async () => {" + SETUP + body + "})()")


# decision: HAND-422
def test_a_click_whose_tab_preparation_fails_sent_nothing_and_is_a_known_failure() -> None:
    """Live 2026-11-04 (a schedule run, two clicks on one site): "Session with given id not found" while the tab was
    being prepared came back "outcome unknown", so FERB refused to try the click again although nothing was sent."""
    out = run('''
      failing.add("Emulation.setFocusEmulationEnabled");     // the tab's preparation fails, before the probe
      const r = await click();
      return {...r, detaches, state: tabState(8).attached};
    ''')
    assert out["outcome"] == "failed" and "Session with given id not found" in out["error"]
    assert out["input"] == 0                                  # no input event of any kind reached the page
    assert out["detaches"] == 1 and out["state"] is False     # the half-prepared session was let go


# decision: HAND-422
def test_a_click_that_fails_after_its_first_input_event_stays_unknown() -> None:
    out = run('''
      await click();                                          // prepared once
      chrome.debugger.sendCommand = (orig => async (target, method, params) => {
        if (method === "Input.dispatchMouseEvent" && params.type === "mousePressed") throw new Error("renderer went away");
        return orig(target, method, params);
      })(chrome.debugger.sendCommand);
      return await click();
    ''')
    assert out["outcome"] == "unknown"                        # the move to the target was already sent


# decision: HAND-423
def test_a_frame_whose_setup_failed_and_is_still_there_is_attached_again_once() -> None:
    out = run('''
      const first = await click();
      failing.add("F1"); frame("F1");                         // a frame session whose setup fails, still listed
      await tabState(8).sessions.get("F1");
      const second = await click();
      return {first: first.outcome, second: second.outcome, pressed: second.pressed, attaches, detaches,
              left: tabState(8).inputErrors.size};
    ''')
    assert out["first"] == "done" and out["second"] == "done" and out["pressed"] == 1
    assert out["attaches"] == 2 and out["detaches"] == 1 and out["left"] == 0


# decision: HAND-423
def test_a_frame_that_stays_broken_after_attaching_again_is_a_known_failure_with_nothing_sent() -> None:
    out = run('''
      await click();
      stuck = true; failing.add("F1"); frame("F1");
      await tabState(8).sessions.get("F1");
      const r = await click();
      return {...r, attaches};
    ''')
    assert out["outcome"] == "failed" and "after attaching again" in out["error"]
    assert out["attaches"] == 2                               # once more, then it gave up
    assert out["pressed"] == 0                                # only the fresh session's readiness probe moved


# decision: HAND-423
@pytest.mark.parametrize("order", ["gone_before_its_error", "gone_after_its_error"])
def test_a_frame_that_went_leaves_no_error_behind(order: str) -> None:
    out = run(f'''
      await click();
      failing.add("F1"); frame("F1");
      const gone = () => chrome.debugger.onEvent.fire({{tabId: 8}}, "Target.detachedFromTarget", {{sessionId: "F1"}});
      if ("{order}" === "gone_before_its_error") gone();
      else {{ await tabState(8).sessions.get("F1"); gone(); }}
      const r = await click();
      return {{...r, attaches}};
    ''')
    assert out["outcome"] == "done" and out["attaches"] == 1  # no re-attach needed: the frame took its error with it


# decision: HAND-486
def test_a_click_whose_move_doesnt_reach_the_page_attaches_again_once_and_never_presses_blind() -> None:
    """Live: 18 of 85 screenshot clicks "never reached the target's frame", always in a tab that had navigated; a fresh
    tab worked. A click checks its move landed before pressing; when it didn't, FERB attaches afresh once."""
    out = run('''
      await click();                                           // the tab's first session, the page hears it
      deafUntil = attaches;                                    // then the page stops hearing this session's moves
      const healed = await click();
      deafUntil = 99;                                          // and now no session's moves arrive
      const deaf = await click();
      return {healed, deaf, attaches};
    ''')
    assert out["healed"]["outcome"] == "done" and out["healed"]["pressed"] == 1
    assert out["deaf"]["outcome"] == "failed" and out["deaf"]["pressed"] == 0
    assert out["attaches"] == 3                               # once for the healed click, once more for the deaf one


# decision: HAND-486
def test_a_new_document_in_a_driven_tab_runs_the_readiness_probe_again() -> None:
    out = run('''
      await click();
      const before = tabState(8).ready;
      tabLoading(8, {status: "loading"});
      const after = tabState(8).ready;
      const r = await click();
      return {before, after, r, ready: tabState(8).ready, attaches};
    ''')
    assert out["before"] is True and out["after"] is False
    assert out["r"]["outcome"] == "done" and out["ready"] is True and out["attaches"] == 1   # same session, probed again


# decision: HAND-498, HAND-422
@pytest.mark.parametrize("answer,outcome", [("Not allowed", "failed"), ("Session with given id not found.", "unknown")])
def test_a_file_the_browser_refuses_to_set_is_a_known_failure_and_other_errors_stay_unknown(answer: str,
                                                                                            outcome: str) -> None:
    """Live: no upload ever succeeded; DOM.setFileInputFiles answered "Not allowed" and the call came back "outcome
    unknown", although no file was set."""
    out = run('''
      inFrame = async (tab, frame, func) => String(func).includes("stamp(") ? {ok: true} : true;
      chrome.debugger.sendCommand = async (target, method) => {
        cmds.push([target.sessionId || null, method]);
        if (method === "DOM.performSearch") return {searchId: "q", resultCount: 1};
        if (method === "DOM.getSearchResults") return {nodeIds: [7]};
        if (method === "DOM.setFileInputFiles") throw new Error(ANSWER);
        return {};
      };
      await onCommand({gen: 1, id: "u1", op: "upload",
                       args: {tab_id: 8, frame_id: 0, snap: 1, el: 2, paths: ["/tmp/x/cv.pdf"]}});
      const body = sent.find(b => b.id === "u1");
      return {outcome: body.outcome, error: body.error};
    '''.replace("ANSWER", repr(answer)))
    assert out["outcome"] == outcome, out
    if outcome == "failed":
        assert "nothing was uploaded" in out["error"]
