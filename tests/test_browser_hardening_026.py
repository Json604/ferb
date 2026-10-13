"""Executed extension regressions for background source identity and full evidence retrieval."""
from tests.chromelab import worker


def test_large_sealed_source_round_trips_without_a_javascript_argument_limit() -> None:
    import json
    import subprocess
    from tests.chromelab import EXT, _node
    script = '''const p=require(process.argv[1]); (async()=>{
      const key=Buffer.alloc(32,1).toString('base64'),text='Study 🌱 '.repeat(150000);
      const sealed=await p.seal(key,text,'read',p.FEED_URL);
      const restored=await p.unseal(key,sealed,'read',p.FEED_URL);
      console.log(JSON.stringify({exact:restored===text,chars:restored.length}));
    })().catch(e=>{console.error(e);process.exit(1);});'''
    result = subprocess.run([_node(), '-e', script, str(EXT/'protocol.js')], capture_output=True, text=True)
    assert result.returncode == 0, result.stderr
    assert json.loads(result.stdout) == {'exact': True, 'chars': 1350000}


def test_file_chooser_is_intercepted_in_every_exact_debugger_target_and_reported() -> None:
    result = worker('''(async()=>{
      const calls=[]; cdp=async(tab,method,params,session)=>{calls.push({method,params,session});return {};};
      ready=async()=>{}; send=async()=>{};
      await attach(1);
      chrome.debugger.onEvent.fire({tabId:1},'Target.attachedToTarget',
        {sessionId:'child',targetInfo:{type:'iframe'}});
      await Promise.all(tabState(1).sessions.values());
      chrome.debugger.onEvent.fire({tabId:1,sessionId:'child'},'Page.fileChooserOpened',
        {frameId:'frame-child',backendNodeId:71,mode:'selectMultiple'});
      return {calls,observed:await commands.dialog({tab_id:1})};
    })()''')
    intercepts = [c for c in result['calls'] if c['method'] == 'Page.setInterceptFileChooserDialog']
    assert len(intercepts) == 2, result
    assert {c.get('session') for c in intercepts} == {None, 'child'}
    assert all(c['params'] == {'enabled': True} for c in intercepts)
    chooser = result['observed']['file_choosers'][0]
    assert chooser['frame_id'] == 'frame-child' and chooser['session_id'] == 'child'
    assert chooser['native_picker_shown'] is False and chooser['transferred'] is False
    assert chooser['reason'] == 'native_file_chooser_intercepted'


def test_file_chooser_event_schema_preserves_observed_frame_and_no_transfer() -> None:
    from ferb.act.browser.ext_protocol import Event
    event = Event.model_validate({'event':'file_chooser','tab_id':1,'frame_id':'child-frame',
        'session_id':'child-session','backend_node_id':71,'mode':'selectMultiple',
        'reason':'native_file_chooser_intercepted','native_picker_shown':False,'transferred':False,
        'direct_upload_available':True,'detail':'No source selected'})
    facts = event.model_dump(exclude_none=True)
    assert facts['frame_id'] == 'child-frame' and facts['session_id'] == 'child-session'
    assert facts['native_picker_shown'] is False and facts['transferred'] is False


def test_page_events_do_not_invent_submit_or_select_a_first_ambiguous_option() -> None:
    import json
    import subprocess
    from tests.chromelab import EXT, _node
    script = r'''
const vm=require('vm'),fs=require('fs'); let submitted=0;
class Select {}
const el=new Select(); Object.assign(el,{isConnected:true,tagName:'SELECT',focus(){},dispatchEvent(){},
  options:[{value:'x',text:'same'},{value:'same',text:'other'}],value:'untouched',
  form:{requestSubmit(){submitted++;}}});
const doc={documentElement:{},body:{}};
const ctx=vm.createContext({document:doc,window:{},crypto:globalThis.crypto,WeakRef,
  HTMLSelectElement:Select,innerWidth:800,innerHeight:600,addEventListener(){},MutationObserver:class{observe(){}},
  KeyboardEvent:class{},Event:class{},el});
let source=fs.readFileSync(process.argv[1],'utf8').replace('let els = new Map();','let els = new Map([[1,new WeakRef(el)]]);').replace('let snapGen = 0;','let snapGen = 1;');
vm.runInContext(source,ctx);
const selected=ctx.window.__ferb.act(1,1,'select','same');
ctx.window.__ferb.act(1,1,'press','Enter');
console.log(JSON.stringify({selected,value:el.value,submitted}));
'''
    result = subprocess.run([_node(), '-e', script, str(EXT/'content.js')], capture_output=True, text=True)
    assert result.returncode == 0, result.stderr
    out = json.loads(result.stdout)
    assert out['submitted'] == 0
    assert out['selected']['ok'] is False and out['selected']['reason'] == 'ambiguous_option'
    assert out['value'] == 'untouched'
    assert len(out['selected']['candidates']) == 2


