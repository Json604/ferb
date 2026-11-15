//! Local text embeddings (`embeddings.py`, MEM-6, MEM-25, MEM-26): potion-base-8M's static table read by FERB's
//! own tokenizer from memory-mapped safetensors; a text is the normalized mean of its tokens' rows (float32 rows
//! accumulated as float64), stored as normalized int8 with sign bits; ranking is the Hamming distance of the sign
//! bits first when there are many, then the int8 cosine. Nothing here downloads (MEM-26: the app ships the files;
//! `download` is the Python checkout's setup and is not ported).

use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value};
use sha2::{Digest, Sha256};

use crate::pymath;
use crate::pystr;
use crate::pyval::{self, Crash};
use crate::terms::hex;
use crate::tokenizer::WordPiece;
use crate::Error;

/// The pinned model (`embeddings.py:38-49`).
pub const MODEL_NAME: &str = "potion-base-8M";
pub const MODEL_REPOSITORY: &str = "minishlab/potion-base-8M";
pub const MODEL_REVISION: &str = "bf8b056651a2c21b8d2565580b8569da283cab23";
pub const MODEL_DIMENSIONS: i64 = 256;
pub const MODEL_FILES: [(&str, &str); 3] = [
    (
        "config.json",
        "2a6ac0e9aaa356a68a5688070db78fc3a464fefe85d2f06a1905ce3718687553",
    ),
    (
        "model.safetensors",
        "f65d0f325faadc1e121c319e2faa41170d3fa07d8c89abd48ca5358d9a223de2",
    ),
    (
        "tokenizer.json",
        "e67e803f624fb4d67dea1c730d06e1067e1b14d830e2c2202569e3ef0f70bb50",
    ),
];
/// What of a text is embedded, in characters.
pub const EMBED_CHARS: usize = 2000;
/// Above this many vectors `nearest` narrows by sign bits first, to the closest `PREFILTER`.
pub const PREFILTER_ABOVE: usize = 1000;
pub const PREFILTER: usize = 256;

/// The pinned model's description, as `MODEL` is in Python.
pub fn model() -> Value {
    let files: Map<String, Value> = MODEL_FILES
        .iter()
        .map(|(n, h)| ((*n).to_owned(), Value::String((*h).to_owned())))
        .collect();
    serde_json::json!({
        "name": MODEL_NAME, "repository": MODEL_REPOSITORY, "revision": MODEL_REVISION, "license": "MIT",
        "dimensions": MODEL_DIMENSIONS, "files": files,
    })
}

fn refused(msg: &str) -> Error {
    Error::Refused(msg.to_owned())
}

fn fullmatch_hex(s: &str, n: usize) -> bool {
    s.len() == n
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

fn repository_ok(s: &str) -> bool {
    let seg =
        |p: &str| !p.is_empty() && p.chars().all(|c| pystr::re_word(c) || c == '.' || c == '-');
    match s.split_once('/') {
        Some((a, b)) => seg(a) && seg(b),
        None => false,
    }
}

/// `PurePosixPath(name)`: (absolute, its parts).
fn posix_parts(name: &str) -> (bool, Vec<&str>) {
    let absolute = name.starts_with('/');
    let parts = name
        .split('/')
        .filter(|p| !p.is_empty() && *p != ".")
        .collect();
    (absolute, parts)
}

/// `PurePath.suffix` of a last part.
fn suffix(name: &str) -> &str {
    match name.rfind('.') {
        Some(i) if i > 0 && i < name.len() - 1 => &name[i..],
        _ => "",
    }
}

/// `data.get(key, default)` on the manifest (a dict).
fn field<'a>(data: &'a Value, key: &str) -> Result<Option<&'a Value>, Error> {
    Ok(pyval::get(data, key)?)
}

