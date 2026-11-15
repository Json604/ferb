//! FERB's own pinned tokenizer copy (ferb/cli/tokenizer.py): `ferb setup` puts it under FERB's home, checked against
//! the pinned SHA-256; a verified copy already on this Mac is copied, else the public file is downloaded once.

use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

/// ferb/core/context.py TOKENIZER_URL and TOKENIZER_SHA256.
pub const TOKENIZER_URL: &str =
    "https://openaipublic.blob.core.windows.net/encodings/o200k_base.tiktoken";
pub const TOKENIZER_SHA256: &str =
    "446a9538cb6c348e3516120d7c08b09f57c36495e2acfffe59a5bf8b0cfb1a2d";

/// A download: the bytes, or (exception type, message).
pub type Fetch<'a> = dyn FnMut(&str) -> Result<Vec<u8>, (String, String)> + 'a;

/// context.py `tokenizer_path`.
pub fn tokenizer_path(home: &Path) -> PathBuf {
    home.join("cache/tokenizer/o200k_base.tiktoken")
}

/// tokenizer.py:22: put the pinned copy in place and say what happened in one line; never fails. `verified` is a
/// verified copy already on this Mac (context.verified_tokenizer), `fetch` downloads (Err: exception type, message).
pub fn ensure(
    home: &Path,
    pin: &str,
    verified: &mut dyn FnMut() -> Option<Vec<u8>>,
    fetch: &mut Fetch<'_>,
) -> String {
    let target = tokenizer_path(home);
    let attempt = (|| -> Result<String, (String, String)> {
        if target.is_file() {
            if let Ok(data) = std::fs::read(&target) {
                if hex::encode(Sha256::digest(&data)) == pin {
                    return Ok("Tokenizer: FERB's pinned copy is in place.".into());
                }
            }
        }
        let (data, how) = match verified() {
            Some(d) => (d, "copied from this Mac's tiktoken cache"),
            None => (fetch(TOKENIZER_URL)?, "downloaded"),
        };
        if hex::encode(Sha256::digest(&data)) != pin {
            return Ok("Tokenizer: the downloaded file doesn't match FERB's pinned hash; nothing was kept.".into());
        }
        let io = |e: std::io::Error, p: &Path| {
            let kind = match e.kind() {
                std::io::ErrorKind::NotFound => "FileNotFoundError",
                std::io::ErrorKind::PermissionDenied => "PermissionError",
                _ => "OSError",
            };
            (kind.to_string(), crate::py::os_error_text(&e, Some(p)))
        };
        let parent = target.parent().expect("the copy has a folder");
        std::fs::create_dir_all(parent).map_err(|e| io(e, parent))?;
        let part = target.with_extension("part");
        std::fs::write(&part, &data).map_err(|e| io(e, &part))?;
        std::fs::rename(&part, &target).map_err(|e| io(e, &part))?;
        Ok(format!("Tokenizer: FERB's pinned copy {how} and verified."))
    })();
    match attempt {
        Ok(line) => line,
        Err((kind, message)) => format!(
            "Tokenizer: not available ({kind}: {message}); token counts use a byte ceiling until FERB's next update can fetch it."
        ),
    }
}