def test_unexpected_dialog_is_pending_until_agent_explicitly_answers() -> None:
    result = worker('''(async () => {
      const calls=[]; cdp=async(tab,method,params)=>{calls.push({method,params});return {};};
      tabState(1);
      chrome.debugger.onEvent.fire({tabId:1},"Page.javascriptDialogOpening",{type:"confirm",message:"Delete draft?"});
      await new Promise(r=>setTimeout(r,0));
      const pending=await commands.dialog({tab_id:1}); const before=calls.length;
      const answered=await commands.dialog({tab_id:1,decision:"dismiss"});
      return {pending,before,answered,calls};
    })()''')
    assert result['before'] == 0
    assert result['pending']['pending']['message'] == 'Delete draft?'
    assert result['answered']['handled'] == 'dismissed'
    assert result['calls'][0]['params'] == {'accept': False}


def test_upload_resolves_the_selected_child_frame_without_a_native_picker() -> None:
    result = worker('''(async () => {
      const calls = [], frames = [];
      const state = tabState(1); state.sessions.set("child", Promise.resolve());
      attach = async () => state;
      inFrame = async (tab, frame, func, args) => {
        frames.push(frame);
        if (String(func).includes("uploadFiles")) return [{name:"assignment.pdf",size:19,sha256:"content"}];
        return {ok:true};
      };
      cdp = async (tab, method, params, session) => {
        calls.push({method,params,session});
        if (method === "DOM.performSearch") return {searchId: session || "main", resultCount:session ? 1 : 0};
        if (method === "DOM.getSearchResults") return {nodeIds:[71]};
        return {};
      };
      const response = await commands.upload({tab_id:1,frame_id:985,snap:9,el:8,paths:["/tmp/local/assignment.pdf"]});
      return {response,frames,calls};
    })()''')
    assert result['response']['ok'], result
    assert set(result['frames']) == {985}
    sets = [c for c in result['calls'] if c['method'] == 'DOM.setFileInputFiles']
    assert len(sets) == 1
    assert sets[0]['session'] == 'child'
    assert sets[0]['params'] == {'nodeId': 71, 'files': ['/tmp/local/assignment.pdf']}
    assert result['response']['selected_files'][0]['sha256'] == 'content'


def test_ambiguous_stamped_upload_never_selects_a_first_node() -> None:
    result = worker('''(async () => {
      attach = async () => tabState(1); inFrame = async () => ({ok:true});
      const calls = []; cdp = async (tab, method) => {calls.push(method);
        return method === "DOM.performSearch" ? {searchId:"two",resultCount:2} : {};};
      const response = await commands.upload({tab_id:1,frame_id:0,snap:1,el:2,paths:["/tmp/a.pdf"]});
      return {response,calls};
    })()''')
    assert result['response']['ok'] is False
    assert result['response']['reason'] == 'upload_target_ambiguous'
    assert 'DOM.setFileInputFiles' not in result['calls']


def test_page_search_reads_child_frames_and_does_not_clip_long_hits() -> None:
    result = worker('''(async () => {
      chrome.scripting = {executeScript: async args => {
        if (!args.target.allFrames) throw Error("top frame only");
        return [{frameId:0,result:{lines:0,hits:[]}},
          {frameId:985,result:{lines:1,hits:["assignment " + "x".repeat(12000) + " final requirement"]}}];}};
      return await commands.find({tab_id:1,text:"assignment"});
    })()''')
    assert result['hits'][0]['frame_id'] == 985
    assert result['hits'][0]['text'].endswith(' final requirement')
    assert len(result['hits'][0]['text']) > 12000


