"""Sanitized Zomato trace shape: six unnamed textboxes. DOM attributes are explicit regression assumptions,
not verified live facts; the owner trace did not retain maxlength/inputmode. No browser is launched here.
"""
import json
import subprocess
from pathlib import Path
from tests.chromelab import _node


def test_six_unnamed_numeric_boxes_are_secret_and_filled_as_one_bound_group():
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
''' + group + fill + r'''
const classified=nodes.every(isSecret);
const mismatch=fill(1,1,location.origin,'12345',true);
const before=nodes.map(n=>n.value);
const result=fill(1,1,location.origin,'123456',true);
const changed=fill(1,1,'https://outside.example','654321',true);
console.log(JSON.stringify({classified,mismatch,before,result,changed,values:nodes.map(n=>n.value)}));
'''
    ran = subprocess.run([_node(), '-e', script], capture_output=True, text=True, check=True)
    result = json.loads(ran.stdout)
    assert result['classified'] and not result['mismatch']['ok'] and result['before'] == ['']*6
    assert result['result']['ok'] and not result['changed']['ok']
    assert result['values'] == list('123456')
