//! FERB's evidence store (ferb/act/evidence/store.py, V1, D20): captured state the final checker can re-inspect,
//! kept 7 days, in ~/.ferb/evidence with a row each in the `evidence` table (migration 0020). Never a secret value:
//! data goes through the redactor, an image's known secret fields are painted before it is written, and an image
//! that can't be painted is dropped (the row says so). Never a file without its row: the file is written new
//! (O_EXCL, 0600) and removed when the row can't be inserted.

use std::io::Write;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Map, Value};

use crate::mask::ImgBox;
use crate::ocr::Paint;

pub const UNMASKABLE: &str = "its secret fields couldn't be masked";

fn ext(mime: &str) -> &'static str {
    match mime {
        "image/jpeg" => "jpg",
        "video/mp4" => "mp4",
        _ => "bin",
    }
}

pub struct EvidenceStore {
    pub root: PathBuf,
    /// (captured_at in isoformat, the local day "YYYY-MM-DD" it falls on).
    pub now: Arc<dyn Fn() -> (String, String) + Send + Sync>,
    pub ids: Arc<dyn Fn() -> String + Send + Sync>,
    /// The Redactor over a JSON value (redact.py `Redactor.__call__`).
    pub redact: Arc<dyn Fn(&Value) -> Value + Send + Sync>,
    pub paint: Arc<dyn Paint>,
}

impl EvidenceStore {
    pub fn new(
        root: &Path,
        now: Arc<dyn Fn() -> (String, String) + Send + Sync>,
        ids: Arc<dyn Fn() -> String + Send + Sync>,
        redact: Arc<dyn Fn(&Value) -> Value + Send + Sync>,
        paint: Arc<dyn Paint>,
    ) -> std::io::Result<Self> {
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(root)?;
        Ok(EvidenceStore {
            root: root.to_path_buf(),
            now,
            ids,
            redact,
            paint,
        })
    }

    /// store.py:26: one capture: the image (masked first when `rects` name secret fields) and its row.
    #[allow(clippy::too_many_arguments)]
    pub fn put(
        &self,
        db: &Connection,
        task_id: &str,
        span_id: Option<&str>,
        kind: &str,
        image: Option<&[u8]>,
        data: Option<Map<String, Value>>,
        rects: &[ImgBox],
        mime: &str,
        origin: &str,
    ) -> Result<String, String> {
        let mut data = data;
        let mut image: Option<Vec<u8>> = image.map(<[u8]>::to_vec);
        if let Some(img) = &image {
            if !rects.is_empty() {
                match self.paint.paint(img, rects) {
                    Ok(painted) => image = Some(painted),
                    Err(_) => {
                        image = None;
                        let mut d = data.unwrap_or_default();
                        d.insert("image_withheld".into(), json!(UNMASKABLE));
                        data = Some(d);
                    }
                }
            }
        }
        let eid = (self.ids)();
        let (at, day) = (self.now)();
        let payload = data.map(|d| {
            ferb_py::json::dumps_with(
                &(self.redact)(&Value::Object(d)),
                ferb_py::json::Style {
                    ascii: false,
                    sort_keys: false,
                    indent: None,
                },
            )
        });
        let rel = image.as_ref().map(|_| format!("{day}/{eid}.{}", ext(mime)));
        if let (Some(rel), Some(img)) = (&rel, &image) {
            write_new(&self.root.join(rel), img).map_err(|e| e.to_string())?;
        }
        let inserted = db.execute(
            "INSERT INTO evidence (id, task_id, span_id, kind, mime, path, data_json, origin, captured_at) VALUES (?,?,?,?,?,?,?,?,?)",
            params![eid, task_id, span_id, kind, rel.as_ref().map(|_| mime), rel, payload, origin, at],
        );
        if let Err(e) = inserted {
            if let Some(rel) = &rel {
                let _ = std::fs::remove_file(self.root.join(rel));
            }
            return Err(e.to_string());
        }
        Ok(eid)
    }

