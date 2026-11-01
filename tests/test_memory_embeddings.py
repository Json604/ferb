"""MEM-6 (step 9b): the local embedding model, a static table read by FERB's own code. Nothing is downloaded here:
the model is a tiny synthetic one built in the test, and the fetch is a fake."""
import hashlib
import io
import json
import math
import struct
from pathlib import Path
from typing import Any

import pytest

from ferb.memory.embeddings import (MODEL, StaticEmbedder, WordPiece, download, manifest_ok, nearest, quantize,
                                    sign_bits, similarity, store, verify)

VOCAB = ["[PAD]", "[UNK]", "piano", "music", "permit", "##s", "play", ",", "!"]


def tiny_model(root: Path) -> dict[str, Any]:
    """A 4-dimension table: piano and music point the same way, permit another."""
    rows = {"piano": [1, 0, 0, 0], "music": [0.9, 0.1, 0, 0], "permit": [0, 0, 1, 0], "##s": [0, 0, 0, 1],
            "play": [0.5, 0.5, 0, 0], ",": [0, 0, 0, 0.1], "!": [0, 0, 0, 0.1]}
    data = b"".join(struct.pack("<4f", *rows.get(t, [0, 0, 0, 0])) for t in VOCAB)
    header = json.dumps({"embeddings": {"dtype": "F32", "shape": [len(VOCAB), 4], "data_offsets": [0, len(data)]}})
    root.mkdir(parents=True, exist_ok=True)
    (root / "model.safetensors").write_bytes(struct.pack("<Q", len(header)) + header.encode() + data)
    (root / "tokenizer.json").write_text(json.dumps({
        "normalizer": {"type": "BertNormalizer", "lowercase": True, "strip_accents": None},
        "model": {"type": "WordPiece", "unk_token": "[UNK]", "continuing_subword_prefix": "##",
                  "max_input_chars_per_word": 100, "vocab": {t: i for i, t in enumerate(VOCAB)}}}))
    (root / "config.json").write_text("{}")
    files = {n: hashlib.sha256((root / n).read_bytes()).hexdigest()
             for n in ("config.json", "model.safetensors", "tokenizer.json")}
    return {"name": "tiny", "repository": "test/tiny", "revision": "a" * 40, "dimensions": 4, "files": files}


# decision: MEM-25
def test_the_pinned_model_is_data_files_with_hashes() -> None:
    manifest_ok(MODEL)
    for bad in ({**MODEL, "revision": "main"}, {**MODEL, "files": {**MODEL["files"], "run.py": "0" * 64}},
                {**MODEL, "files": {"../x.json": "0" * 64, "model.safetensors": "0" * 64, "tokenizer.json": "0" * 64}}):
        with pytest.raises(ValueError):
            manifest_ok(bad)


# decision: MEM-25
def test_the_tokenizer_splits_like_bert_and_drops_unknown_words() -> None:
    w = WordPiece(json.loads(json.dumps({"normalizer": {"lowercase": True},
                                         "model": {"type": "WordPiece", "vocab": {t: i for i, t in enumerate(VOCAB)}}})))
    assert w.ids("Piano, PIANOS!") == [2, 7, 2, 5, 8]          # case folded, punctuation split, a "##s" piece
    assert w.ids("pianó zebra") == [2]                         # an accent stripped; an unknown word dropped whole


# decision: MEM-25
def test_a_text_is_the_normalized_mean_of_its_tokens(tmp_path: Path) -> None:
    root = tmp_path / "tiny"
    model = StaticEmbedder(root, tiny_model(root))
    try:
        piano, music, permit = (model.encode(t) for t in ("piano", "music", "permit"))
        assert piano and music and permit
        assert similarity(piano, music) > 0.9 and similarity(piano, permit) == 0
        assert model.encode("zebra") is None                    # nothing the model knows
        floats = model.floats("play piano")
        assert floats is not None and math.isclose(sum(x * x for x in floats), 1.0, rel_tol=1e-6)
    finally:
        model.close()


