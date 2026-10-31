"""Local text embeddings for memory search (notes/memory.md decision 8). Must not send text anywhere, run downloaded
code, or embed anything but redacted text.

The model is a static embedding table (model2vec's potion-base-8M, MIT; Ruling in notes/memory.md, step 9b): each
token of a WordPiece vocabulary has one 256-number vector; a text's embedding is the mean of its tokens' vectors,
normalized. No neural network runs at query time, so the cost is a table lookup per token (pure Python, no new
framework), the files are ~31 MB, and the table is memory-mapped (only the rows a text touches are read in).

The files are fetched once from a pinned revision with pinned SHA-256s (`download`) into
`~/.ferb/models/embeddings/<name>` at first use, or read from wherever `[memory] embedding_model` points (the app
bundle can ship them there, step 12). Nothing else is ever downloaded and only these data files are read
(`.json` and `.safetensors`: never code).

Vectors are stored as normalized int8 (`quantize`) with their sign bits (`sign_bits`) in `recall_vectors` (migration
0054). `nearest` ranks by the sign bits' Hamming distance first when there are many, then by the int8 cosine of the
closest few, from an in-memory copy kept per database and model, reloaded when the table changed (`recall_state`
generation, kept by triggers)."""
from __future__ import annotations

import array
import hashlib
import json
import math
import mmap
import operator
import os
import re
import struct
import tempfile
import threading
import unicodedata
from pathlib import Path
from typing import Any, Callable, Iterable
from urllib.request import urlopen

# The pinned model (Ruling, step 9b): minishlab/potion-base-8M, MIT licence, 29,528 tokens x 256 dimensions (float32)
MODEL: dict[str, Any] = {
    "name": "potion-base-8M",
    "repository": "minishlab/potion-base-8M",
    "revision": "bf8b056651a2c21b8d2565580b8569da283cab23",
    "license": "MIT",
    "dimensions": 256,
    "files": {
        "config.json": "2a6ac0e9aaa356a68a5688070db78fc3a464fefe85d2f06a1905ce3718687553",
        "model.safetensors": "f65d0f325faadc1e121c319e2faa41170d3fa07d8c89abd48ca5358d9a223de2",
        "tokenizer.json": "e67e803f624fb4d67dea1c730d06e1067e1b14d830e2c2202569e3ef0f70bb50",
    },
}
# Ruling: what of a text is embedded (a static model's mean dilutes over long text; the keyword index holds it all)
EMBED_CHARS = 2000
MAX_TOKENS = 512                   # model2vec's own default cut
# Ruling: above this many vectors, `nearest` narrows by sign bits to the closest PREFILTER, then scores those exactly
PREFILTER_ABOVE = 1000
PREFILTER = 256


def manifest_ok(data: dict[str, Any]) -> dict[str, Any]:
    """A model description FERB accepts: a pinned commit, a SHA-256 for each file, data files only."""
    if not re.fullmatch(r"[0-9a-f]{40}", str(data.get("revision", ""))):
        raise ValueError("model revision must be an immutable commit hash")
    if not re.fullmatch(r"[\w.-]+/[\w.-]+", str(data.get("repository", ""))):
        raise ValueError("invalid model repository")
    files = data.get("files", {})
    if not files or "tokenizer.json" not in files or not any(n.endswith(".safetensors") for n in files):
        raise ValueError("a model needs a tokenizer and safetensors weights")
    for name, sha in files.items():
        p = Path(name)
        if p.is_absolute() or ".." in p.parts or len(p.parts) != 1 or p.suffix not in (".json", ".safetensors"):
            raise ValueError("model files must be plain data files in one folder")
        if not re.fullmatch(r"[0-9a-f]{64}", str(sha)):
            raise ValueError("every file needs a SHA256 hash")
    if int(data.get("dimensions") or 0) <= 0:
        raise ValueError("the model needs its dimensions")
    return data


def verify(root: Path, data: dict[str, Any]) -> None:
    """Every pinned file is there, a regular file (never a link), with its SHA-256."""
    for name, expected in data["files"].items():
        path = root / name
        if path.is_symlink() or not path.is_file():
            raise ValueError("model file missing or not a regular file: " + name)
        with path.open("rb") as f:
            if hashlib.file_digest(f, "sha256").hexdigest() != expected:
                raise ValueError("model file hash mismatch: " + name)


def default_root(home: Path, data: dict[str, Any] = MODEL) -> Path:
    return home / "models" / "embeddings" / f"{data['name']}@{data['revision'][:12]}"


