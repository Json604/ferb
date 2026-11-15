//! FERB's resource log (`ferb/daemon/resources.py`, L30, owner 2026-10-23): every SAMPLE_SECONDS the daemon writes one
//! row of numbers, kept `[retention] resource_days` (30). Never content: no arguments, paths, text or titles; a
//! process is named only by its executable's basename, through the daemon's redactor (rule 14).
//!
//! A row: the daemon's CPU (getrusage) and physical footprint (libproc); its descendants' (walked from its pid; a
//! process FERB opens through Launch Services belongs to launchd and isn't one); FERB.app's processes outside that
//! tree; the database's size; and the window's work, each count a seq or rowid range from the last sample's maximum,
//! never a table scan. The first window starts at the log's creation, during the daemon's start-up.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;

use ferb_store::Db;
use serde_json::{json, Map, Value};

/// owner 2026-10-23: "every 5 minutes".
pub const SAMPLE_SECONDS: u64 = 300;
/// `[retention] resource_days`: the owner's "30 days".
pub const DEFAULT_DAYS: i64 = 30;
/// The window's busiest descendants named in a row.
pub const TOP: usize = 3;
/// FERB.app: a process whose executable lies inside a bundle named so.
pub const APP_BUNDLE: &str = "FERB.app";

/// Where each window's counts start: the highest seq or rowid at the last sample. NOT INDEXED keeps the trail's
/// count a range of its seq whatever statistics say.
pub const CURSORS: [(&str, &str); 3] = [
    ("trail", "SELECT COALESCE(MAX(seq), 0) FROM trail"),
    ("decisions", "SELECT COALESCE(MAX(seq), 0) FROM decisions"),
    ("spans", "SELECT COALESCE(MAX(rowid), 0) FROM spans"),
];
pub const EVENTS: &str =
    "SELECT source, COUNT(*) FROM trail NOT INDEXED WHERE seq > ? AND seq <= ? GROUP BY source";
pub const JEV: &str =
    "SELECT error IS 'not sampled', COUNT(*) FROM decisions WHERE seq > ? AND seq <= ? GROUP BY 1";
pub const MODELS: &str = "SELECT kind, COUNT(*) FROM spans WHERE rowid > ? AND rowid <= ? \
     AND kind IN ('model.turn', 'worker', 'model.learning', 'tool.call') GROUP BY kind";
pub const RUNNING: &str = "SELECT COUNT(*) FROM tasks WHERE status = 'running'";
pub const COLUMNS: [&str; 13] = [
    "at",
    "day",
    "window_s",
    "cpu_s",
    "footprint",
    "peak_footprint",
    "child_count",
    "child_footprint",
    "child_cpu_s",
    "app_cpu_s",
    "app_footprint",
    "db_bytes",
    "work_json",
];

/// One process as the kernel reports it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Process {
    pub pid: i32,
    /// When it started (mach absolute time): with the pid, which process this is.
    pub start: u64,
    /// User + system CPU since it started.
    pub cpu_s: f64,
    /// Physical footprint in bytes (Activity Monitor's Memory column).
    pub footprint: i64,
    /// Its lifetime maximum.
    pub peak_footprint: i64,
    /// Its children that ended and that it waited for, theirs included.
    pub children_cpu_s: f64,
}

/// What a sample reads from the machine; the daemon uses [`MacReadings`], tests a fake.
pub trait Readings: Send {
    /// This process's user + system CPU seconds.
    fn own_cpu(&self) -> Result<f64, String>;
    /// Its children that ended and were waited for (and theirs).
    fn children_cpu(&self) -> f64;
    /// None: gone or not readable.
    fn process(&self, pid: i32) -> Option<Process>;
    fn children(&self, pid: i32) -> Vec<i32>;
    fn pids(&self) -> Vec<i32>;
    /// Its executable.
    fn path(&self, pid: i32) -> Option<String>;
}

/// Mach absolute time units in seconds, by the timebase.
pub fn mach_seconds(units: u64, numer: u32, denom: u32) -> f64 {
    units as f64 * f64::from(numer) / f64::from(denom) / 1e9
}

/// Whether an executable lies inside a FERB.app bundle (the packaged app, its helpers).
pub fn in_app_bundle(path: &str) -> bool {
    let parts: Vec<&str> = Path::new(path)
        .components()
        .map(|c| c.as_os_str().to_str().unwrap_or(""))
        .collect();
    parts.len() > 1
        && parts[..parts.len() - 1]
            .iter()
            .any(|p| p.ends_with(APP_BUNDLE))
}

/// (pid, start): one process, never a reused pid.
pub type Key = (i32, u64);
/// What is still owed against a parent's children counter (None: the daemon's `children_cpu()`).
type Owed = HashMap<Option<Key>, f64>;

#[derive(Clone, Copy, Debug)]
struct Seen {
    cpu_s: f64,
    children_cpu_s: f64,
    parent: Option<Key>,
    depth: usize,
}

/// What one sample leaves for the next.
struct Mark {
    mono: f64,
    own_cpu: f64,
    reaped_cpu: f64,
    cursors: BTreeMap<String, i64>,
    perception: (i64, BTreeMap<String, i64>),
    tracked: HashMap<Key, Seen>,
    owed: Owed,
    app: HashMap<Key, f64>,
    preview: Map<String, Value>,
}

type Tree = HashMap<Key, (Process, Option<Key>, usize)>;
/// A window's cursors and its counts.
type Window = (BTreeMap<String, i64>, Map<String, Value>);

/// The window's clock: now as ISO text and the local day, and a monotonic reading.
pub trait SampleClock: Send {
    fn now_iso(&self) -> String;
    fn local_day(&self) -> String;
    fn monotonic(&self) -> f64;
}

/// Python's `round(x, n)` for the numbers a row holds.
fn round(x: f64, n: usize) -> f64 {
    ferb_memory::pymath::round(x, n)
}

/// Creating one takes the baseline; `sample` writes the window since the last sample (or the creation).
pub struct ResourceLog {
    pub db_path: std::path::PathBuf,
    clock: Box<dyn SampleClock>,
    readings: Box<dyn Readings>,
    redact: Box<dyn Fn(&str) -> String + Send>,
    perception: Box<dyn Fn() -> (i64, BTreeMap<String, i64>) + Send>,
    preview: Box<dyn Fn() -> Map<String, Value> + Send>,
    pid: i32,
    mark: Mark,
}