/// `manifest_ok`: a pinned commit, a SHA-256 for each file, data files only in one folder, a dimension.
pub fn manifest_ok(data: &Value) -> Result<(), Error> {
    let revision = field(data, "revision")?.map_or(String::new(), pyval::to_str);
    if !fullmatch_hex(&revision, 40) {
        return Err(refused("model revision must be an immutable commit hash"));
    }
    let repository = field(data, "repository")?.map_or(String::new(), pyval::to_str);
    if !repository_ok(&repository) {
        return Err(refused("invalid model repository"));
    }
    let files = match field(data, "files")? {
        None => None,
        Some(Value::Object(m)) if !m.is_empty() => Some(m),
        Some(v) if !pyval::truthy(v) => None,
        Some(_) => return Err(Crash("AttributeError").into()),
    };
    let Some(files) = files else {
        return Err(refused("a model needs a tokenizer and safetensors weights"));
    };
    if !files.contains_key("tokenizer.json") || !files.keys().any(|n| n.ends_with(".safetensors")) {
        return Err(refused("a model needs a tokenizer and safetensors weights"));
    }
    for (name, sha) in files {
        let (absolute, parts) = posix_parts(name);
        let last = parts.last().copied().unwrap_or("");
        if absolute
            || parts.contains(&"..")
            || parts.len() != 1
            || !matches!(suffix(last), ".json" | ".safetensors")
        {
            return Err(refused(
                "model files must be plain data files in one folder",
            ));
        }
        if !fullmatch_hex(&pyval::to_str(sha), 64) {
            return Err(refused("every file needs a SHA256 hash"));
        }
    }
    let dims = match field(data, "dimensions")? {
        Some(v) if pyval::truthy(v) => pyval::int(v)?,
        _ => 0,
    };
    if dims <= 0 {
        return Err(refused("the model needs its dimensions"));
    }
    Ok(())
}

fn files_of(data: &Value) -> Result<&Map<String, Value>, Error> {
    match pyval::get(data, "files")? {
        Some(Value::Object(m)) => Ok(m),
        _ => Err(Crash("KeyError").into()),
    }
}

/// `verify`: every pinned file there, a regular file (never a link), with its SHA-256.
pub fn verify(root: &Path, data: &Value) -> Result<(), Error> {
    for (name, expected) in files_of(data)? {
        let path = root.join(name);
        let regular = fs::symlink_metadata(&path).is_ok_and(|m| !m.file_type().is_symlink())
            && fs::metadata(&path).is_ok_and(|m| m.is_file());
        if !regular {
            return Err(Error::Refused(format!(
                "model file missing or not a regular file: {name}"
            )));
        }
        let mut hasher = Sha256::new();
        let mut file = fs::File::open(&path)?;
        std::io::copy(&mut file, &mut hasher)?;
        if hex(&hasher.finalize()) != pyval::to_str(expected) {
            return Err(Error::Refused(format!("model file hash mismatch: {name}")));
        }
    }
    Ok(())
}

/// `default_root(home)`: `<home>/models/embeddings/<name>@<revision[:12]>`.
pub fn default_root(home: &Path) -> PathBuf {
    home.join("models")
        .join("embeddings")
        .join(format!("{MODEL_NAME}@{}", &MODEL_REVISION[..12]))
}

/// What turns a text into the vector search stores (the static model, or a test's stand-in).
pub trait Embedder {
    /// `<name>@<revision[:12]>:<dimensions>`: which model's vectors these are.
    fn name(&self) -> &str;
    fn dimensions(&self) -> usize;
    /// The text's normalized int8 vector, or `None` when the model knows none of its tokens.
    fn encode(&self, text: &str) -> Option<Vec<i8>>;
}

/// The weights, mapped read-only where the platform can (only the rows a text touches are read in).
struct Table {
    #[cfg(unix)]
    map: Mapped,
    #[cfg(not(unix))]
    bytes: Vec<u8>,
}

#[cfg(unix)]
struct Mapped {
    ptr: *mut libc::c_void,
    len: usize,
}

// SAFETY: the mapping is read-only and private; nothing writes through it, so sharing it between threads is
// sharing immutable bytes.
#[cfg(unix)]
unsafe impl Send for Mapped {}
// SAFETY: as above: only reads.
#[cfg(unix)]
unsafe impl Sync for Mapped {}

