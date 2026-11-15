//! The store as memory uses it: Python's `db.conn.execute(...)` over `ferb-store`'s connection, with one place every
//! write statement passes (`exec`), so a test can fail any one of them as a crash would (`set_fault`), and rows read
//! by column name as `sqlite3.Row` reads them.

use std::cell::RefCell;
use std::rc::Rc;

use rusqlite::types::Value as SqlV;
use serde_json::Value;

use ferb_store::Db;

use crate::{Error, Result};

pub use rusqlite::types::Value as Sql;

/// Something that binds as one SQL parameter.
pub trait Param {
    fn sql(&self) -> SqlV;
}

impl Param for SqlV {
    fn sql(&self) -> SqlV {
        self.clone()
    }
}
impl Param for &str {
    fn sql(&self) -> SqlV {
        SqlV::Text((*self).to_owned())
    }
}
impl Param for String {
    fn sql(&self) -> SqlV {
        SqlV::Text(self.clone())
    }
}
impl Param for &String {
    fn sql(&self) -> SqlV {
        SqlV::Text((*self).clone())
    }
}
impl Param for i64 {
    fn sql(&self) -> SqlV {
        SqlV::Integer(*self)
    }
}
impl Param for usize {
    fn sql(&self) -> SqlV {
        SqlV::Integer(*self as i64)
    }
}
impl Param for f64 {
    fn sql(&self) -> SqlV {
        SqlV::Real(*self)
    }
}
impl Param for bool {
    fn sql(&self) -> SqlV {
        SqlV::Integer(i64::from(*self))
    }
}
impl<T: Param> Param for Option<T> {
    fn sql(&self) -> SqlV {
        self.as_ref().map_or(SqlV::Null, Param::sql)
    }
}

/// `[a, b, c]` as SQL parameters.
#[macro_export]
macro_rules! sql {
    () => { Vec::<$crate::store::Sql>::new() };
    ($($x:expr),+ $(,)?) => { vec![$($crate::store::Param::sql(&$x)),+] };
}

/// One row, read by column name or position.
#[derive(Clone, Debug)]
pub struct Row {
    pub names: Rc<Vec<String>>,
    pub values: Vec<SqlV>,
}

impl Row {
    pub fn get(&self, name: &str) -> &SqlV {
        let at = self
            .names
            .iter()
            .position(|n| n == name)
            .unwrap_or_else(|| panic!("no column {name}"));
        &self.values[at]
    }

    pub fn at(&self, n: usize) -> &SqlV {
        &self.values[n]
    }

    /// `str(r[name])`.
    pub fn s(&self, name: &str) -> String {
        text(self.get(name))
    }

    pub fn s_at(&self, n: usize) -> String {
        text(self.at(n))
    }

    /// A text column that may be NULL.
    pub fn opt(&self, name: &str) -> Option<String> {
        match self.get(name) {
            SqlV::Null => None,
            v => Some(text(v)),
        }
    }

    pub fn opt_at(&self, n: usize) -> Option<String> {
        match self.at(n) {
            SqlV::Null => None,
            v => Some(text(v)),
        }
    }

    pub fn i(&self, name: &str) -> i64 {
        int(self.get(name))
    }

    pub fn i_at(&self, n: usize) -> i64 {
        int(self.at(n))
    }

    pub fn f(&self, name: &str) -> f64 {
        match self.get(name) {
            SqlV::Integer(i) => *i as f64,
            SqlV::Real(r) => *r,
            SqlV::Text(t) => t.trim().parse().unwrap_or(0.0),
            _ => 0.0,
        }
    }

    pub fn is_null(&self, name: &str) -> bool {
        matches!(self.get(name), SqlV::Null)
    }

    /// The row as JSON values, in column order.
    pub fn json(&self) -> Vec<Value> {
        self.values.iter().map(to_json).collect()
    }
}

/// `str(value)` of an SQLite value.
pub fn text(v: &SqlV) -> String {
    match v {
        SqlV::Null => "None".into(),
        SqlV::Integer(i) => i.to_string(),
        SqlV::Real(r) => ferb_py::json::python_float(*r),
        SqlV::Text(t) => t.clone(),
        SqlV::Blob(b) => String::from_utf8_lossy(b).into_owned(),
    }
}

fn int(v: &SqlV) -> i64 {
    match v {
        SqlV::Integer(i) => *i,
        SqlV::Real(r) => *r as i64,
        SqlV::Text(t) => t.trim().parse().unwrap_or(0),
        _ => 0,
    }
}