fn sql_err(e: impl std::fmt::Display) -> String {
    format!("OperationalError: {e}")
}

fn cursors(db: &Db) -> Result<BTreeMap<String, i64>, String> {
    CURSORS
        .iter()
        .map(|(name, sql)| {
            db.conn()
                .query_row(sql, [], |r| r.get::<_, i64>(0))
                .map(|n| (name.to_string(), n))
                .map_err(sql_err)
        })
        .collect()
}

impl ResourceLog {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        db: &Db,
        clock: Box<dyn SampleClock>,
        readings: Box<dyn Readings>,
        redact: Box<dyn Fn(&str) -> String + Send>,
        perception: Box<dyn Fn() -> (i64, BTreeMap<String, i64>) + Send>,
        preview: Box<dyn Fn() -> Map<String, Value> + Send>,
        pid: i32,
    ) -> Result<ResourceLog, String> {
        let mut log = ResourceLog {
            db_path: db.path().to_path_buf(),
            clock,
            readings,
            redact,
            perception,
            preview,
            pid,
            mark: Mark {
                mono: 0.0,
                own_cpu: 0.0,
                reaped_cpu: 0.0,
                cursors: BTreeMap::new(),
                perception: (0, BTreeMap::new()),
                tracked: HashMap::new(),
                owed: HashMap::new(),
                app: HashMap::new(),
                preview: Map::new(),
            },
        };
        let (tree, mut walked) = log.walk();
        walked.insert(log.pid);
        let app = log.app(&walked);
        log.mark = Mark {
            mono: log.clock.monotonic(),
            own_cpu: log.readings.own_cpu()?,
            reaped_cpu: log.readings.children_cpu(),
            cursors: cursors(db)?,
            perception: (log.perception)(),
            tracked: tree
                .iter()
                .map(|(k, (p, parent, depth))| {
                    (
                        *k,
                        Seen {
                            cpu_s: p.cpu_s,
                            children_cpu_s: p.children_cpu_s,
                            parent: *parent,
                            depth: *depth,
                        },
                    )
                })
                .collect(),
            owed: HashMap::new(),
            app: app.iter().map(|(k, p)| (*k, p.cpu_s)).collect(),
            preview: (log.preview)(),
        };
        Ok(log)
    }

    /// One window's row, written and returned. Nothing is kept from a sample that fails: the next covers both.
    pub fn sample(&mut self, db: &Db) -> Result<Map<String, Value>, String> {
        let (now, day, mono) = (
            self.clock.now_iso(),
            self.clock.local_day(),
            self.clock.monotonic(),
        );
        let own = self.readings.own_cpu()?;
        let reaped = self.readings.children_cpu(); // before the walk: see `descendants`
        let me = self.readings.process(self.pid);
        let (tree, mut walked) = self.walk();
        let (live, children_cpu, tracked, owed) = self.descendants(&tree, reaped);
        walked.insert(self.pid);
        let app = self.app(&walked);
        let (cursors, mut work) = self.work(db)?;
        let mut busiest: Vec<(f64, Key)> = live
            .iter()
            .filter(|(_, c)| **c > 0.0)
            .map(|(k, c)| (*c, *k))
            .collect();
        // Python's sorted(reverse=True) on (cpu, (pid, start)) tuples
        busiest.sort_by(|a, b| b.partial_cmp(a).unwrap_or(std::cmp::Ordering::Equal));
        busiest.truncate(TOP);
        let top: Vec<Value> = busiest
            .iter()
            .map(|(cpu, key)| {
                let path = self.readings.path(key.0).unwrap_or_default();
                let base = Path::new(&path)
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                let name = (self.redact)(&base);
                json!([
                    if name.is_empty() {
                        "unknown".to_string()
                    } else {
                        name
                    },
                    round(*cpu, 3)
                ])
            })
            .collect();
        work.insert("top".into(), Value::Array(top));
        let counts = (self.perception)();
        let previews = (self.preview)();
        let mut preview = Map::new();
        for (key, value) in &previews {
            let before = self.mark.preview.get(key);
            let v = if key.starts_with("max_") {
                value.clone()
            } else if value.is_f64() {
                json!(round(
                    value.as_f64().unwrap_or(0.0) - before.and_then(Value::as_f64).unwrap_or(0.0),
                    1
                ))
            } else {
                json!(
                    value.as_i64().unwrap_or(0)
                        - before.and_then(Value::as_f64).unwrap_or(0.0) as i64
                )
            };
            preview.insert(key.clone(), v);
        }
        work.insert("preview".into(), Value::Object(preview));
        let mut batches = Map::new();
        for (reason, n) in &counts.1 {
            let before = self.mark.perception.1.get(reason).copied().unwrap_or(0);
            if *n > before {
                batches.insert(reason.clone(), json!(n - before));
            }
        }
        work.insert(
            "perception".into(),
            json!({"wakes": (counts.0 - self.mark.perception.0).max(0), "batches": batches}),
        );
        let app_cpu = (!app.is_empty()).then(|| {
            round(
                app.iter()
                    .map(|(k, p)| (p.cpu_s - self.counted(k)).max(0.0))
                    .sum(),
                3,
            )
        });
        let app_footprint =
            (!app.is_empty()).then(|| app.values().map(|p| p.footprint).sum::<i64>());
        let mut ordered = Map::new();
        for key in [
            "events",
            "perception",
            "jev",
            "models",
            "tool_calls",
            "tasks_running",
            "top",
            "preview",
        ] {
            ordered.insert(key.into(), work.get(key).cloned().unwrap_or(Value::Null));
        }
        let mut row = Map::new();
        row.insert("at".into(), json!(now));
        row.insert("day".into(), json!(day));
        row.insert("window_s".into(), json!(round(mono - self.mark.mono, 3)));
        row.insert(
            "cpu_s".into(),
            json!(round((own - self.mark.own_cpu).max(0.0), 3)),
        );
        row.insert("footprint".into(), json!(me.map(|m| m.footprint)));
        row.insert("peak_footprint".into(), json!(me.map(|m| m.peak_footprint)));
        row.insert("child_count".into(), json!(tree.len()));
        row.insert(
            "child_footprint".into(),
            json!(tree.values().map(|(p, _, _)| p.footprint).sum::<i64>()),
        );
        row.insert("child_cpu_s".into(), json!(round(children_cpu, 3)));
        row.insert("app_cpu_s".into(), json!(app_cpu));
        row.insert("app_footprint".into(), json!(app_footprint));
        row.insert(
            "db_bytes".into(),
            json!(ferb_store::store_size(&self.db_path)),
        );
        row.insert(
            "work_json".into(),
            json!(ferb_py::json::dumps(&Value::Object(ordered))),
        );
        let marks = vec!["?"; COLUMNS.len()].join(", ");
        let values: Vec<rusqlite::types::Value> = COLUMNS
            .iter()
            .map(|c| match &row[*c] {
                Value::Null => rusqlite::types::Value::Null,
                Value::String(s) => rusqlite::types::Value::Text(s.clone()),
                Value::Number(n) if n.is_i64() || n.is_u64() => {
                    rusqlite::types::Value::Integer(n.as_i64().unwrap_or(0))
                }
                Value::Number(n) => rusqlite::types::Value::Real(n.as_f64().unwrap_or(0.0)),
                other => rusqlite::types::Value::Text(other.to_string()),
            })
            .collect();
        db.conn()
            .execute(
                &format!(
                    "INSERT INTO resource_samples ({}) VALUES ({marks})",
                    COLUMNS.join(", ")
                ),
                rusqlite::params_from_iter(values),
            )
            .map_err(sql_err)?;
        self.mark = Mark {
            mono,
            own_cpu: own,
            reaped_cpu: reaped,
            cursors,
            perception: counts,
            tracked,
            owed,
            app: app.iter().map(|(k, p)| (*k, p.cpu_s)).collect(),
            preview: previews,
        };
        Ok(row)
    }

    /// The daemon's descendants that can be read, each with the nearest readable ancestor it is under (None: the
    /// daemon) and its depth, and every pid walked. One that can't be read is skipped; its children still walked.
    fn walk(&self) -> (Tree, HashSet<i32>) {
        let mut found: Tree = HashMap::new();
        let mut walked: HashSet<i32> = HashSet::new();
        let mut queue: Vec<(i32, Option<Key>, usize)> = self
            .readings
            .children(self.pid)
            .into_iter()
            .map(|p| (p, None, 1))
            .collect();
        while let Some((pid, parent, depth)) = queue.pop() {
            if walked.contains(&pid) || pid == self.pid {
                continue;
            }
            walked.insert(pid);
            let mut under = parent;
            if let Some(proc_) = self.readings.process(pid) {
                under = Some((pid, proc_.start));
                found.insert((pid, proc_.start), (proc_, parent, depth));
            }
            queue.extend(
                self.readings
                    .children(pid)
                    .into_iter()
                    .map(|c| (c, under, depth + 1)),
            );
        }
        (found, walked)
    }

    /// The descendants' CPU in the window (resources.py `_descendants`): each live one adds its own CPU since the
    /// last sample (since it started, when first seen) and what its waited-for children's counter grew by; the
    /// daemon adds what children_cpu() grew by. From each counter's growth comes off what was already counted for the
    /// processes behind it; never below 0, and what a counter hasn't shown yet is taken off a later window.
    fn descendants(
        &self,
        tree: &Tree,
        reaped: f64,
    ) -> (HashMap<Key, f64>, f64, HashMap<Key, Seen>, Owed) {
        let mark = &self.mark;
        let mut owed = mark.owed.clone();
        let mut gone: Vec<&Key> = mark
            .tracked
            .keys()
            .filter(|k| !tree.contains_key(k))
            .collect();
        // deepest first (Python's sort is stable over the dict's order; ties broken by the key here)
        gone.sort_by(|a, b| {
            mark.tracked[*b]
                .depth
                .cmp(&mark.tracked[*a].depth)
                .then(a.cmp(b))
        });
        for key in gone {
            let seen = mark.tracked[key];
            if let Some(still) = self.readings.process(key.0) {
                if still.start == key.1 {
                    owed.remove(&Some(*key));
                    continue;
                }
            }
            if seen.parent.is_none() || mark.tracked.contains_key(&seen.parent.unwrap_or_default())
            {
                let carried = owed.remove(&Some(*key)).unwrap_or(0.0);
                *owed.entry(seen.parent).or_insert(0.0) +=
                    seen.cpu_s + seen.children_cpu_s + carried;
            }
        }
        let mut live: HashMap<Key, f64> = HashMap::new();
        for (key, (p, _, _)) in tree {
            let before = mark.tracked.get(key);
            let own = p.cpu_s - before.map_or(0.0, |b| b.cpu_s);
            let grown = p.children_cpu_s - before.map_or(0.0, |b| b.children_cpu_s);
            live.insert(*key, own.max(0.0) + settle(&mut owed, Some(*key), grown));
        }
        let finished = settle(&mut owed, None, reaped - mark.reaped_cpu);
        let tracked: HashMap<Key, Seen> = tree
            .iter()
            .map(|(k, (p, parent, depth))| {
                (
                    *k,
                    Seen {
                        cpu_s: p.cpu_s,
                        children_cpu_s: p.children_cpu_s,
                        parent: *parent,
                        depth: *depth,
                    },
                )
            })
            .collect();
        let total = live.values().sum::<f64>() + finished;
        let owed = owed
            .into_iter()
            .filter(|(k, _)| k.is_none() || k.is_some_and(|k| tracked.contains_key(&k)))
            .collect();
        (live, total, tracked, owed)
    }

    /// A FERB.app process's CPU already counted: as FERB.app's, or as the daemon's descendant until it left the
    /// tree; 0 for one first seen, so its CPU since it started counts.
    fn counted(&self, key: &Key) -> f64 {
        self.mark
            .app
            .get(key)
            .copied()
            .or_else(|| self.mark.tracked.get(key).map(|s| s.cpu_s))
            .unwrap_or(0.0)
    }

    /// FERB.app's processes outside the daemon and its descendants (those are counted as the daemon's).
    fn app(&self, tree: &HashSet<i32>) -> HashMap<Key, Process> {
        let mut app = HashMap::new();
        for pid in self.readings.pids() {
            if tree.contains(&pid) || !in_app_bundle(&self.readings.path(pid).unwrap_or_default()) {
                continue;
            }
            if let Some(p) = self.readings.process(pid) {
                app.insert((pid, p.start), p);
            }
        }
        app
    }

    /// The window's counts, each the range from the last sample's maximum to the current one.
    fn work(&self, db: &Db) -> Result<Window, String> {
        let c = db.conn();
        let now = cursors(db)?;
        let counted = |sql: &str, name: &str| -> Result<Vec<(Value, i64)>, String> {
            let mut st = c.prepare(sql).map_err(sql_err)?;
            let rows = st
                .query_map(
                    rusqlite::params![self.mark.cursors.get(name).copied().unwrap_or(0), now[name]],
                    |r| {
                        let k = match r.get_ref(0)? {
                            rusqlite::types::ValueRef::Integer(i) => json!(i),
                            rusqlite::types::ValueRef::Text(t) => {
                                json!(String::from_utf8_lossy(t))
                            }
                            rusqlite::types::ValueRef::Null => Value::Null,
                            other => json!(format!("{other:?}")),
                        };
                        Ok((k, r.get::<_, i64>(1)?))
                    },
                )
                .map_err(sql_err)?;
            rows.collect::<Result<Vec<_>, _>>().map_err(sql_err)
        };
        let mut events = Map::new();
        for (k, n) in counted(EVENTS, "trail")? {
            events.insert(ferb_py::value::py_str(&k), json!(n));
        }
        let jev = counted(JEV, "decisions")?;
        let jev_of = |v: i64| {
            jev.iter()
                .find(|(k, _)| *k == json!(v))
                .map_or(0, |(_, n)| *n)
        };
        let mut models = Map::new();
        let mut tool_calls = 0;
        for (k, n) in counted(MODELS, "spans")? {
            let kind = ferb_py::value::py_str(&k);
            if kind == "tool.call" {
                tool_calls = n;
            } else {
                models.insert(kind, json!(n));
            }
        }
        let running: i64 = c.query_row(RUNNING, [], |r| r.get(0)).map_err(sql_err)?;
        let mut work = Map::new();
        work.insert("events".into(), Value::Object(events));
        work.insert(
            "jev".into(),
            json!({"asked": jev_of(0), "not_sampled": jev_of(1)}),
        );
        work.insert("models".into(), Value::Object(models));
        work.insert("tool_calls".into(), json!(tool_calls));
        work.insert("tasks_running".into(), json!(running));
        Ok((now, work))
    }
}