    pub fn list(&self, db: &Connection, task_id: &str) -> rusqlite::Result<Vec<Value>> {
        let mut st = db.prepare("SELECT id, task_id, span_id, kind, mime, origin, captured_at, path, data_json FROM evidence WHERE task_id = ? ORDER BY captured_at, rowid")?;
        let rows = st.query_map([task_id], |r| Ok(meta(r, true)))?;
        rows.collect()
    }

    /// store.py:104: a row and its file's bytes (only from inside the store).
    pub fn read(
        &self,
        db: &Connection,
        evidence_id: &str,
    ) -> rusqlite::Result<Option<(Value, Option<Vec<u8>>)>> {
        let row = db
            .query_row(
                "SELECT id, task_id, span_id, kind, mime, origin, captured_at, path, data_json FROM evidence WHERE id = ?",
                [evidence_id],
                |r| Ok((meta(r, false), r.get::<_, Option<String>>(7)?)),
            )
            .optional()?;
        Ok(row.map(|(m, rel)| {
            let bytes = self
                .inside(rel.as_deref())
                .and_then(|p| std::fs::read(p).ok());
            (m, bytes)
        }))
    }

    /// store.py:111: captures older than the cutoff go, file first; a file that can't be deleted keeps its row.
    pub fn purge_before(&self, db: &Connection, cutoff_iso: &str) -> rusqlite::Result<usize> {
        let rows: Vec<(String, Option<String>)> = {
            let mut st = db.prepare("SELECT id, path FROM evidence WHERE captured_at < ?")?;
            let it = st.query_map([cutoff_iso], |r| Ok((r.get(0)?, r.get(1)?)))?;
            it.collect::<rusqlite::Result<_>>()?
        };
        let mut gone = vec![];
        for (id, rel) in rows {
            if let Some(path) = self.inside(rel.as_deref()) {
                match std::fs::remove_file(&path) {
                    Ok(()) => {}
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                    Err(_) => continue,
                }
            }
            gone.push(id);
        }
        for chunk in gone.chunks(500) {
            let marks = vec!["?"; chunk.len()].join(",");
            db.execute(
                &format!("DELETE FROM evidence WHERE id IN ({marks})"),
                rusqlite::params_from_iter(chunk.iter()),
            )?;
        }
        if let Ok(days) = std::fs::read_dir(&self.root) {
            for d in days.flatten() {
                let p = d.path();
                if p.is_dir()
                    && std::fs::read_dir(&p)
                        .map(|mut x| x.next().is_none())
                        .unwrap_or(false)
                {
                    let _ = std::fs::remove_dir(&p);
                }
            }
        }
        Ok(gone.len())
    }

    /// A row's file, only when it resolves inside the store (symlinks followed).
    pub fn inside(&self, rel: Option<&str>) -> Option<PathBuf> {
        let rel = rel.filter(|r| !r.is_empty())?;
        let path = std::fs::canonicalize(self.root.join(rel)).ok()?;
        let root = std::fs::canonicalize(&self.root).ok()?;
        path.starts_with(&root).then_some(path)
    }
}

fn meta(r: &rusqlite::Row<'_>, with_path: bool) -> Value {
    let path: Option<String> = r.get(7).unwrap_or(None);
    let data: Option<String> = r.get(8).unwrap_or(None);
    let mut m = json!({"id": r.get::<_, String>(0).unwrap_or_default(), "task_id": r.get::<_, String>(1).unwrap_or_default(),
        "span_id": r.get::<_, Option<String>>(2).unwrap_or(None), "kind": r.get::<_, String>(3).unwrap_or_default(),
        "mime": r.get::<_, Option<String>>(4).unwrap_or(None), "origin": r.get::<_, String>(5).unwrap_or_default(),
        "captured_at": r.get::<_, String>(6).unwrap_or_default(), "has_image": path.is_some(),
        "data": data.as_deref().filter(|d| !d.is_empty()).and_then(|d| ferb_py::json::loads(d).ok())});
    if with_path {
        m["path"] = json!(path);
    }
    m
}

/// store.py:150: the capture's file, created new, owner-only, in an owner-only day folder.
fn write_new(path: &Path, image: &[u8]) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        match std::fs::DirBuilder::new().mode(0o700).create(parent) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(e),
        }
    }
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)?;
    f.write_all(image)
}