#[cfg(unix)]
impl Drop for Mapped {
    fn drop(&mut self) {
        if self.len > 0 {
            // SAFETY: `ptr`/`len` are exactly what mmap returned, unmapped once here.
            unsafe {
                libc::munmap(self.ptr, self.len);
            }
        }
    }
}

impl Table {
    #[cfg(unix)]
    fn open(path: &Path) -> Result<Table, Error> {
        use std::os::fd::AsRawFd;
        let file = fs::File::open(path)?;
        let len = usize::try_from(file.metadata()?.len()).map_err(|_| Crash("OverflowError"))?;
        if len == 0 {
            return Ok(Table {
                map: Mapped {
                    ptr: std::ptr::null_mut(),
                    len: 0,
                },
            });
        }
        // SAFETY: a fresh read-only private mapping of an open file of `len` bytes; checked for MAP_FAILED.
        let ptr = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                len,
                libc::PROT_READ,
                libc::MAP_PRIVATE,
                file.as_raw_fd(),
                0,
            )
        };
        if ptr == libc::MAP_FAILED {
            return Err(std::io::Error::last_os_error().into());
        }
        Ok(Table {
            map: Mapped { ptr, len },
        })
    }

    #[cfg(not(unix))]
    fn open(path: &Path) -> Result<Table, Error> {
        Ok(Table {
            bytes: fs::read(path)?,
        })
    }

    fn bytes(&self) -> &[u8] {
        #[cfg(unix)]
        {
            if self.map.len == 0 {
                return &[];
            }
            // SAFETY: the mapping is `len` readable bytes that live as long as `self`.
            unsafe { std::slice::from_raw_parts(self.map.ptr as *const u8, self.map.len) }
        }
        #[cfg(not(unix))]
        {
            &self.bytes
        }
    }
}

/// `StaticEmbedder`: the pinned static model from a verified folder. `encode` only reads.
pub struct StaticEmbedder {
    pub dimensions: usize,
    name: String,
    pub tokenizer: WordPiece,
    table: Table,
    start: usize,
    pub rows: i64,
}