def download(root: Path, data: dict[str, Any] = MODEL, fetch: Callable[..., Any] = urlopen) -> None:
    """Fetch the pinned files once into `root` (a temporary folder renamed into place after every hash checks)."""
    manifest_ok(data)
    if root.exists():
        verify(root, data)
        return
    root.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
    with tempfile.TemporaryDirectory(prefix=".embedding-", dir=root.parent) as folder:
        tmp = Path(folder) / "model"
        tmp.mkdir()
        for name in data["files"]:
            url = f"https://huggingface.co/{data['repository']}/resolve/{data['revision']}/{name}"
            target = tmp / name
            with fetch(url, timeout=60) as incoming, target.open("wb") as out:
                while chunk := incoming.read(1024 * 1024):
                    out.write(chunk)
                out.flush()
                os.fsync(out.fileno())
            target.chmod(0o600)
        verify(tmp, data)
        os.rename(tmp, root)
        root.chmod(0o700)


# ---------- the tokenizer (BERT normalizer, pre-tokenizer and WordPiece, as the model's tokenizer.json declares) ----

def _punctuation(ch: str) -> bool:
    cp = ord(ch)
    if 33 <= cp <= 47 or 58 <= cp <= 64 or 91 <= cp <= 96 or 123 <= cp <= 126:
        return True
    return unicodedata.category(ch).startswith("P")


def _cjk(cp: int) -> bool:
    return (0x4E00 <= cp <= 0x9FFF or 0x3400 <= cp <= 0x4DBF or 0x20000 <= cp <= 0x2A6DF or 0x2A700 <= cp <= 0x2B73F
            or 0x2B740 <= cp <= 0x2B81F or 0x2B820 <= cp <= 0x2CEAF or 0xF900 <= cp <= 0xFAFF
            or 0x2F800 <= cp <= 0x2FA1F)


class WordPiece:
    def __init__(self, spec: dict[str, Any]) -> None:
        model = spec["model"]
        if model.get("type") != "WordPiece":
            raise ValueError("the embedding model's tokenizer is not WordPiece")
        self.vocab: dict[str, int] = {str(k): int(v) for k, v in model["vocab"].items()}
        self.unk = str(model.get("unk_token") or "[UNK]")
        self.prefix = str(model.get("continuing_subword_prefix") or "##")
        self.max_chars = int(model.get("max_input_chars_per_word") or 100)
        norm = spec.get("normalizer") or {}
        self.lowercase = bool(norm.get("lowercase", True))
        strip = norm.get("strip_accents")
        self.strip_accents = self.lowercase if strip is None else bool(strip)

    def _normalize(self, text: str) -> str:
        out = []
        for ch in text:
            cp = ord(ch)
            if cp == 0 or cp == 0xFFFD or (unicodedata.category(ch).startswith("C") and ch not in "\t\n\r"):
                continue
            if ch.isspace():
                out.append(" ")
            elif _cjk(cp):
                out.append(f" {ch} ")
            else:
                out.append(ch)
        text = "".join(out)
        if self.lowercase:
            text = text.lower()
        if self.strip_accents:
            text = "".join(c for c in unicodedata.normalize("NFD", text) if unicodedata.category(c) != "Mn")
        return text

    def _words(self, text: str) -> Iterable[str]:
        for chunk in text.split():
            word = ""
            for ch in chunk:
                if _punctuation(ch):
                    if word:
                        yield word
                        word = ""
                    yield ch
                else:
                    word += ch
            if word:
                yield word

    def ids(self, text: str) -> list[int]:
        """Token ids, unknown words left out (model2vec drops the unknown token before pooling)."""
        out: list[int] = []
        for word in self._words(self._normalize(text)):
            if len(word) > self.max_chars:
                continue
            start, pieces = 0, list[int]()
            while start < len(word):
                end, found = len(word), None
                while start < end:
                    piece = word[start:end] if start == 0 else self.prefix + word[start:end]
                    if piece in self.vocab:
                        found = self.vocab[piece]
                        break
                    end -= 1
                if found is None:
                    pieces = []
                    break
                pieces.append(found)
                start = end
            out += pieces
            if len(out) >= MAX_TOKENS:
                return out[:MAX_TOKENS]
        return out


