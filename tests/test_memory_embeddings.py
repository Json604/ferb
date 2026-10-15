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


def test_local_model_identity_covers_preprocessing(monkeypatch,tmp_path):
    import sys,types
    from ferb.memory import embeddings
    monkeypatch.setattr(embeddings,'verify',lambda root,data:None)
    stub=types.ModuleType('mlx_embeddings.utils');stub.load=lambda *a,**k:(None,None)
    monkeypatch.setitem(sys.modules,'mlx_embeddings.utils',stub)
    data={'repository':'Qwen/model','revision':'a'*40,'dimensions':256,'files':{},'query_prefix':'query: '}
    first=embeddings.LocalMLX(tmp_path,data)
    second=embeddings.LocalMLX(tmp_path,data|{'query_prefix':'different: '})
    assert first.name != second.name


def test_without_the_memory_extra_nothing_is_downloaded(daemon, monkeypatch, tmp_path):
    """Review M8: the packaged app has no `memory` extra; `ferb memory embeddings evaluate|adopt` downloaded the model,
    then failed importing mlx_embeddings. The missing extra is found first, and nothing is fetched."""
    from ferb.cli.client import Client
    from ferb.memory import embeddings
    monkeypatch.setattr(embeddings, "extra_missing", lambda: "mlx_embeddings")
    fetched = []
    monkeypatch.setattr(embeddings, "download", lambda *a, **k: fetched.append(a))
    manifest = tmp_path / "manifest.json"
    manifest.write_text("{}")
    c = Client(daemon.sock)
    try:
        status = c.call("memory.learning", {"op": "embedding", "action": "status"})
        with pytest.raises(Exception, match="optional memory extra"):
            c.call("memory.learning", {"op": "embedding", "action": "prepare", "manifest": str(manifest)})
    finally:
        c.close()
    assert status["available"] is False and fetched == []
