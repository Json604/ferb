"""Sanitized Zomato trace shape: six unnamed textboxes. DOM attributes are explicit regression assumptions,
not verified live facts; the owner trace did not retain maxlength/inputmode. No browser is launched here.
"""
import json
import subprocess
from pathlib import Path
import pytest
from tests.chromelab import _node, worker


def split_script(after: str) -> dict:
    source = Path('extensions/chromium/content.js').read_text()
    capture_text = source[source.index('  function maskedEditable('):source.index('  function label(')]
    group = source[source.index('  function codeGroup('):source.index('\n\n  function formKind')]
    describe = source[source.index('  function describe('):source.index('  // Visible text nearest')]
    act = source[source.index('  function act('):source.index('  function rect(')]
    capture = source[source.index('  // Frame links for capture masks'):source.index('  function snapshot(')]
    fill = source[source.index('  function fill('):source.index('\n\n  window.__ferb =')]
    script = r'''
const secret = /otp|one.?time|verification.?code/i;
let sealedSplitDocument = false;
const SECRET_NAME=secret, label=()=>'', visible=()=>true, sealedFields=new WeakSet();
class HTMLInputElement {};
class HTMLTextAreaElement {};
let nodes=Array.from({length:6},()=>Object.assign(new HTMLInputElement(), {tagName:'INPUT',type:'text',maxLength:1,
 inputMode:'numeric',pattern:'',name:'',id:'',autocomplete:'',isConnected:true,ownerDocument:1,value:''}));
let parent={querySelectorAll:()=>nodes, parentElement:null}; nodes.forEach(n=>n.parentElement=parent);
let location={origin:'https://www.zomato.com'}, element=()=>({el:nodes[0]}), setValue=(n,v)=>n.value=v;
const addEventListener=()=>{}, top={}, window={}, chrome={runtime:{}},
 getComputedStyle=()=>({getPropertyValue:()=>'none'});
let ids=0; const randomId=()=>'id'+(++ids);
''' + capture_text + group + describe + capture + act + fill + after
    ran = subprocess.run([_node(), '-e', script], capture_output=True, text=True, check=True)
    return json.loads(ran.stdout)


def test_six_unnamed_numeric_boxes_are_secret_and_filled_as_one_bound_group():
    result = split_script(r'''
const classified=nodes.every(isSecret);
const mismatch=fill(1,1,location.origin,'12345',true);
const before=nodes.map(n=>n.value);
const result=fill(1,1,location.origin,'123456',true);
const changed=fill(1,1,'https://outside.example','654321',true);
console.log(JSON.stringify({classified,mismatch,before,result,changed,values:nodes.map(n=>n.value)}));
''')
    assert result['classified'] and not result['mismatch']['ok'] and result['before'] == ['']*6
    assert result['result']['ok'] and not result['changed']['ok']
    assert result['values'] == list('123456')


@pytest.mark.parametrize('mutation', ['detach_after_first', 'clear_after_last'])
def test_split_fill_after_mutation_reports_unknown_when_widget_changes(mutation):
    result = split_script(r'''
let writes = 0;
setValue = (n, v) => {
  n.value = v; writes++;
  if (MUTATION === 'detach_after_first' && writes === 1) nodes[1].isConnected = false;
  if (MUTATION === 'clear_after_last' && writes === 6) nodes.forEach(n => n.value = '');
};
const result = fill(1, 1, location.origin, '123456', true);
console.log(JSON.stringify({result, writes}));
'''.replace('MUTATION', json.dumps(mutation)))
    assert result['writes'] > 0 and result['result']['unknown'] is True
    assert result['result']['ok'] is False


def test_partial_secret_fill_is_recorded_unknown_and_duplicate_is_not_replayed():
    result = worker('''(async () => {
      gen = 1; let calls = 0, events = [];
      send = async (kind, body) => events.push(body);
      install = async () => ({key:'paired'}); unseal = async () => 'synthetic-secret';
      inFrame = async () => {calls++; return {ok:false, unknown:true, detail:'code_group_changed_after_partial_fill'};};
      const command = {gen:1, id:'partial', op:'fill', args:{tab_id:1, frame_id:0, snap:1, el:0,
        sealed:{}, origin:'https://www.zomato.com', code_only:true}};
      await onCommand(command); await onCommand(command);
      return {calls, seen:(await session()).seen, events};
    })()''')
    assert result['calls'] == 1 and result['seen']['partial'] == 'unknown'
    assert result['events'][0]['outcome'] == 'unknown'
    assert 'synthetic-secret' not in str(result['events'])