class StaticEmbedder:
    """The pinned static model from a verified folder. `encode` is thread-safe (read-only data)."""

    def __init__(self, root: Path, data: dict[str, Any] = MODEL) -> None:
        manifest_ok(data)
        verify(root, data)
        self.dimensions = int(data["dimensions"])
        self.name = f"{data['name']}@{data['revision'][:12]}:{self.dimensions}"
        self.tokenizer = WordPiece(json.loads((root / "tokenizer.json").read_text(encoding="utf-8")))
        weights = next(n for n in data["files"] if n.endswith(".safetensors"))
        self._file = open(root / weights, "rb")
        self._map = mmap.mmap(self._file.fileno(), 0, access=mmap.ACCESS_READ)
        size = struct.unpack("<Q", self._map[:8])[0]
        header = json.loads(self._map[8:8 + size])
        tensor = next(v for k, v in header.items() if k != "__metadata__")
        if tensor["dtype"] != "F32" or len(tensor["shape"]) != 2 or tensor["shape"][1] != self.dimensions:
            raise ValueError("unexpected embedding table shape")
        begin, end = tensor["data_offsets"]
        self.rows = int(tensor["shape"][0])
        self._table = memoryview(self._map)[8 + size + begin:8 + size + end].cast("f")

    def close(self) -> None:
        try:
            self._table.release()
            self._map.close()
        finally:
            self._file.close()

    def floats(self, text: str) -> list[float] | None:
        """The text's normalized mean embedding, or None when no token is known."""
        ids = [i for i in self.tokenizer.ids(text[:EMBED_CHARS]) if 0 <= i < self.rows]
        if not ids:
            return None
        d = self.dimensions
        acc = [0.0] * d
        add = operator.add
        for i in ids:
            acc = list(map(add, acc, self._table[i * d:(i + 1) * d]))
        norm = math.sqrt(sum(x * x for x in acc))
        return [x / norm for x in acc] if norm else None

    def encode(self, text: str, *, query: bool = False) -> list[int] | None:
        values = self.floats(text)
        return quantize(values) if values is not None else None


def quantize(values: list[float]) -> list[int]:
    if not values or any(not math.isfinite(x) for x in values):
        raise ValueError("embedding must contain finite numbers")
    norm = math.sqrt(sum(x * x for x in values))
    if not norm:
        raise ValueError("zero embedding")
    peak = max(abs(x / norm) for x in values)       # normalized int8; the scale is int8's range, not a data cap
    return [round(127 * x / norm / peak) for x in values]


def sign_bits(vector: list[int]) -> bytes:
    """One bit per dimension (set when positive): `nearest`'s cheap first ranking."""
    value = 0
    for n, x in enumerate(vector):
        if x > 0:
            value |= 1 << n
    return value.to_bytes((len(vector) + 7) // 8, "little")


def text_hash(text: str) -> str:
    return hashlib.sha256(text.encode()).hexdigest()


# ---------- the stored vectors ----------

def store(conn: Any, doc: str, kind: str, at: str, model: str, digest: str, vector: list[int]) -> None:
    conn.execute("INSERT OR REPLACE INTO recall_vectors (doc, model, kind, at, text_hash, vector, bits) "
                 "VALUES (?,?,?,?,?,?,?)", (doc, model, kind, at, digest,
                                            array.array("b", vector).tobytes(), sign_bits(vector)))


class _Loaded:
    def __init__(self, generation: int, rows: list[tuple[str, str, int, array.array[int], float]]) -> None:
        self.generation, self.rows = generation, rows


_cache: dict[tuple[str, str], _Loaded] = {}
_cache_lock = threading.Lock()


def _generation(conn: Any) -> int:
    row = conn.execute("SELECT value FROM recall_state WHERE name = 'vectors'").fetchone()
    return int(row[0]) if row else 0


def _loaded(db: Any, model: str) -> _Loaded:
    key = (str(getattr(db, "path", id(db))), model)
    generation = _generation(db.conn)
    with _cache_lock:
        held = _cache.get(key)
        if held is not None and held.generation == generation:
            return held
    rows = []
    for doc, kind, raw, bits in db.conn.execute("SELECT doc, kind, vector, bits FROM recall_vectors WHERE model = ?",
                                                (model,)):
        vec = array.array("b", raw)
        norm = math.sqrt(sum(x * x for x in vec)) or 1.0
        rows.append((str(doc), str(kind), int.from_bytes(bits, "little"), vec, norm))
    held = _Loaded(generation, rows)
    with _cache_lock:
        _cache[key] = held
    return held


def nearest(db: Any, model: str, vector: list[int], kinds: Iterable[str], limit: int = 100
            ) -> list[tuple[str, float]]:
    """The stored docs of `kinds` nearest `vector` by cosine, best first, at most `limit`."""
    wanted = set(kinds)
    rows = [r for r in _loaded(db, model).rows if r[1] in wanted]
    if not rows:
        return []
    q = array.array("b", vector)
    qnorm = math.sqrt(sum(x * x for x in q)) or 1.0
    if len(rows) > PREFILTER_ABOVE:
        qbits = int.from_bytes(sign_bits(vector), "little")
        rows = sorted(rows, key=lambda r: (r[2] ^ qbits).bit_count())[:max(PREFILTER, limit)]
    mul = operator.mul
    scored = [(doc, sum(map(mul, q, vec)) / (qnorm * norm)) for doc, _, _, vec, norm in rows]
    scored.sort(key=lambda s: (-s[1], s[0]))
    return scored[:limit]


def similarity(a: list[int], b: list[int]) -> float:
    na, nb = math.sqrt(sum(x * x for x in a)), math.sqrt(sum(x * x for x in b))
    return sum(map(operator.mul, a, b)) / (na * nb) if na and nb else 0.0
