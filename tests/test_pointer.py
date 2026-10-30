"""Exact visual pointer input, using the real extension JS in an isolated Node harness."""
from tests.chromelab import worker


def test_pointer_keeps_selected_pixel_and_releases_after_interrupted_drag():
    r = worker('''(async () => {
      attach = async () => tabState(1);
      readyPoint = async (t,p) => mouse(t,"mouseMoved",p.x,p.y,{buttons:0});
      checkVisual = async () => {};
      checkPoint = async (tab, p) => p;
      const st = tabState(1);
      st.visual = {points: new Map([["a", {x: 12,y: 23}], ["b", {x: 122,y: 233}]])};
      const events = [];
      mouse = async (tab, type, x, y, extra) => {
        events.push({type,x,y,...extra});
        if (type === "mouseMoved" && extra && extra.buttons === 1) throw new Error("cancelled");
      };
      let error;
      try { await commands.pointer({tab_id:1, point:"a", end:"b", kind:"drag"}, "gesture"); }
      catch (e) { error = e.message; }
      return {events, error, consumed: !st.visual};
    })()''')
    assert r["error"] == "cancelled"
    assert r["events"][0]["x"] == 12 and r["events"][0]["y"] == 23
    assert r["events"][-1]["type"] == "mouseReleased"
    assert r["consumed"]


def test_stale_visual_target_dispatches_no_input():
    r = worker('''(async () => {
      attach = async () => tabState(1);
      readyPoint = async (t,p) => mouse(t,"mouseMoved",p.x,p.y,{buttons:0});
      checkVisual = async () => { throw new Error("visual_stale"); };
      const st=tabState(1); st.visual={points:new Map([["a",{x:1,y:2}]])};
      let events=0; mouse=async()=>{events++;};
      const r=await commands.pointer({tab_id:1, point:"a", kind:"click"}, "gesture");
      return {error:r.detail,events};
    })()''')
    assert r == {"error": "visual_stale", "events": 0}


def test_pointer_cancellation_stops_path_and_releases_button():
    r = worker('''(async () => {
      attach=async()=>tabState(1); readyPoint=async(t,p)=>mouse(t,"mouseMoved",p.x,p.y,{buttons:0}); checkVisual=async()=>{}; checkPoint=async(t,p)=>p;
      const st=tabState(1);st.visual={points:new Map([["a",{x:1,y:2}],["b",{x:150,y:250}]])};
      const events=[];
      mouse=async(t,type,x,y,extra)=>{
        events.push(type);
        if(type==="mousePressed") await commands.pointer_cancel({tab_id:1,command_id:"gesture"});
      };
      try { await commands.pointer({tab_id:1,point:"a",end:"b",kind:"drag"},"gesture"); }
      catch(e) {return {events,error:e.message};}
    })()''')
    assert r["error"] == "pointer_cancelled"
    assert r["events"] == ["mouseMoved", "mousePressed", "mouseReleased"]


def test_daemon_cancellation_sends_release_request_to_extension():
    import asyncio
    import json
    from types import SimpleNamespace
    import pytest
    from ferb.act.browser.ext_bridge import Bridge, _Conn
    from ferb.act.browser.ext_protocol import Result

    async def go():
        sent = []
        bridge = Bridge(SimpleNamespace(browser="chrome", folder="Lab"), None, lambda *_: None, "test")
        class Writer:
            def write(self, data):
                command = json.loads(json.loads(data)["body"])     # protocol 18: a signed envelope
                sent.append(command)
                if command["op"] == "pointer_cancel":
                    bridge.pending[command["id"]].set_result(Result(id=command["id"], gen=0, status="ok"))
            async def drain(self):
                pass
        bridge.conn = _Conn(Writer(), key=b"k" * 32, nonce="n")
        call = asyncio.create_task(bridge.call("pointer", {"tab_id": 7}, cid="gesture"))
        await asyncio.sleep(0)
        call.cancel()
        with pytest.raises(asyncio.CancelledError):
            await call
        assert sent[-1]["op"] == "pointer_cancel"
        assert sent[-1]["args"] == {"tab_id": 7, "command_id": "gesture"}
        assert "gesture" in bridge.lost and not bridge.pending
    asyncio.run(go())