/// A children counter's growth less what is owed against it, never below 0; what it doesn't cover stays owed.
fn settle(owed: &mut Owed, parent: Option<Key>, grown: f64) -> f64 {
    let pending = owed.remove(&parent).unwrap_or(0.0);
    let grown = grown.max(0.0);
    if pending > grown {
        owed.insert(parent, pending - grown);
    }
    (grown - pending).max(0.0)
}

/// Samples from before the last `days` days (at least 1 like every retention setting), by local day.
pub fn purge(db: &Db, today: chrono::NaiveDate, days: i64) -> Result<i64, String> {
    let cutoff = today - chrono::Duration::days(ferb_store::retention::days(days));
    db.conn()
        .execute(
            "DELETE FROM resource_samples WHERE day < ?",
            [cutoff.format("%Y-%m-%d").to_string()],
        )
        .map(|n| n as i64)
        .map_err(sql_err)
}

// ---------------------------------------------------------------------------------------------------------------
// the Mac's readings
// ---------------------------------------------------------------------------------------------------------------

/// `RUSAGE_INFO_V4`: the first flavor with ri_lifetime_max_phys_footprint.
const RUSAGE_INFO_V4: i32 = 4;

/// `struct rusage_info_v4` (<sys/resource.h>), field for field.
#[repr(C)]
#[derive(Default)]
struct RusageInfoV4 {
    ri_uuid: [u8; 16],
    ri_user_time: u64,
    ri_system_time: u64,
    ri_pkg_idle_wkups: u64,
    ri_interrupt_wkups: u64,
    ri_pageins: u64,
    ri_wired_size: u64,
    ri_resident_size: u64,
    ri_phys_footprint: u64,
    ri_proc_start_abstime: u64,
    ri_proc_exit_abstime: u64,
    ri_child_user_time: u64,
    ri_child_system_time: u64,
    ri_child_pkg_idle_wkups: u64,
    ri_child_interrupt_wkups: u64,
    ri_child_pageins: u64,
    ri_child_elapsed_abstime: u64,
    ri_diskio_bytesread: u64,
    ri_diskio_byteswritten: u64,
    ri_cpu_time_qos_default: u64,
    ri_cpu_time_qos_maintenance: u64,
    ri_cpu_time_qos_background: u64,
    ri_cpu_time_qos_utility: u64,
    ri_cpu_time_qos_legacy: u64,
    ri_cpu_time_qos_user_initiated: u64,
    ri_cpu_time_qos_user_interactive: u64,
    ri_billed_system_time: u64,
    ri_serviced_system_time: u64,
    ri_logical_writes: u64,
    ri_lifetime_max_phys_footprint: u64,
    ri_instructions: u64,
    ri_cycles: u64,
    ri_billed_energy: u64,
    ri_serviced_energy: u64,
    ri_interval_max_phys_footprint: u64,
    ri_runnable_time: u64,
}