impl StaticEmbedder {
    /// Open the model at `root` described by `data` (checked, hashes verified).
    pub fn open(root: &Path, data: &Value) -> Result<StaticEmbedder, Error> {
        manifest_ok(data)?;
        verify(root, data)?;
        let dims = pyval::int(pyval::get(data, "dimensions")?.ok_or(Crash("KeyError"))?)?;
        let revision = pyval::to_str(pyval::get(data, "revision")?.ok_or(Crash("KeyError"))?);
        let model_name = pyval::to_str(pyval::get(data, "name")?.ok_or(Crash("KeyError"))?);
        let name = format!("{model_name}@{}:{dims}", &revision[..12]);
        let spec_text = fs::read(root.join("tokenizer.json"))?;
        let spec_text = String::from_utf8(spec_text).map_err(|_| Crash("UnicodeDecodeError"))?;
        let spec: Value = serde_json::from_str(&spec_text).map_err(|_| Crash("JSONDecodeError"))?;
        let tokenizer = WordPiece::from_spec(&spec)?;
        let weights = files_of(data)?
            .keys()
            .find(|n| n.ends_with(".safetensors"))
            .ok_or(Crash("StopIteration"))?
            .clone();
        let table = Table::open(&root.join(weights))?;
        let bytes = table.bytes();
        let size = bytes
            .get(..8)
            .map(|b| u64::from_le_bytes(b.try_into().unwrap_or([0; 8])))
            .ok_or(Crash("JSONDecodeError"))?;
        let size = usize::try_from(size).map_err(|_| Crash("JSONDecodeError"))?;
        let header_bytes = bytes
            .get(8..8usize.saturating_add(size))
            .ok_or(Crash("JSONDecodeError"))?;
        let header_bytes = header_bytes
            .strip_prefix(b"\xef\xbb\xbf")
            .unwrap_or(header_bytes);
        let header: Value =
            serde_json::from_slice(header_bytes).map_err(|_| Crash("JSONDecodeError"))?;
        let Value::Object(header) = header else {
            return Err(Crash("AttributeError").into());
        };
        let tensor = header
            .iter()
            .find(|(k, _)| k.as_str() != "__metadata__")
            .map(|(_, v)| v)
            .ok_or(Crash("StopIteration"))?;
        let shape_refused = || refused("unexpected embedding table shape");
        let get = |key: &str| -> Result<&Value, Error> {
            Ok(pyval::get(tensor, key)?.ok_or(Crash("KeyError"))?)
        };
        let shape = match get("shape")? {
            Value::Array(a) => a,
            _ => return Err(Crash("TypeError").into()),
        };
        let dims_match = shape
            .get(1)
            .and_then(Value::as_f64)
            .is_some_and(|d| d == dims as f64);
        if get("dtype")? != "F32" || shape.len() != 2 || !dims_match {
            return Err(shape_refused());
        }
        let offsets = match get("data_offsets")? {
            Value::Array(a) if a.len() == 2 => a,
            _ => return Err(Crash("ValueError").into()),
        };
        let begin = pyval::int(&offsets[0])?;
        let end = pyval::int(&offsets[1])?;
        let rows = pyval::int(&shape[0])?;
        let dims = usize::try_from(dims).map_err(|_| shape_refused())?;
        // Deliberate difference from Python: a table whose data does not hold `rows` x `dims` floats inside the
        // file is refused (Python's memoryview slices clamp silently and later rows read short). The pinned
        // file always holds them, so the two agree on every file the hashes accept.
        let start = 8 + size;
        let fits = begin >= 0
            && end >= begin
            && rows >= 0
            && (start as i64)
                .checked_add(end)
                .is_some_and(|e| e <= bytes.len() as i64)
            && (end - begin) as u128 >= rows as u128 * dims as u128 * 4;
        if !fits {
            return Err(shape_refused());
        }
        Ok(StaticEmbedder {
            dimensions: dims,
            name,
            tokenizer,
            start: start + begin as usize,
            table,
            rows,
        })
    }