# decision: MEM-25, MEM-26
def test_a_changed_file_is_refused(tmp_path: Path) -> None:
    root = tmp_path / "tiny"
    data = tiny_model(root)
    (root / "config.json").write_text('{"changed": true}')
    with pytest.raises(ValueError, match="hash mismatch"):
        verify(root, data)


# decision: MEM-26
def test_the_fetch_checks_every_hash_before_the_folder_appears(tmp_path: Path) -> None:
    source = tmp_path / "source"
    data = tiny_model(source)
    fetched: list[str] = []

    def fetch(url: str, timeout: float = 0) -> Any:
        fetched.append(url)
        return io.BytesIO((source / url.rsplit("/", 1)[1]).read_bytes())
    root = tmp_path / "models" / "tiny"
    download(root, data, fetch)
    assert root.is_dir() and len(fetched) == 3 and all(data["revision"] in u for u in fetched)
    download(root, data, lambda *a, **k: pytest.fail("fetched twice"))          # already there: verified, not fetched
    bad = {**data, "files": {**data["files"], "config.json": "0" * 64}}
    with pytest.raises(ValueError):
        download(tmp_path / "models" / "bad", bad, fetch)
    assert not (tmp_path / "models" / "bad").exists()


def test_quantized_vectors_rank_by_cosine_with_a_sign_bit_prefilter(tmp_path: Path) -> None:
    from ferb.store.db import Database
    db = Database(tmp_path / "v.db")
    db.migrate()
    assert quantize([3.0, -4.0]) == [95, -127] and sign_bits([1, -1, 0, 5]) == bytes([0b1001])
    for n in range(1200):                                       # past the prefilter's size
        store(db.conn, f"doc{n}", "chat", "t", "m", "h", [1 if n % 2 else -1] * 4 + [-1] * 4)
    store(db.conn, "best", "chat", "t", "m", "h", [5] * 8)
    store(db.conn, "fact", "fact", "t", "m", "h", [5] * 8)
    found = nearest(db, "m", [3] * 8, ["chat"], 3)
    assert found[0][0] == "best" and "fact" not in [d for d, _ in found]
    db.conn.execute("DELETE FROM recall_vectors WHERE doc = 'best'")
    assert nearest(db, "m", [3] * 8, ["chat"], 1)[0][0] != "best"            # the cached copy reloads on a change
    db.close()


# decision: MEM-26
def test_the_model_ferb_app_ships_loads_from_the_bundle_with_no_network(tmp_path: Path,
                                                                         monkeypatch: pytest.MonkeyPatch) -> None:
    """Step 12b: FERB.app ships the model (Contents/Resources/models), so a new install needs no download: the daemon
    running from the app reads it there, never fetches it and writes nothing under ~/.ferb/models."""
    import asyncio
    import shutil
    from types import SimpleNamespace
    from ferb.config import Config
    from ferb.daemon.areas.memory import MemoryGlue
    from ferb.device import install
    from ferb.memory import embeddings
    app = tmp_path / "Applications" / "FERB.app"
    data = tiny_model(tmp_path / "built")
    shutil.copytree(tmp_path / "built", embeddings.default_root(app / install.BUNDLE_MODELS.parent, data))
    monkeypatch.setattr(embeddings, "MODEL", data)
    monkeypatch.setattr(install, "app_bundle", lambda prefix=None: app)

    def no_network(*args: Any, **kwargs: Any) -> Any:
        raise AssertionError("the model was fetched")
    monkeypatch.setattr(embeddings, "download", no_network)
    monkeypatch.setattr(embeddings, "urlopen", no_network)
    journaled: list[tuple[str, Any]] = []

    class Daemon(MemoryGlue):
        def __init__(self) -> None:
            self.home = tmp_path / ".ferb"
            self.config = Config(self.home)
            self.learning = SimpleNamespace(embedder=None)

        def _journal(self) -> Any:
            return SimpleNamespace(append=lambda kind, row: journaled.append((kind, row)))

        def _redact_text(self, text: str) -> str:
            return text
    daemon = Daemon()
    asyncio.run(daemon._load_embedder())
    assert daemon.learning.embedder is not None and daemon.learning.embedder.dimensions == 4
    assert journaled == [] and not (tmp_path / ".ferb" / "models").exists()
