//! `swap` (`ferb/store/db.py:236-259`): one row's columns rewritten from what they hold now,
//! compare-and-set, so a rewrite never writes from a stale read (fix wave 6a).

use rusqlite::{params_from_iter, types::Value as Sql, Connection, OptionalExtension};

use crate::Result;

/// The values of the columns named, in order.
pub type Row = Vec<Sql>;

/// One row's `columns` rewritten through `change` from what they hold now. The write applies
/// only while the row still holds what was read; when another connection wrote the row in
/// between, it is read and changed again. `held`: the values as the caller already read them.
/// `change` giving them back unchanged writes nothing. Returns the values written, or `None`
/// (also when the row doesn't exist). Table and column names are FERB's own constants.
pub fn swap(
    conn: &Connection,
    table: &str,
    key: &str,
    ident: &Sql,
    columns: &[&str],
    mut change: impl FnMut(&Row) -> Result<Row>,
    mut held: Option<Row>,
) -> Result<Option<Row>> {
    let read = format!("SELECT {} FROM {table} WHERE {key} = ?", columns.join(", "));
    let sets: Vec<String> = columns.iter().map(|c| format!("{c} = ?")).collect();
    let same: Vec<String> = columns.iter().map(|c| format!("{c} IS ?")).collect();
    let write = format!(
        "UPDATE {table} SET {} WHERE {key} = ? AND {}",
        sets.join(", "),
        same.join(" AND ")
    );
    loop {
        let before = match held.take() {
            Some(row) => row,
            None => {
                let row = conn
                    .query_row(&read, [ident], |r| {
                        (0..columns.len()).map(|i| r.get::<_, Sql>(i)).collect()
                    })
                    .optional()?;
                match row {
                    Some(row) => row,
                    None => return Ok(None),
                }
            }
        };
        let new = change(&before)?;
        if new == before {
            return Ok(None);
        }
        let params = new
            .iter()
            .chain(std::iter::once(ident))
            .chain(before.iter());
        if conn.execute(&write, params_from_iter(params))? > 0 {
            return Ok(Some(new));
        }
    }
}
