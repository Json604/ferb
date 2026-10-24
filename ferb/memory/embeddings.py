"""Local-only optional MLX embeddings, pinned artifacts and measured adoption (M8).

No text goes to a network service. Downloads contain public model files only. Candidate models remain disabled
until the owner-history evaluation beats keyword search; failures leave keyword retrieval available.
"""
from __future__ import annotations

import asyncio
import hashlib
import array
import sqlite3
import json
import math
import os
import re
import tempfile
import threading
from pathlib import Path
from typing import Any, Callable
from urllib.request import urlopen

from ferb.memory.learning import LearningRefused, digest, encoded, search_text


def manifest(path: Path) -> dict[str, Any]:
    data = json.loads(path.read_text())
    if not re.fullmatch(r'[0-9a-f]{40}', str(data.get('revision', ''))):
        raise ValueError('model revision must be an immutable commit hash')
    if not re.fullmatch(r'[\w.-]+/[\w.-]+', str(data.get('repository', ''))):
        raise ValueError('invalid model repository')
    files = data.get('files', {})
    if not files or 'config.json' not in files or not any(n.endswith('.safetensors') for n in files):
        raise ValueError('manifest needs config and safetensors weights')
    for name, sha in files.items():
        p = Path(name)
        if p.is_absolute() or '..' in p.parts or p.suffix not in ('.json', '.safetensors', '.txt', '.model', '.tiktoken'):
            raise ValueError('model manifest contains an unsafe path or executable file')
        if not re.fullmatch(r'[0-9a-f]{64}', str(sha)):
            raise ValueError('every file needs a SHA256 hash')
    if data.get('dimensions') not in (256, 512):
        raise ValueError('choose the planned 256 or 512 embedding dimensions')
    return data


def verify(root: Path, data: dict[str, Any]) -> None:
    actual = {str(p.relative_to(root)) for p in root.rglob('*') if p.is_file()}
    if actual != set(data['files']):
        raise ValueError('model directory differs from its pinned manifest')
    for name, expected in data['files'].items():
        path = root / name
        if path.is_symlink() or any(p.is_symlink() for p in path.parents if p != root.parent):
            raise ValueError('model artifacts cannot be symlinks')
        with path.open('rb') as f:
            if hashlib.file_digest(f, 'sha256').hexdigest() != expected:
                raise ValueError('model file hash mismatch: ' + name)


def extra_missing() -> str | None:
    """The optional `memory` extra's module a local embedding model needs, when this install lacks it (the packaged
    app doesn't bundle it): checked before anything is downloaded (review M8). sqlite-vec has a Python fallback."""
    import importlib.util
    return None if importlib.util.find_spec("mlx_embeddings") is not None else "mlx_embeddings"


def download(root: Path, data: dict[str, Any], fetch: Callable[..., Any] = urlopen) -> None:
    root.parent.mkdir(parents=True, exist_ok=True)
    if root.exists():
        verify(root, data)
        return
    with tempfile.TemporaryDirectory(prefix='.embedding-', dir=root.parent) as folder:
        tmp = Path(folder)
        for name, sha in data['files'].items():
            target = tmp / name
            target.parent.mkdir(parents=True, exist_ok=True)
            url = f"https://huggingface.co/{data['repository']}/resolve/{data['revision']}/{name}"
            with fetch(url) as incoming, target.open('wb') as out:
                while chunk := incoming.read(1024 * 1024):
                    out.write(chunk)
                out.flush()
                os.fsync(out.fileno())
            target.chmod(0o600)
        verify(tmp, data)
        os.rename(tmp, root)
        root.chmod(0o700)