#[repr(C)]
#[derive(Default)]
struct Timebase {
    numer: u32,
    denom: u32,
}

#[link(name = "proc")]
extern "C" {
    fn proc_pid_rusage(pid: i32, flavor: i32, buffer: *mut RusageInfoV4) -> i32;
    fn proc_listchildpids(ppid: i32, buffer: *mut i32, buffersize: i32) -> i32;
    fn proc_listallpids(buffer: *mut i32, buffersize: i32) -> i32;
    fn proc_pidpath(pid: i32, buffer: *mut u8, buffersize: u32) -> i32;
}

extern "C" {
    fn mach_timebase_info(info: *mut Timebase) -> i32;
}

/// The mach timebase (numer, denom); an error when it can't be read: CPU must never silently read as 0.
pub fn mach_timebase() -> Result<(u32, u32), String> {
    let mut t = Timebase::default();
    // SAFETY: `t` is a valid, writable timebase struct for the call.
    let r = unsafe { mach_timebase_info(&mut t) };
    if r != 0 || t.numer == 0 || t.denom == 0 {
        return Err(format!(
            "OSError: mach_timebase_info: {r} ({}/{})",
            t.numer, t.denom
        ));
    }
    Ok((t.numer, t.denom))
}

/// The kernel's numbers: getrusage, and libproc.
pub struct MacReadings {
    timebase: (u32, u32),
}

impl MacReadings {
    pub fn new() -> Result<MacReadings, String> {
        Ok(MacReadings {
            timebase: mach_timebase()?,
        })
    }