def test_coincident_browser_windows_leave_the_secret_field_answer_unknown() -> None:
    result = worker('''(async () => {
      chrome.windows = {getAll:async()=>[{id:1,left:0,top:0,width:100,height:100},
        {id:2,left:0,top:0,width:100,height:100}]};
      let queried = false; chrome.tabs.query=async()=>{queried=true;return [{id:1}];};
      const response=await commands.window_secrets({bounds:[0,0,100,100]}); return {response,queried};
    })()''')
    assert result['response'] == {'matched': False, 'reason': 'ambiguous_window'}
    assert not result['queried']


def test_large_extension_evidence_is_chunked_and_reassembled_without_clipping() -> None:
    result = worker('''(async () => {
      gen=1; const sent=[]; send=async(type,body)=>sent.push(body);
      const original={text:"🌱 syllabus ".repeat(150000),elements:[{label:"end"}]};
      commands.snapshot=async()=>original;
      await onCommand({gen:1,id:"large",op:"snapshot",args:{tab_id:1}});
      const transfer=sent[0].value.result_parts;
      if (!transfer) return {chunked:false};
      let text="",offset=0,max=0;
      do {const part=await commands.result_part({id:transfer.id,offset});text+=part.text;
        max=Math.max(max,part.text.length);offset=part.next_offset;} while(offset!==null);
      const restored=JSON.parse(text);
      return {chunked:true,exact:restored.text===original.text,max,tail:restored.elements[0].label};
    })()''')
    assert result.get('chunked') and result.get('exact'), result
    assert result['max'] <= 100000
    assert result['tail'] == 'end'


def test_large_action_input_is_reassembled_before_exactly_one_delivery() -> None:
    result = worker('''(async()=>{
      gen=1; const sent=[],received=[];send=async(type,body)=>sent.push(body);
      commands.act=async args=>{received.push(args.text);return {ok:true};};
      const original="syllabus 🌱 ".repeat(150000);
      const message=JSON.stringify({id:"write",gen:1,op:"act",args:{text:original}});
      for(let offset=0;offset<message.length;offset+=100000) await commands.command_part({
        id:"write",offset,text:message.slice(offset,offset+100000),total_chars:message.length});
      return {deliveries:received.length,exact:received[0]===original,id:sent[0].id};
    })()''')
    assert result == {'deliveries':1,'exact':True,'id':'write'}


def test_cold_open_returns_window_choices_without_creating_or_focusing():
    result = worker('''(async()=>{
      await mutateSession(s=>{s.windowId=null;});
      const touched=[];
      chrome.windows={getAll:async()=>[{id:7,state:'fullscreen',focused:true,type:'normal'},
          {id:8,state:'minimized',focused:false,type:'normal'}],
        create:async()=>{touched.push('create');return {id:9};},
        get:async id=>({id,state:'minimized'}),update:async()=>touched.push('update')};
      chrome.tabs.create=async()=>{touched.push('tab');return {id:10};};
      const value=await commands.open({url:'https://example.test'},'cold');
      return {value,touched};
    })()''')
    assert result['touched'] == []
    assert result['value']['opened'] is False
    assert result['value']['reason'] == 'window_selection_required'
    assert [w['id'] for w in result['value']['windows']] == [7, 8]


def test_explicit_window_opens_only_inactive_tab_and_never_creates_window():
    """The tab opens blank and inactive in the chosen window; it can hold a download before the URL loads (D35)."""
    result = worker('''(async()=>{
      const calls=[], order=[]; chrome.windows={get:async id=>({id,state:'fullscreen',type:'normal'}),
        create:async()=>{throw Error('must not create a window');}};
      chrome.tabs.create=async args=>{calls.push(args);return {id:10};};
      chrome.tabs.update=async()=>{};chrome.tabs.get=async()=>({id:10,windowId:7,url:'https://example.test'});
      arm=async id=>{order.push(['arm',id]);}; load=async (id,url)=>{order.push(['load',id,url]);};
      return {value:await commands.open({url:'https://example.test',window_id:7},'explicit'),calls,order};
    })()''')
    assert result['calls'] == [{'windowId': 7, 'url': 'about:blank', 'active': False}]
    assert result['order'] == [['arm', 10], ['load', 10, 'https://example.test']]
    assert result['value']['window_id'] == 7


def test_disappeared_explicit_window_never_falls_back_to_another_window():
    result = worker('''(async()=>{
      chrome.windows={get:async()=>{throw Error('No window');},create:async()=>{throw Error('wrong fallback');}};
      try {await commands.open({url:'https://example.test',window_id:77},'gone');return 'unexpected success';}
      catch(e){return String(e);}
    })()''')
    assert 'No window' in result
