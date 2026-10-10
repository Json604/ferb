from ferb.act.fs.usage import usage

def test_disk_usage_never_follows_links_or_reads_file_bytes(tmp_path, monkeypatch):
    source = tmp_path/'tree'; source.mkdir()
    (source/'file').write_bytes(b'private content')
    elsewhere = tmp_path/'elsewhere'; elsewhere.mkdir(); (elsewhere/'large').write_bytes(b'x'*1000)
    (source/'link').symlink_to(elsewhere)
    result = usage(source)
    assert result['files'] == 2 and result['bytes'] < 1000 and result['complete']
    assert 'private content' not in str(result)