    fn usage(who: i32) -> f64 {
        // SAFETY: getrusage writes into the zeroed struct it is given.
        let mut u: libc::rusage = unsafe { std::mem::zeroed() };
        // SAFETY: `u` is a valid rusage struct; `who` is RUSAGE_SELF or RUSAGE_CHILDREN.
        unsafe { libc::getrusage(who, &mut u) };
        let s = |t: libc::timeval| t.tv_sec as f64 + f64::from(t.tv_usec) / 1e6;
        s(u.ru_utime) + s(u.ru_stime)
    }

    /// A libproc pid list (its answer is a count of pids); a buffer it fills is too small, so it is asked again.
    fn list(call: &dyn Fn(*mut i32, i32) -> i32) -> Vec<i32> {
        let mut size = 256usize;
        loop {
            let mut buf = vec![0i32; size];
            let n = call(buf.as_mut_ptr(), (size * 4) as i32);
            if n < 0 {
                return Vec::new();
            }
            if (n as usize) < size {
                return buf[..n as usize]
                    .iter()
                    .copied()
                    .filter(|p| *p > 0)
                    .collect();
            }
            size *= 2;
        }
    }
}

impl Readings for MacReadings {
    fn own_cpu(&self) -> Result<f64, String> {
        Ok(Self::usage(libc::RUSAGE_SELF))
    }

    fn children_cpu(&self) -> f64 {
        Self::usage(libc::RUSAGE_CHILDREN)
    }

    fn process(&self, pid: i32) -> Option<Process> {
        let mut info = RusageInfoV4::default();
        // SAFETY: `info` is a correctly laid out rusage_info_v4 for the V4 flavor.
        if pid <= 0 || unsafe { proc_pid_rusage(pid, RUSAGE_INFO_V4, &mut info) } != 0 {
            return None;
        }
        let (n, d) = self.timebase;
        Some(Process {
            pid,
            start: info.ri_proc_start_abstime,
            cpu_s: mach_seconds(info.ri_user_time + info.ri_system_time, n, d),
            footprint: info.ri_phys_footprint as i64,
            peak_footprint: info.ri_lifetime_max_phys_footprint as i64,
            children_cpu_s: mach_seconds(info.ri_child_user_time + info.ri_child_system_time, n, d),
        })
    }

    fn children(&self, pid: i32) -> Vec<i32> {
        // SAFETY: the buffer and its size in bytes are passed together.
        Self::list(&|b, s| unsafe { proc_listchildpids(pid, b, s) })
    }

    fn pids(&self) -> Vec<i32> {
        // SAFETY: the buffer and its size in bytes are passed together.
        Self::list(&|b, s| unsafe { proc_listallpids(b, s) })
    }

    fn path(&self, pid: i32) -> Option<String> {
        let mut buf = vec![0u8; 4096]; // PROC_PIDPATHINFO_MAXSIZE
                                       // SAFETY: the buffer and its size are passed together.
        let n = unsafe { proc_pidpath(pid, buf.as_mut_ptr(), buf.len() as u32) };
        if pid <= 0 || n <= 0 {
            return None;
        }
        buf.truncate(n as usize);
        Some(String::from_utf8_lossy(&buf).into_owned())
    }
}

#[cfg(test)]
mod tests {
    //! tests/test_resource_log.py's cases, on the same invented readings (`Fake`).
    use super::*;
    use std::sync::{Arc, Mutex};

    const DAEMON: i32 = 100;
    const MB: i64 = 1_000_000;

    #[derive(Default)]
    struct State {
        own: f64,
        reaped: f64,
        procs: BTreeMap<i32, Process>,
        kids: BTreeMap<i32, Vec<i32>>,
        paths: BTreeMap<i32, String>,
    }

    #[derive(Clone)]
    struct Fake(Arc<Mutex<State>>);

    fn p(pid: i32, start: u64, cpu: f64, footprint: i64) -> Process {
        Process {
            pid,
            start,
            cpu_s: cpu,
            footprint,
            peak_footprint: footprint,
            children_cpu_s: 0.0,
        }
    }

    impl Fake {
        fn new() -> Fake {
            let mut s = State::default();
            s.procs.insert(DAEMON, p(DAEMON, 1, 0.0, 100 * MB));
            Fake(Arc::new(Mutex::new(s)))
        }
        fn with<T>(&self, f: impl FnOnce(&mut State) -> T) -> T {
            f(&mut self.0.lock().unwrap())
        }
        fn spawn(
            &self,
            pid: i32,
            parent: i32,
            start: u64,
            cpu: f64,
            footprint: i64,
            path: Option<&str>,
        ) {
            self.with(|s| {
                s.procs.insert(pid, p(pid, start, cpu, footprint));
                s.kids.entry(parent).or_default().push(pid);
                if let Some(path) = path {
                    s.paths.insert(pid, path.into());
                }
            })
        }
        fn run(&self, pid: i32, cpu: f64) {
            self.with(|s| s.procs.get_mut(&pid).unwrap().cpu_s = cpu)
        }
        fn waited(&self, pid: i32, cpu: f64) {
            self.with(|s| s.procs.get_mut(&pid).unwrap().children_cpu_s = cpu)
        }
        fn end(&self, pid: i32) {
            self.with(|s| {
                s.procs.remove(&pid);
                for kids in s.kids.values_mut() {
                    kids.retain(|k| *k != pid);
                }
            })
        }
    }

    impl Readings for Fake {
        fn own_cpu(&self) -> Result<f64, String> {
            Ok(self.with(|s| s.own))
        }
        fn children_cpu(&self) -> f64 {
            self.with(|s| s.reaped)
        }
        fn process(&self, pid: i32) -> Option<Process> {
            self.with(|s| s.procs.get(&pid).copied())
        }
        fn children(&self, pid: i32) -> Vec<i32> {
            self.with(|s| s.kids.get(&pid).cloned().unwrap_or_default())
        }
        fn pids(&self) -> Vec<i32> {
            self.with(|s| s.procs.keys().copied().collect())
        }
        fn path(&self, pid: i32) -> Option<String> {
            self.with(|s| s.paths.get(&pid).cloned())
        }
    }

