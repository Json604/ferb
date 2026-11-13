//! The store's one-time clean-up (`setup --cleanup`, `ferb/store/cleanup.py`): ferb-store's plan and apply, with
//! their episode-card side (`cleanup::CardSources`: `Learning.card_source_plan` and `prune_card_sources`) over
//! ferb-learning, as `cleanup._learning(db)` builds it: the system clock, nothing published, text kept as it is. The
//! preview reads on connections that never write; the apply runs on the stopped daemon's store, the card pruning on
//! the very connection the rest of the run uses.

use std::cell::RefCell;
use std::path::Path;
use std::rc::Rc;

use ferb_learning::learning::Learning;
use ferb_learning::store::Store;
use ferb_store::cleanup::{CardSources, JournalSink, Run};
use ferb_store::Db;
use rusqlite::Connection;
use serde_json::Value;

struct SystemClock;

impl ferb_learning::Clock for SystemClock {
    fn now(&self) -> ferb_learning::DateTime {
        ferb_learning::DateTime::from_isoformat(&ferb_clock::py::iso(
            &chrono::Local::now().fixed_offset(),
        ))
        .expect("an ISO time")
    }
}

struct LocalZone;

impl ferb_memory::times::LocalZone for LocalZone {
    fn offset_at(&self, unix_seconds: i64) -> i32 {
        chrono::DateTime::from_timestamp(unix_seconds, 0)
            .unwrap_or_default()
            .with_timezone(&chrono::Local)
            .offset()
            .local_minus_utc()
    }
}

struct Seams;

impl ferb_learning::learning::Seams for Seams {
    fn episode(&self, store: &Store, task_id: &str) -> ferb_learning::Result<Option<Value>> {
        ferb_orchestrator::episodes::get(&store.db, task_id)
            .map_err(|e| ferb_learning::Error::Sql("OperationalError".into(), e.to_string()))
    }
    fn trail_sources(&self, source: &str) -> Vec<(String, String)> {
        ferb_sense::tools::trail_sources(&[source])
    }
}

/// `cleanup._learning(db)`: the episode cards' side, over `db`.
pub struct Cards(Learning);

impl Cards {
    pub fn over(db: Rc<Db>) -> Cards {
        Cards(Learning::new(
            Store::new(db),
            Rc::new(SystemClock),
            Rc::new(LocalZone),
            Box::new(|_: &str, _: Value| {}),
            Box::new(|t: &str| t.to_string()),
            Box::new(Seams),
        ))
    }
}

fn store_err(e: ferb_learning::Error) -> ferb_store::Error {
    match e {
        ferb_learning::Error::Sql(_, m) => ferb_store::Error::Value(m),
        other => ferb_store::Error::Value(format!("{}: {other}", other.class())),
    }
}

impl CardSources for Cards {
    fn card_source_plan(
        &self,
        _conn: &Connection,
        removing: &[String],
    ) -> ferb_store::Result<Value> {
        self.0
            .card_source_plan(&mut || false, removing)
            .map_err(store_err)
    }
    fn prune_card_sources(
        &self,
        _conn: &Connection,
        stop: &dyn Fn() -> bool,
        plan: &Value,
    ) -> ferb_store::Result<Value> {
        self.0
            .prune_card_sources(
                &mut || stop(),
                &mut |_, _| {},
                &mut || Ok(()),
                Some(plan.clone()),
            )
            .map_err(store_err)
    }
}

/// `Journal(db, SystemClock(), RandomIds(), Redactor())`, as the clean-up writes it.
struct JournalOver(ferb_learning::journal::Journal);

impl JournalSink for JournalOver {
    fn append(&mut self, kind: &str, data: &Value) -> ferb_store::Result<()> {
        self.0
            .append(kind, Some(data.clone()), None)
            .map(|_| ())
            .map_err(store_err)
    }
}

struct RandomIds;

impl ferb_learning::Ids for RandomIds {
    fn new_id(&self) -> String {
        ferb_runtimes::codex::token_hex(16)
    }
}

/// `cleanup.preview(path)`: the plan on a read-only connection with the sizes the app shows; the card plan read on a
/// connection of its own that can't write.
pub fn preview(path: &Path) -> Value {
    let cards = match Db::open(path).and_then(|db| {
        db.conn().execute_batch("PRAGMA query_only=ON")?;
        Ok(db)
    }) {
        Ok(db) => Cards::over(Rc::new(db)),
        Err(e) => return serde_json::json!({"error": format!("{e}")}),
    };
    match ferb_store::cleanup::preview(path, &cards, ferb_store::disk_free) {
        Ok(v) => v,
        Err(e) => serde_json::json!({"error": format!("{e}")}),
    }
}

/// `_run_cleanup`'s store part: `cleanup.apply(db, journal, ok, stop=, say=)` on the stopped daemon's store
/// (migrated first). Errors are (class, words).
pub fn apply(
    home: &Path,
    ok: &str,
    stop: &dyn Fn() -> bool,
    say: &mut dyn FnMut(&str),
) -> Result<Value, (String, String)> {
    let fail = |e: ferb_store::Error| {
        let class = match &e {
            ferb_store::Error::Sqlite(_) => "OperationalError",
            ferb_store::Error::Io(_) => "OSError",
            ferb_store::Error::Refused(_) => "Refused",
            e if e.is_migration_error() => "MigrationError",
            _ => "ValueError",
        };
        (class.to_string(), e.to_string())
    };
    let db = Rc::new(Db::open(home.join("ferb.db")).map_err(fail)?);
    db.migrate().map_err(fail)?;
    let store = Store::new(db.clone());
    let redactor = ferb_sanitize::redact::Redactor::new(&[], &[]);
    let mut journal = JournalOver(ferb_learning::journal::Journal::new(
        store,
        Rc::new(SystemClock),
        Rc::new(RandomIds),
        Box::new(move |v: &Value| redactor.value(v)),
    ));
    let cards = Cards::over(db.clone());
    let said = RefCell::new(say);
    ferb_store::cleanup::apply(
        &db,
        &mut journal,
        &cards,
        ok,
        Run {
            stop,
            say: &|line: &str| (said.borrow_mut())(line),
            free: &ferb_store::disk_free,
        },
    )
    .map_err(fail)
}
