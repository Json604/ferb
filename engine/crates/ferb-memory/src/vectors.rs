//! The stored vectors (`embeddings.py:274-330`): `recall_vectors` rows (int8 bytes and sign bits), and `nearest`
//! over an in-memory copy kept per database and model, reloaded when `recall_state.vectors` (bumped by triggers)
//! changed.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use rusqlite::{types::Value as Sql, Connection, OptionalExtension};

use crate::embeddings::{rank, sign_bits, Stored};
use crate::pyval::Crash;
use crate::Error;

/// `store`: one vector and its sign bits (replacing the doc's earlier one for this model).
pub fn store(
    conn: &Connection,
    doc: &str,
    kind: &str,
    at: &str,
    model: &str,
    digest: &str,
    vector: &[i8],
) -> Result<(), Error> {
    let bytes: Vec<u8> = vector.iter().map(|&x| x as u8).collect();
    conn.execute(
        "INSERT OR REPLACE INTO recall_vectors (doc, model, kind, at, text_hash, vector, bits) VALUES (?,?,?,?,?,?,?)",
        rusqlite::params![doc, model, kind, at, digest, bytes, sign_bits(vector)],
    )?;
    Ok(())
}

struct Loaded {
    generation: i64,
    rows: Vec<Stored>,
}

type Cache = Mutex<HashMap<(String, String), Arc<Loaded>>>;

fn cache() -> &'static Cache {
    static CACHE: OnceLock<Cache> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn generation(conn: &Connection) -> Result<i64, Error> {
    let v = conn
        .query_row(
            "SELECT value FROM recall_state WHERE name = 'vectors'",
            [],
            |r| r.get::<_, Sql>(0),
        )
        .optional()?;
    Ok(match v {
        None => 0,
        Some(Sql::Integer(i)) => i,
        Some(Sql::Text(t)) => crate::pyval::int(&serde_json::Value::String(t))?,
        Some(_) => return Err(Crash("TypeError").into()),
    })
}

fn loaded(conn: &Connection, model: &str) -> Result<Arc<Loaded>, Error> {
    let key = (
        conn.path()
            .map_or_else(|| format!("{:p}", conn as *const _), str::to_owned),
        model.to_owned(),
    );
    let generation = generation(conn)?;
    if let Some(held) = cache().lock().map_err(|_| Crash("RuntimeError"))?.get(&key) {
        if held.generation == generation {
            return Ok(held.clone());
        }
    }
    // the table's own order, as Python's unordered SELECT returns it on a table scan
    let mut stmt = conn.prepare(
        "SELECT doc, kind, vector, bits FROM recall_vectors WHERE model = ? ORDER BY rowid",
    )?;
    let rows = stmt
        .query_map([model], |r| {
            Ok(Stored::new(
                r.get(0)?,
                r.get(1)?,
                r.get::<_, Vec<u8>>(2)?
                    .into_iter()
                    .map(|b| b as i8)
                    .collect(),
                r.get(3)?,
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    let held = Arc::new(Loaded { generation, rows });
    cache()
        .lock()
        .map_err(|_| Crash("RuntimeError"))?
        .insert(key, held.clone());
    Ok(held)
}

/// `nearest`: the stored docs of `kinds` nearest `vector` by cosine, best first, at most `limit`.
pub fn nearest(
    conn: &Connection,
    model: &str,
    vector: &[i8],
    kinds: &[&str],
    limit: usize,
) -> Result<Vec<(String, f64)>, Error> {
    let held = loaded(conn, model)?;
    let rows: Vec<&Stored> = held
        .rows
        .iter()
        .filter(|r| kinds.contains(&r.kind.as_str()))
        .collect();
    Ok(rank(rows, vector, limit))
}