    #[derive(Clone)]
    struct Clock(Arc<Mutex<f64>>);
    impl SampleClock for Clock {
        fn now_iso(&self) -> String {
            format!(
                "2026-01-01T09:{:02}:00+00:00",
                (*self.0.lock().unwrap() / 60.0) as i64 % 60
            )
        }
        fn local_day(&self) -> String {
            "2026-01-01".into()
        }
        fn monotonic(&self) -> f64 {
            *self.0.lock().unwrap()
        }
    }

    struct Run {
        _dir: tempfile::TempDir,
        db: Db,
        clock: Clock,
        log: ResourceLog,
    }

    fn start(fake: &Fake, redact: Box<dyn Fn(&str) -> String + Send>) -> Run {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(dir.path().join("ferb.db")).unwrap();
        db.migrate().unwrap();
        let clock = Clock(Arc::new(Mutex::new(0.0)));
        let log = ResourceLog::new(
            &db,
            Box::new(clock.clone()),
            Box::new(fake.clone()),
            redact,
            Box::new(|| (0, BTreeMap::new())),
            Box::new(Map::new),
            DAEMON,
        )
        .unwrap();
        Run {
            _dir: dir,
            db,
            clock,
            log,
        }
    }

    impl Run {
        fn sample(&mut self) -> Map<String, Value> {
            *self.clock.0.lock().unwrap() += 300.0;
            self.log.sample(&self.db).unwrap()
        }
        fn child(&mut self) -> f64 {
            self.sample()["child_cpu_s"].as_f64().unwrap()
        }
    }

    fn approx(a: f64, b: f64) {
        assert!((a - b).abs() < 1e-6, "{a} != {b}");
    }

    fn plain() -> Box<dyn Fn(&str) -> String + Send> {
        Box::new(|t: &str| t.to_string())
    }

    #[test]
    fn a_sample_takes_the_daemons_cpu_and_memory_from_the_readings() {
        let fake = Fake::new();
        fake.with(|s| s.own = 10.0);
        let mut r = start(&fake, plain());
        fake.with(|s| {
            s.own = 13.5;
            s.procs.insert(
                DAEMON,
                Process {
                    peak_footprint: 900 * MB,
                    ..p(DAEMON, 1, 13.5, 400 * MB)
                },
            );
        });
        let row = r.sample();
        approx(row["window_s"].as_f64().unwrap(), 300.0);
        approx(row["cpu_s"].as_f64().unwrap(), 3.5);
        assert_eq!(row["footprint"], json!(400 * MB));
        assert_eq!(row["peak_footprint"], json!(900 * MB));
        assert!(row["db_bytes"].as_i64().unwrap() > 0);
        assert_eq!(row["child_count"], json!(0));
        assert_eq!(row["app_cpu_s"], Value::Null);
        assert_eq!(row["app_footprint"], Value::Null);
        let stored: i64 =
            r.db.conn()
                .query_row("SELECT COUNT(*) FROM resource_samples", [], |x| x.get(0))
                .unwrap();
        assert_eq!(stored, 1);
    }

    #[test]
    fn a_daemon_that_cant_be_read_has_no_memory_numbers() {
        let fake = Fake::new();
        let mut r = start(&fake, plain());
        fake.with(|s| s.procs.remove(&DAEMON));
        let row = r.sample();
        assert_eq!(row["footprint"], Value::Null);
        assert_eq!(row["peak_footprint"], Value::Null);
        approx(row["cpu_s"].as_f64().unwrap(), 0.0);
    }

    #[test]
    fn mach_time_converts_with_the_timebase() {
        approx(mach_seconds(3_000_000_000, 125, 3), 125.0);
    }

    #[test]
    fn libproc_reads_this_process() {
        let m = MacReadings::new().unwrap();
        let me = m.process(std::process::id() as i32).unwrap();
        assert!(me.footprint > 0 && me.start > 0);
        assert!(m.path(std::process::id() as i32).is_some());
        assert!(m.pids().contains(&(std::process::id() as i32)));
    }

    #[test]
    fn a_new_childs_cpu_is_counted_once() {
        let fake = Fake::new();
        let mut r = start(&fake, plain());
        fake.spawn(200, DAEMON, 7, 2.0, 50 * MB, None);
        let row = r.sample();
        assert_eq!(row["child_count"], json!(1));
        assert_eq!(row["child_footprint"], json!(50 * MB));
        approx(row["child_cpu_s"].as_f64().unwrap(), 2.0);
        fake.run(200, 2.5);
        approx(r.child(), 0.5);
        approx(r.child(), 0.0);
    }

    #[test]
    fn descendants_are_walked_through_their_parents() {
        let fake = Fake::new();
        let mut r = start(&fake, plain());
        fake.spawn(200, DAEMON, 7, 1.0, 10 * MB, None);
        fake.spawn(300, 200, 8, 2.0, 20 * MB, None);
        fake.spawn(400, 300, 9, 3.0, 30 * MB, None);
        fake.with(|s| s.procs.insert(500, p(500, 3, 9.0, 99 * MB)));
        let row = r.sample();
        assert_eq!(row["child_count"], json!(3));
        assert_eq!(row["child_footprint"], json!(60 * MB));
        approx(row["child_cpu_s"].as_f64().unwrap(), 6.0);
    }

    #[test]
    fn a_reused_pid_with_a_new_start_time_is_counted_fresh() {
        let fake = Fake::new();
        let mut r = start(&fake, plain());
        fake.spawn(200, DAEMON, 7, 5.0, 0, None);
        approx(r.child(), 5.0);
        fake.end(200);
        fake.with(|s| s.reaped += 5.0);
        fake.spawn(200, DAEMON, 9, 1.0, 0, None);
        approx(r.child(), 1.0);
    }

    #[test]
    fn a_finished_direct_child_is_not_counted_again_through_rusage_children() {
        let fake = Fake::new();
        let mut r = start(&fake, plain());
        fake.spawn(200, DAEMON, 7, 2.0, 0, None);
        approx(r.child(), 2.0);
        fake.end(200);
        fake.with(|s| s.reaped += 2.3);
        let row = r.sample();
        approx(row["child_cpu_s"].as_f64().unwrap(), 0.3);
        assert_eq!(row["child_count"], json!(0));
        fake.spawn(201, DAEMON, 8, 0.5, 0, None);
        fake.end(201);
        fake.with(|s| s.reaped += 0.5);
        approx(r.child(), 0.5);
    }