class LocalMLX:
    """Use only a verified local directory; no provider fallback and no remote Python model code."""
    def __init__(self, root: Path, data: dict[str, Any]) -> None:
        verify(root, data)
        from mlx_embeddings.utils import load  # optional memory extra
        self.model, self.tokenizer = load(str(root), tokenizer_config={'local_files_only': True,
                                                                       'trust_remote_code': False})
        self.dimensions = data['dimensions']
        self.name = data['repository'] + '@' + data['revision'] + ':' + str(self.dimensions) + ':' + digest(data)
        self.query_prefix = str(data.get('query_prefix', ''))
        self._encode_lock = threading.Lock()

    def encode(self, text: str, *, query: bool = False) -> list[int]:
        from mlx_embeddings.utils import generate
        value = (self.query_prefix if query else '') + text
        # Do not silently clip long evidence. The model may report an unsupported context length.
        # Idle indexing runs in a thread; a new task can query this same loaded model.
        with self._encode_lock:
            out = generate(self.model, self.tokenizer, texts=value, truncation=False).text_embeds[0].tolist()
        if len(out) < self.dimensions:
            raise ValueError('embedding model returned fewer dimensions than its manifest')
        return quantize(out[:self.dimensions])


def quantize(values: list[float]) -> list[int]:
    if not values or any(not math.isfinite(x) for x in values):
        raise ValueError('embedding must contain finite numbers')
    norm = math.sqrt(sum(x * x for x in values))
    if not norm:
        raise ValueError('zero embedding')
    # Normalized int8; scale is the representable int8 range, not a data cap.
    peak = max(abs(x / norm) for x in values)
    return [round(127 * x / norm / peak) for x in values]


def _unembedded(learning: Any, node: dict[str, Any], model: Any) -> None:
    """A node with nothing to match (a rollup, an intent label: `search_text` is empty) is never embedded; a vector an
    earlier index made of its JSON goes, so similarity can't rank it either."""
    learning.db.conn.execute('DELETE FROM learning_vectors WHERE node_id=? AND model=?', (node['id'], model.name))


def index(learning: Any, model: Any) -> int:
    count = 0
    for node in learning.items():
        text = search_text(node['kind'], node['key'], node['body'])     # what keyword search matches too (L36 §3)
        if not text:
            _unembedded(learning, node, model)
            continue
        sha = digest(text)
        old = learning.db.conn.execute('SELECT text_hash FROM learning_vectors WHERE node_id=? AND model=?',
                                       (node['id'], model.name)).fetchone()
        if old and old[0] == sha:
            continue
        vec = model.encode(text)
        learning.db.conn.execute('INSERT OR REPLACE INTO learning_vectors VALUES (?,?,?,?,?)',
            (node['id'], model.name, array.array('b', vec).tobytes(), node['recorded_at'][:7], sha))
        count += 1
    return count


def ranked(db: Any, model: Any, query: str, *, vector: list[int] | None = None) -> list[str]:
    """Nodes nearest the query. `vector`: the query's embedding when the caller already computed it off the event
    loop (L28); otherwise it is computed here."""
    vector = vector if vector is not None else model.encode(query, query=True)
    # sqlite-vec performs int8 distance locally; Python's exact cosine is a portable offline fallback.
    try:
        import sqlite_vec
        db.conn.enable_load_extension(True)
        try:
            sqlite_vec.load(db.conn)
        finally:
            db.conn.enable_load_extension(False)
        return [r[0] for r in db.conn.execute('SELECT node_id FROM learning_vectors WHERE model=? '
            'ORDER BY vec_distance_cosine(vec_int8(vector_blob),vec_int8(?)),node_id',
            (model.name, array.array('b', vector).tobytes()))]
    except (ImportError, AttributeError, sqlite3.OperationalError):
        def score(raw: bytes) -> float:
            v = list(array.array('b', raw))
            if len(v) != len(vector):
                raise ValueError('stored vector dimension differs from query')
            return sum(a * b for a, b in zip(v, vector)) / math.sqrt(sum(x*x for x in v) * sum(x*x for x in vector))
        return [r[0] for r in sorted(db.conn.execute('SELECT node_id,vector_blob FROM learning_vectors WHERE model=?',
                                                    (model.name,)), key=lambda r: (-score(r[1]), r[0]))]