def test_explicit_unnamed_boxes_need_no_numeric_or_length_attributes_and_stay_masked():
    result = split_script(r'''
 nodes.forEach(n => {n.maxLength=-1;n.inputMode='';});
 element = (g, id) => g === 1 && nodes[id] ? {el:nodes[id]} : {error:'stale'};
 const result=fill(1,0,location.origin,'A9B8C7',false,[0,1,2,3,4,5]);
 console.log(JSON.stringify({result,values:nodes.map(n=>n.value),masked:nodes.every(isSecret)}));
''')
    assert result['result']['ok'] and result['values'] == list('A9B8C7')
    assert result['masked']


@pytest.mark.parametrize('group', [[0,0,1,2,3,4], [0,1,2,3,4,6], [1,2,3,4,5], []])
def test_explicit_split_targets_are_validated_before_any_value_is_written(group):
    result = split_script(r'''
 element = (g, id) => nodes[id] ? {el:nodes[id]} : {error:'stale'};
 const result=fill(1,0,location.origin,'123456',false,GROUP);
 console.log(JSON.stringify({result,values:nodes.map(n=>n.value)}));
'''.replace('GROUP', json.dumps(group)))
    assert not result['result']['ok'] and result['values'] == [''] * 6


def test_explicit_split_replacement_nodes_remain_masked_in_snapshot_and_capture():
    result = split_script(r'''
 nodes.forEach(n => {n.maxLength=-1;n.inputMode='';});
 element = (g, id) => nodes[id] ? {el:nodes[id]} : {error:'stale'};
 const result=fill(1,0,location.origin,'A9B8C7',false,[0,1,2,3,4,5]);
 nodes = nodes.map(n => Object.assign(new HTMLInputElement(), n));
 const document = {querySelectorAll:()=>nodes}, documentId='same-document';
 const implicitRole=()=> 'textbox', hitTest=()=>false, scrollerOf=()=>null, round=n=>n, formKind=()=>null;
 const innerWidth=1000, innerHeight=1000;
 nodes.forEach(n=>{n.getBoundingClientRect=()=>({x:0,y:0,width:10,height:10,left:0,top:0,right:10,bottom:10});n.getAttribute=()=>null;});
 const shown=nodes.map(describe), capture=captureState();
 const search=Object.assign(new HTMLInputElement(),nodes[0],{value:'',type:'search',focus:()=>{}});
 nodes=[search];
 const typed=act(1,0,'type','Indian Food Court');
 nodes=[];
 console.log(JSON.stringify({result,shown,capture,typed,afterRemoval:captureState()}));
''')
    assert result['result']['ok']
    assert result['typed']['ok']
    assert all(not e['secret'] and e['value'] == 'filled' for e in result['shown'])
    assert [r['rect'] for r in result['capture']['rects']] == [[0, 0, 10, 10]] * 6   # every editable field (L13)
    assert result['afterRemoval']['rects'] == []                       # nothing left in the document to paint


@pytest.mark.parametrize('failure', ['before_secret', 'after_secret', 'unreadable'])
def test_screenshot_carries_the_page_before_and_after_and_publishes_no_bytes_for_an_unreadable_page(failure):
    """L13: a page with a secret field (before or after the capture) is captured with both looks in the mask (the
    daemon paints, or withholds a capture whose fields moved: tests/test_pixels_to_model.py); a page with a secret
    field the extension can't cover publishes no bytes (one it can't read at all is captured for the daemon's
    masking since protocol 15: tests/test_pixels_to_model.py)."""
    result = worker('''(async () => {
      attach = async id => tabState(id); let reads=0, captures=0;
      maskState = async () => {
        reads++;
        if (CASE==='unreadable') return {withheld: "a secret field is drawn again elsewhere on the page"};
        const secret = CASE==='before_secret' && reads===1 || CASE==='after_secret' && reads>1;
        return {top: 'doc', viewport: [0, 0, 100, 50],
                items: secret ? [{key: 'field|doc|f', rect: [1, 2, 3, 4], paint: true}] : []};
      };
      cdp=async()=>{captures++;return {data:'picture'};};
      const st=tabState(19);st.shot='previous-shot';st.visual={snap:7};
      const out=await commands.screenshot({tab_id:19});
      return {out,captures,shot:st.shot,visual:st.visual};
    })()'''.replace('CASE', json.dumps(failure)))
    if failure.endswith('secret'):
        looks = [len(result['out']['mask'][k]['items']) for k in ('before', 'after')]
        assert result['out']['data'] == 'picture' and looks == ([1, 0] if failure == 'before_secret' else [0, 1])
    else:
        assert 'data' not in result['out'] and result['out']['withheld']
    assert result['captures'] == (0 if failure == 'unreadable' else 1)
    assert result['shot'] is None and result['visual'] is None