    #[test]
    fn a_child_reaped_after_the_sample_that_missed_it_is_counted_once() {
        let fake = Fake::new();
        let mut r = start(&fake, plain());
        fake.spawn(200, DAEMON, 7, 2.0, 0, None);
        approx(r.child(), 2.0);
        fake.end(200);
        approx(r.child(), 0.0);
        fake.with(|s| s.reaped += 2.4);
        approx(r.child(), 0.4);
        fake.with(|s| s.reaped += 1.0);
        approx(r.child(), 1.0);
    }

    #[test]
    fn a_grandchild_its_parent_waited_for_is_not_counted_again() {
        let fake = Fake::new();
        let mut r = start(&fake, plain());
        fake.spawn(200, DAEMON, 7, 1.0, 0, None);
        fake.spawn(300, 200, 8, 4.0, 0, None);
        approx(r.child(), 5.0);
        fake.end(300);
        fake.waited(200, 4.5);
        approx(r.child(), 0.5);
        fake.end(200);
        fake.with(|s| s.reaped += 1.2 + 4.5);
        approx(r.child(), 0.2);
    }

    #[test]
    fn a_command_a_live_child_ran_counts_in_the_window_it_ended() {
        let fake = Fake::new();
        let mut r = start(&fake, plain());
        fake.spawn(200, DAEMON, 7, 1.0, 0, None);
        approx(r.child(), 1.0);
        fake.waited(200, 3.0);
        approx(r.child(), 3.0);
        fake.run(200, 1.5);
        fake.end(200);
        fake.with(|s| s.reaped += 1.5 + 3.0);
        approx(r.child(), 0.5);
    }

    #[test]
    fn descendants_under_an_unreadable_process_are_walked_and_counted_once() {
        let fake = Fake::new();
        let mut r = start(&fake, plain());
        fake.spawn(200, DAEMON, 7, 1.0, 0, None);
        fake.spawn(300, 200, 8, 2.0, 0, None);
        fake.spawn(400, 300, 9, 4.0, 0, None);
        fake.with(|s| s.procs.remove(&300));
        let row = r.sample();
        assert_eq!(row["child_count"], json!(2));
        approx(row["child_cpu_s"].as_f64().unwrap(), 5.0);
        fake.end(400);
        approx(r.child(), 0.0);
        fake.with(|s| s.kids.get_mut(&200).unwrap().retain(|k| *k != 300));
        fake.waited(200, 2.5 + 4.5);
        approx(r.child(), 2.5 + 0.5);
    }

    #[test]
    fn the_orphaned_child_of_an_unreadable_process_is_never_taken_off() {
        let fake = Fake::new();
        let mut r = start(&fake, plain());
        fake.spawn(200, DAEMON, 7, 1.0, 0, None);
        fake.with(|s| s.procs.remove(&200));
        fake.spawn(300, 200, 8, 4.0, 0, None);
        approx(r.child(), 4.0);
        fake.with(|s| {
            s.kids.get_mut(&DAEMON).unwrap().retain(|k| *k != 200);
            s.kids.remove(&200);
            s.kids.entry(1).or_default().push(300);
            s.reaped += 1.0;
        });
        fake.run(300, 4.2);
        let row = r.sample();
        approx(row["child_cpu_s"].as_f64().unwrap(), 1.0);
        assert_eq!(row["child_count"], json!(0));
    }

    #[test]
    fn a_descendant_left_running_by_its_parent_is_never_taken_off() {
        let fake = Fake::new();
        let mut r = start(&fake, plain());
        fake.spawn(200, DAEMON, 7, 1.0, 0, None);
        fake.spawn(300, 200, 8, 4.0, 0, None);
        approx(r.child(), 5.0);
        fake.end(200);
        fake.with(|s| {
            s.reaped += 1.1;
            s.procs.insert(300, p(300, 8, 4.2, 0));
            s.kids.entry(1).or_default().push(300);
        });
        let row = r.sample();
        approx(row["child_cpu_s"].as_f64().unwrap(), 0.1);
        assert_eq!(row["child_count"], json!(0));
    }

    #[test]
    fn the_top_three_processes_are_named_by_their_redacted_executable() {
        let fake = Fake::new();
        let mut r = start(&fake, Box::new(|t: &str| t.replace("secret", "[REDACTED]")));
        fake.spawn(200, DAEMON, 7, 30.0, 0, Some("/Users/k/.ferb/bin/codex"));
        fake.spawn(201, DAEMON, 8, 2.0, 0, Some("/bin/zsh"));
        fake.spawn(202, DAEMON, 9, 5.0, 0, Some("/Users/k/project/secret-tool"));
        fake.spawn(203, DAEMON, 10, 0.1, 0, Some("/bin/sleep"));
        fake.spawn(204, DAEMON, 11, 0.0, 0, Some("/bin/cat"));
        fake.spawn(205, DAEMON, 12, 9.0, 0, None);
        let row = r.sample();
        let work: Value = serde_json::from_str(row["work_json"].as_str().unwrap()).unwrap();
        assert_eq!(
            work["top"],
            json!([["codex", 30.0], ["unknown", 9.0], ["[REDACTED]-tool", 5.0]])
        );
        assert!(!row["work_json"].as_str().unwrap().contains("/Users"));
    }

    #[test]
    fn ferb_app_is_its_processes_outside_the_daemons_tree() {
        let fake = Fake::new();
        fake.with(|s| {
            s.paths.insert(
                DAEMON,
                "/Applications/FERB.app/Contents/Helpers/FERB.app/Contents/MacOS/FERB".into(),
            );
            s.procs.insert(
                500,
                Process {
                    peak_footprint: 90 * MB,
                    ..p(500, 2, 100.0, 80 * MB)
                },
            );
            s.paths.insert(
                500,
                "/Applications/FERB.app/Contents/MacOS/FERBNotch".into(),
            );
            s.procs.insert(700, p(700, 3, 50.0, 1));
            s.paths
                .insert(700, "/Applications/Safari.app/Contents/MacOS/Safari".into());
        });
        fake.spawn(
            200,
            DAEMON,
            4,
            9.0,
            0,
            Some("/Applications/FERB.app/Contents/Resources/python/bin/python3.12"),
        );
        let mut r = start(&fake, plain());
        fake.run(500, 103.0);
        fake.run(200, 10.0);
        fake.with(|s| {
            s.procs.insert(600, p(600, 5, 0.5, 7 * MB));
            s.paths.insert(
                600,
                "/Applications/FERB.app/Contents/MacOS/FERBNotch".into(),
            );
        });
        fake.run(700, 60.0);
        let row = r.sample();
        approx(row["app_cpu_s"].as_f64().unwrap(), 3.5);
        assert_eq!(row["app_footprint"], json!(87 * MB));
        approx(row["child_cpu_s"].as_f64().unwrap(), 1.0);
        assert_eq!(row["child_count"], json!(1));
        fake.end(500);
        fake.end(600);
        let row = r.sample();
        assert_eq!(row["app_cpu_s"], Value::Null);
        assert_eq!(row["app_footprint"], Value::Null);
    }

