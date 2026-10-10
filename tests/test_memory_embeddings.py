import hashlib
from pathlib import Path

import pytest
from ferb.memory.embeddings import manifest, quantize, verify


def test_int8_finite_normalization():
    assert quantize([0.0, 1.0, -1.0]) == [0,127,-127]
    with pytest.raises(ValueError):
        quantize([float('nan')])


def test_manifest_requires_pinned_hashes_and_no_executable_or_traversal(tmp_path):
    import json
    p = tmp_path/'manifest.json'
    data = {'repository':'Qwen/model', 'revision':'a'*40, 'dimensions':256,
            'files':{'config.json':'b'*64,'model.safetensors':'c'*64}}
    p.write_text(json.dumps(data))
    assert manifest(p)['revision'] == 'a'*40
    data['files']['../escape.json'] = 'd'*64
    p.write_text(json.dumps(data))
    with pytest.raises(ValueError):
        manifest(p)


def test_model_directory_rejects_extra_files_and_changed_weights(tmp_path):
    p=tmp_path/'model.safetensors';p.write_bytes(b'weights')
    data={'files':{p.name:hashlib.sha256(b'weights').hexdigest()}}
    verify(tmp_path,data)
    (tmp_path/'hidden.py').write_text('')
    with pytest.raises(ValueError):
        verify(tmp_path,data)


def test_paired_memory_report_never_calls_missing_owner_verdict_a_pass():
    from ferb.eval.memory import paired_results
    rows=[{'case_id':'q','task_id':'actual-run','verified':True}]
    r=paired_results([{'id':'q'}],rows,rows)
    assert r['acceptance']=='pending'
    assert r['with']['owner_passes']==0
