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
    group = source[source.index('  function codeGroup('):source.index('\n\n  function formKind')]
    fill = source[source.index('  function fill('):source.index('\n\n  window.__ferb =')]
    script = r'''
const secret = /otp|one.?time|verification.?code/i;
const SECRET_NAME=secret, label=()=>'', visible=()=>true;
class HTMLInputElement {};
class HTMLTextAreaElement {};
let nodes=Array.from({length:6},()=>Object.assign(new HTMLInputElement(), {tagName:'INPUT',type:'text',maxLength:1,
 inputMode:'numeric',pattern:'',name:'',id:'',autocomplete:'',isConnected:true,ownerDocument:1,value:''}));
let parent={querySelectorAll:()=>nodes, parentElement:null}; nodes.forEach(n=>n.parentElement=parent);
let location={origin:'https://www.zomato.com'}, element=()=>({el:nodes[0]}), setValue=(n,v)=>n.value=v;
''' + group + fill + after
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