    #[test]
    fn a_process_leaving_the_daemons_tree_for_ferb_app_is_counted_once() {
        let fake = Fake::new();
        fake.spawn(200, DAEMON, 4, 1.0, 0, None);
        fake.spawn(
            300,
            200,
            5,
            2.0,
            0,
            Some("/Applications/FERB.app/Contents/MacOS/FERBNotch"),
        );
        let mut r = start(&fake, plain());
        fake.run(300, 3.0);
        let row = r.sample();
        approx(row["child_cpu_s"].as_f64().unwrap(), 1.0);
        assert_eq!(row["app_cpu_s"], Value::Null);
        fake.end(200);
        fake.with(|s| {
            s.reaped += 1.0;
            s.procs.insert(300, p(300, 5, 3.5, 7 * MB));
            s.kids.entry(1).or_default().push(300);
        });
        let row = r.sample();
        approx(row["child_cpu_s"].as_f64().unwrap(), 0.0);
        assert_eq!(row["child_count"], json!(0));
        approx(row["app_cpu_s"].as_f64().unwrap(), 0.5);
        assert_eq!(row["app_footprint"], json!(7 * MB));
    }

    #[test]
    fn the_work_counts_are_what_was_added_since_the_last_sample() {
        let fake = Fake::new();
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(dir.path().join("ferb.db")).unwrap();
        db.migrate().unwrap();
        let c = db.conn();
        let trail = |id: &str, source: &str| {
            c.execute(
                "INSERT INTO trail (id, at, day, device_id, actor, source, data_json) VALUES (?, 'x', 'd', 'dev', \
                 'owner', ?, '{}')",
                [id, source],
            )
            .unwrap();
        };
        let span = |id: &str, kind: &str| {
            c.execute(
                "INSERT INTO spans (id, kind, started_at, input_json) VALUES (?, ?, 'x', '{}')",
                [id, kind],
            )
            .unwrap();
        };
        trail("t0", "accessibility");
        span("s0", "worker");
        let counters = Arc::new(Mutex::new((
            2i64,
            BTreeMap::from([("age".to_string(), 1i64)]),
        )));
        let previews = Arc::new(Mutex::new(crate::basics::object(
            json!({"requests": 4, "frames": 3, "capture_ms": 90.0, "cached": 1, "busy": 0}),
        )));
        let clock = Clock(Arc::new(Mutex::new(0.0)));
        let (cc, pp) = (counters.clone(), previews.clone());
        let mut log = ResourceLog::new(
            &db,
            Box::new(clock.clone()),
            Box::new(fake),
            plain(),
            Box::new(move || cc.lock().unwrap().clone()),
            Box::new(move || pp.lock().unwrap().clone()),
            DAEMON,
        )
        .unwrap();
        for (i, s) in [
            "accessibility",
            "app_focus",
            "accessibility",
            "app_focus",
            "accessibility",
        ]
        .iter()
        .enumerate()
        {
            trail(&format!("t{}", i + 1), s);
        }
        for (i, kind) in [
            "model.turn",
            "worker",
            "model.learning",
            "tool.call",
            "tool.call",
            "tool.call",
            "task",
        ]
        .iter()
        .enumerate()
        {
            span(&format!("s{}", i + 1), kind);
        }
        c.execute(
            "INSERT INTO tasks (id, tree_id, request_id, goal, identity, role, scopes_json, status, created_at, \
             updated_at) VALUES ('a', 'a', 'r', 'g', 'owner', 'mixed', '[]', 'running', 'x', 'x')",
            [],
        )
        .unwrap();
        *counters.lock().unwrap() = (
            7,
            BTreeMap::from([("age".to_string(), 2), ("tokens".to_string(), 1)]),
        );
        *previews.lock().unwrap() = crate::basics::object(
            json!({"requests": 10, "frames": 8, "capture_ms": 340.5, "cached": 2, "busy": 1}),
        );
        *clock.0.lock().unwrap() += 300.0;
        let row = log.sample(&db).unwrap();
        let work: Value = serde_json::from_str(row["work_json"].as_str().unwrap()).unwrap();
        assert_eq!(
            work,
            json!({"events": {"accessibility": 3, "app_focus": 2},
                   "perception": {"wakes": 5, "batches": {"age": 1, "tokens": 1}},
                   "jev": {"asked": 0, "not_sampled": 0},
                   "models": {"model.learning": 1, "model.turn": 1, "worker": 1}, "tool_calls": 3,
                   "tasks_running": 1, "top": [],
                   "preview": {"requests": 6, "frames": 5, "capture_ms": 250.5, "cached": 1, "busy": 1}})
        );
    }

    #[test]
    fn samples_older_than_resource_days_are_purged() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(dir.path().join("ferb.db")).unwrap();
        db.migrate().unwrap();
        let today = chrono::NaiveDate::from_ymd_opt(2026, 10, 23).unwrap();
        for back in [0, 1, 2, 30, 31, 40] {
            let day = (today - chrono::Duration::days(back))
                .format("%Y-%m-%d")
                .to_string();
            db.conn()
                .execute(
                    "INSERT INTO resource_samples (at, day, window_s, cpu_s, footprint, peak_footprint, \
                     child_count, child_footprint, child_cpu_s, app_cpu_s, app_footprint, db_bytes, work_json) \
                     VALUES (?, ?, 300, 1, 1, 1, 1, 10, 0, NULL, NULL, 1, '{}')",
                    [format!("{day}T10:00:00+05:30"), day],
                )
                .unwrap();
        }
        assert_eq!(purge(&db, today, 30).unwrap(), 2);
        assert_eq!(purge(&db, today, 0).unwrap(), 2);
        let left: Vec<String> = db
            .conn()
            .prepare("SELECT day FROM resource_samples ORDER BY day DESC")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        assert_eq!(left, ["2026-10-23", "2026-10-22"]);
    }
}