    fn value(&self, row: usize, col: usize) -> f64 {
        let at = self.start + (row * self.dimensions + col) * 4;
        let b = &self.table.bytes()[at..at + 4];
        f64::from(f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    /// `floats`: the text's normalized mean embedding, or `None` when no token is known.
    pub fn floats(&self, text: &str) -> Option<Vec<f64>> {
        let ids: Vec<usize> = self
            .tokenizer
            .ids(pystr::prefix(text, EMBED_CHARS))
            .into_iter()
            .filter(|&i| 0 <= i && i < self.rows)
            .map(|i| i as usize)
            .collect();
        if ids.is_empty() {
            return None;
        }
        let mut acc = vec![0.0f64; self.dimensions];
        for i in ids {
            for (j, a) in acc.iter_mut().enumerate() {
                *a += self.value(i, j);
            }
        }
        let norm = pymath::sum(acc.iter().map(|x| x * x)).sqrt();
        if norm == 0.0 {
            return None;
        }
        Some(acc.iter().map(|x| x / norm).collect())
    }
}

impl Embedder for StaticEmbedder {
    fn name(&self) -> &str {
        &self.name
    }

    fn dimensions(&self) -> usize {
        self.dimensions
    }

    fn encode(&self, text: &str) -> Option<Vec<i8>> {
        quantize(&self.floats(text)?).ok()
    }
}

/// Why `quantize` refused (Python's exception and message).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QuantizeError {
    pub kind: &'static str,
    pub message: &'static str,
}

/// `quantize`: normalized, scaled so the largest component is ±127, rounded half to even.
pub fn quantize(values: &[f64]) -> Result<Vec<i8>, QuantizeError> {
    if values.is_empty() || values.iter().any(|x| !x.is_finite()) {
        return Err(QuantizeError {
            kind: "ValueError",
            message: "embedding must contain finite numbers",
        });
    }
    let norm = pymath::sum(values.iter().map(|x| x * x)).sqrt();
    if norm == 0.0 {
        return Err(QuantizeError {
            kind: "ValueError",
            message: "zero embedding",
        });
    }
    let mut peak = (values[0] / norm).abs();
    for x in &values[1..] {
        let v = (x / norm).abs();
        if v > peak {
            peak = v;
        }
    }
    if peak == 0.0 {
        return Err(QuantizeError {
            kind: "ZeroDivisionError",
            message: "float division by zero",
        });
    }
    Ok(values
        .iter()
        .map(|x| (127.0 * x / norm / peak).round_ties_even() as i8)
        .collect())
}

/// `sign_bits`: one bit per dimension, set when positive, little-endian.
pub fn sign_bits(vector: &[i8]) -> Vec<u8> {
    let mut out = vec![0u8; vector.len().div_ceil(8)];
    for (n, &x) in vector.iter().enumerate() {
        if x > 0 {
            out[n / 8] |= 1 << (n % 8);
        }
    }
    out
}

/// `text_hash`: the SHA-256 of the text's UTF-8.
pub fn text_hash(text: &str) -> String {
    hex(&Sha256::digest(text.as_bytes()))
}

fn int_norm(v: &[i8]) -> f64 {
    let s: i64 = v.iter().map(|&x| i64::from(x) * i64::from(x)).sum();
    (s as f64).sqrt()
}

fn dot(a: &[i8], b: &[i8]) -> i64 {
    a.iter()
        .zip(b)
        .map(|(&x, &y)| i64::from(x) * i64::from(y))
        .sum()
}

/// `similarity`: the cosine of two int8 vectors (0.0 when either is zero).
pub fn similarity(a: &[i8], b: &[i8]) -> f64 {
    let (na, nb) = (int_norm(a), int_norm(b));
    if na != 0.0 && nb != 0.0 {
        dot(a, b) as f64 / (na * nb)
    } else {
        0.0
    }
}

/// One stored vector as `nearest` holds it in memory.
#[derive(Clone, Debug)]
pub struct Stored {
    pub doc: String,
    pub kind: String,
    pub bits: Vec<u8>,
    pub vector: Vec<i8>,
    pub norm: f64,
}

impl Stored {
    pub fn new(doc: String, kind: String, vector: Vec<i8>, bits: Vec<u8>) -> Stored {
        let norm = int_norm(&vector);
        Stored {
            doc,
            kind,
            bits,
            norm: if norm == 0.0 { 1.0 } else { norm },
            vector,
        }
    }
}

fn hamming(a: &[u8], b: &[u8]) -> u32 {
    let n = a.len().max(b.len());
    (0..n)
        .map(|i| (a.get(i).copied().unwrap_or(0) ^ b.get(i).copied().unwrap_or(0)).count_ones())
        .sum()
}

/// `nearest`'s ranking over rows already filtered by kind: the Hamming prefilter above [`PREFILTER_ABOVE`] rows,
/// then the cosine, best first (ties by doc), at most `limit`.
pub fn rank(rows: Vec<&Stored>, vector: &[i8], limit: usize) -> Vec<(String, f64)> {
    if rows.is_empty() {
        return Vec::new();
    }
    let q = int_norm(vector);
    let qnorm = if q == 0.0 { 1.0 } else { q };
    let mut rows = rows;
    if rows.len() > PREFILTER_ABOVE {
        let qbits = sign_bits(vector);
        rows.sort_by_key(|r| hamming(&r.bits, &qbits));
        rows.truncate(PREFILTER.max(limit));
    }
    let mut scored: Vec<(String, f64)> = rows
        .iter()
        .map(|r| {
            (
                r.doc.clone(),
                dot(vector, &r.vector) as f64 / (qnorm * r.norm),
            )
        })
        .collect();
    scored.sort_by(|a, b| {
        b.1.partial_cmp(&a.1)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.0.cmp(&b.0))
    });
    scored.truncate(limit);
    scored
}