def evaluate(learning: Any, model: Any, cases: list[dict[str, Any]], *, prepare: bool = True) -> dict[str, Any]:
    """Paired reciprocal-rank measurement, grounded in recorded owner requests and owner-labelled relevant IDs.

    No fixed minimum corpus size is invented; the report exposes n and the plan calls for ~50–100 owner queries.
    Adoption is a separate explicit owner action and also requires measured hybrid improvement.
    """
    if not cases:
        raise LearningRefused('evaluation needs labelled owner-history queries')
    for case in cases:
        source = learning.db.conn.execute('SELECT request_text FROM episodes WHERE task_id=?',
                                          (case.get('task_id'),)).fetchone()
        if not source or case.get('query') != source[0] or not case.get('relevant'):
            raise LearningRefused('query must match its real episode request and have relevance labels')
        if any(learning.get(i) is None for i in case['relevant']):
            raise LearningRefused('label references missing memory')
    if prepare:
        index(learning, model)
    previous = learning.embedder
    rows = []
    try:
        for case in cases:
            result = {'task_id': case['task_id']}
            for name, embedder in (('keyword', None), ('hybrid', model)):
                learning.embedder = embedder
                hits = learning.search(case['query'])
                # Hold out the query task itself to avoid measuring identity lookup as helpful memory.
                held_out = learning.descendants(['episode:' + case['task_id']])
                hits = [h for h in hits if h['id'] not in held_out and h['body'].get('task_id') != case['task_id']]
                pos = next((n + 1 for n, hit in enumerate(hits) if hit['id'] in case['relevant']), None)
                result[name] = 1 / pos if pos else 0.0
            rows.append(result)
    finally:
        learning.embedder = previous
    scores = {k: sum(r[k] for r in rows) / len(rows) for k in ('keyword', 'hybrid')}
    out = {'queries': len(rows), 'mrr': scores, 'improved': scores['hybrid'] > scores['keyword'], 'rows': rows,
           'answer_quality': 'not measured; paired with/without-memory task evaluation still required'}
    ident = digest([model.name, cases, out])
    learning.db.conn.execute('INSERT OR REPLACE INTO learning_embedding_evals VALUES (?,?,?,?,?,0)',
                             (ident, model.name, digest(cases), learning.now(), encoded(out)))
    return out | {'id': ident, 'model': model.name}


def adopt(learning: Any, model: Any, evaluation_id: str) -> None:
    row = learning.db.conn.execute('SELECT * FROM learning_embedding_evals WHERE id=? AND model=?',
                                   (evaluation_id, model.name)).fetchone()
    if row is None or json.loads(row['result_json']).get('improved') is not True:
        raise LearningRefused('this exact model has not beaten keyword retrieval on owner history')
    learning.db.conn.execute('UPDATE learning_embedding_evals SET adopted=0')
    learning.db.conn.execute('UPDATE learning_embedding_evals SET adopted=1 WHERE id=?', (evaluation_id,))
    learning.embedder = model


async def index_async(learning: Any, model: Any) -> int:
    """Keep inference off the daemon thread; only the owner loop touches SQLite."""
    count = 0
    for node in learning.items():
        text = search_text(node['kind'], node['key'], node['body'])     # what keyword search matches too (L36 §3)
        if not text:
            _unembedded(learning, node, model)
            continue
        sha = digest(text)
        old = learning.db.conn.execute('SELECT text_hash FROM learning_vectors WHERE node_id=? AND model=?',
                                       (node['id'], model.name)).fetchone()
        if old and old[0] == sha:
            continue
        vector = await asyncio.to_thread(model.encode, text)
        if learning.get(node['id']) is None:
            continue  # owner may forget evidence during inference
        learning.db.conn.execute('INSERT OR REPLACE INTO learning_vectors VALUES (?,?,?,?,?)',
            (node['id'], model.name, array.array('b', vector).tobytes(), node['recorded_at'][:7], sha))
        count += 1
    return count


async def evaluate_async(learning: Any, model: Any, cases: list[dict[str, Any]]) -> dict[str, Any]:
    await index_async(learning, model)
    vectors = {}
    for case in cases:
        vectors[case['query']] = await asyncio.to_thread(model.encode, case['query'], query=True)
    class PreparedQueries:
        name = model.name
        def encode(self, text: str, *, query: bool = False) -> list[int]:
            if not query:
                raise ValueError('evaluation corpus must be prepared before scoring')
            return vectors[text]
    return evaluate(learning, PreparedQueries(), cases, prepare=False)