def test_accessibility_names_cannot_replace_capture_masked_dom_labels():
    result = worker('''(async () => {
      const element={binding:'field',label:'',capture_masked:true};
      const frames=[{frame_id:0,document_binding:'doc',elements:[element]}];
      cdp=async(id,method,args)=>{
        if(method==='Page.getFrameTree') return {frameTree:{frame:{id:'main'}}};
        if(method==='Accessibility.getFullAXTree') return {nodes:[
          {role:{value:'RootWebArea'},backendDOMNodeId:1},
          {role:{value:'textbox'},name:{value:'secret-character'},backendDOMNodeId:2}]};
        if(method==='DOM.describeNode') return {node:{attributes:['data-ferb-ax',args.backendNodeId===1?'doc':'field']}};
        throw Error(method);
      };
      await accessibilitySnapshot(19,1,frames);
      return {element,errors:tabState(19).axErrors};
    })()''')
    assert result['element']['label'] == '' and not result['errors']


def test_editable_text_capture_and_labels_omit_replacement_secret_characters():
    source = Path('extensions/chromium/content.js').read_text()
    capture_text = source[source.index('  function maskedEditable('):source.index('  function codeGroup(')]
    page_text = source[source.index('  function pageText('):source.index('  // Frame links for capture masks')]
    script = r'''
const sealedSplitDocument=true, SKIP_TEXT=new Set(), labelText=s=>s.trim();
const NodeFilter={SHOW_TEXT:4,FILTER_REJECT:2,FILTER_ACCEPT:1};
const editor={tagName:'DIV',isContentEditable:true,innerText:'SECRET_EDITOR',getAttribute:()=>null};
const area={tagName:'TEXTAREA',innerText:'SECRET_AREA',getAttribute:()=>null};
const normal={tagName:'SPAN'};
const root={tagName:'DIV',getAttribute:()=>null,innerText:'Restaurant SECRET_EDITOR SECRET_AREA'};
const texts=[{data:'Restaurant',parentElement:normal},{data:'SECRET_EDITOR',parentElement:editor},{data:'SECRET_AREA',parentElement:area}];
const document={body:root,createTreeWalker:(el,show,{acceptNode})=>{
 let remaining=texts.filter(n=>acceptNode(n)===NodeFilter.FILTER_ACCEPT);return {nextNode:()=>remaining.shift()};
},createRange:()=>({selectNodeContents:()=>{},getBoundingClientRect:()=>({width:10,height:10,top:0})})};
''' + capture_text + page_text + r'''
console.log(JSON.stringify({direct:captureText(root),page:pageText(),label:label(root),editorLabel:label(editor),areaLabel:label(area)}));
'''
    ran = subprocess.run([_node(), '-e', script], capture_output=True, text=True, check=True)
    result = json.loads(ran.stdout)
    assert result == {'direct':'Restaurant','page':'Restaurant','label':'Restaurant','editorLabel':'','areaLabel':''}


@pytest.mark.parametrize('supported', [None, False, True])
def test_explicit_split_fill_requires_loaded_content_helper_feature(supported):
    result = worker('''(async () => {
      let calls=0,targets=null;
      globalThis.window={__ferb:{fill:(g,i,o,v,c,group)=>{
        calls++;targets=group;return {ok:true};
      }}};
      if (SUPPORTED !== null) window.__ferb.supportsSplitSecretFill=SUPPORTED;
      install=async()=>({key:'paired'});unseal=async()=> 'synthetic-secret';
      inFrame=async(tab,frame,fn,args)=>fn(...args);
      const result=await commands.fill({tab_id:1,frame_id:0,snap:1,el:0,sealed:{},
        origin:'https://www.zomato.com',split_els:[0,1,2,3,4,5]},'sealed-command');
      return {result,calls,targets};
    })()'''.replace('SUPPORTED', json.dumps(supported)))
    assert result['result']['ok'] is (supported is True)
    assert result['calls'] == int(supported is True)
    if supported:
        assert result['targets'] == list(range(6))
    else:
        assert result['result']['detail'] == 'split_fill_helper_update_required'