/// An SQLite value as JSON (what Python's `sqlite3` hands back, as `json.dumps` writes it).
pub fn to_json(v: &SqlV) -> Value {
    match v {
        SqlV::Null => Value::Null,
        SqlV::Integer(i) => Value::from(*i),
        SqlV::Real(r) => serde_json::Number::from_f64(*r).map_or(Value::Null, Value::Number),
        SqlV::Text(t) => Value::String(t.clone()),
        SqlV::Blob(b) => Value::String(String::from_utf8_lossy(b).into_owned()),
    }
}

/// `','.join('?' * n)`.
pub fn marks(n: usize) -> String {
    vec!["?"; n].join(",")
}

type Fault = Box<dyn FnMut(&str) -> bool>;

/// The store connection memory writes through.
pub struct Store {
    pub db: Rc<Db>,
    fault: RefCell<Option<Fault>>,
}

const WRITES: [&str; 4] = ["INSERT", "UPDATE", "DELETE", "REPLACE"];

impl Store {
    pub fn new(db: Rc<Db>) -> Rc<Store> {
        Rc::new(Store {
            db,
            fault: RefCell::new(None),
        })
    }

    /// Told every write statement before it runs; `true` fails it ("injected"), as a crash at that point would.
    pub fn set_fault(&self, fault: Option<Fault>) {
        *self.fault.borrow_mut() = fault;
    }

    fn check(&self, sql: &str) -> Result<()> {
        let head = sql.trim_start().to_ascii_uppercase();
        if WRITES.iter().any(|w| head.starts_with(w)) {
            if let Some(f) = self.fault.borrow_mut().as_mut() {
                if f(sql) {
                    return Err(Error::Sql("OperationalError".into(), "injected".into()));
                }
            }
        }
        Ok(())
    }

    pub fn conn(&self) -> &rusqlite::Connection {
        self.db.conn()
    }

    /// `execute` of a statement that returns no rows; the number of rows it changed.
    pub fn exec(&self, sql: &str, params: Vec<SqlV>) -> Result<usize> {
        self.check(sql)?;
        let mut st = self.conn().prepare_cached(sql)?;
        Ok(st.execute(rusqlite::params_from_iter(params))?)
    }

    /// `executemany`.
    pub fn exec_many(&self, sql: &str, rows: Vec<Vec<SqlV>>) -> Result<usize> {
        self.check(sql)?;
        let mut st = self.conn().prepare_cached(sql)?;
        let mut n = 0;
        for params in rows {
            n += st.execute(rusqlite::params_from_iter(params))?;
        }
        Ok(n)
    }

    /// A statement without parameters that isn't counted as a write (`BEGIN`, `COMMIT`, `SAVEPOINT`, DDL).
    pub fn plain(&self, sql: &str) -> Result<()> {
        self.check(sql)?;
        self.conn().execute_batch(sql)?;
        Ok(())
    }

    pub fn query(&self, sql: &str, params: Vec<SqlV>) -> Result<Vec<Row>> {
        let mut st = self.conn().prepare_cached(sql)?;
        let names: Rc<Vec<String>> = Rc::new(st.column_names().map(|s| s.to_string()).collect());
        let n = names.len();
        let mut rows = st.query(rusqlite::params_from_iter(params))?;
        let mut out = Vec::new();
        while let Some(r) = rows.next()? {
            let mut values = Vec::with_capacity(n);
            for i in 0..n {
                values.push(r.get::<_, SqlV>(i)?);
            }
            out.push(Row {
                names: names.clone(),
                values,
            });
        }
        Ok(out)
    }

    pub fn one(&self, sql: &str, params: Vec<SqlV>) -> Result<Option<Row>> {
        Ok(self.query(sql, params)?.into_iter().next())
    }

    /// The first column of the first row as an int (`COUNT(*)` and the like).
    pub fn count(&self, sql: &str, params: Vec<SqlV>) -> Result<i64> {
        Ok(self.one(sql, params)?.map_or(0, |r| r.i_at(0)))
    }

    pub fn exists(&self, sql: &str, params: Vec<SqlV>) -> Result<bool> {
        Ok(self.one(sql, params)?.is_some())
    }

    pub fn in_transaction(&self) -> bool {
        !self.conn().is_autocommit()
    }

    /// `db.truncate_wal(after)`.
    pub fn truncate_wal(&self, after: &str) -> Result<bool> {
        Ok(self.db.truncate_wal(after)?)
    }
}
