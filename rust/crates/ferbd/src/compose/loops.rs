//! The daemon's background work after the start (`_build_timers`, `_build_memory`'s loops, `_build_sense`): the
//! timer loop (scheduled runs, reminders, timers) and the schedules reconciled at start, held banners flushed every
//! 10 s, the hourly retention pass starting a minute after start-up, and sensing with its trail.
//!
//! Each loop is supervised as `_supervise` does: a loop that fails is journaled and started again after a pause
//! that doubles up to a minute.

use std::sync::Arc;
use std::time::Duration;

use ferb_daemon::areas_a;
use ferb_sense::manager::{Emit, Hooks, Sense};
use ferb_sense::observation::Observation;
use serde_json::{json, Value};

use super::areas::Shared;
use super::{Daemon, Options};

/// `LOOP_RESTART_S`, `LOOP_RESTART_MAX_S` (main.py:93).
pub const LOOP_RESTART_S: f64 = 1.0;
pub const LOOP_RESTART_MAX_S: f64 = 60.0;

pub fn start(d: &Arc<Daemon>) {
    // the timer loop runs on group A's thread, where its state lives
    if d.opts.timer_loop {
        let a = d.a.clone();
        let _ = a.spawn(|st| async move { areas_a::schedules::run_timers(st).await });
    }

    // held banners (notify.py:117)
    let n = d.b.notifier.clone();
    d.keep(tokio::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_secs(10)).await;
            n.flush_held().await;
        }
    }));

    // the retention pass: hourly, a minute after start-up (retention.py:315)
    if d.opts.retention {
        let a = d.a.clone();
        let purges: Arc<dyn areas_a::retention::Purges> = Arc::new(Purges {
            home: d.opts.home.clone(),
            redactor: d.redactor.clone(),
            clock: d.opts.clock.clone(),
            ids: d.opts.ids.clone(),
        });
        let j = d.journal.clone();
        d.keep(tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(60)).await;
            loop {
                let p = purges.clone();
                let r =
                    a.call(move |st| async move {
                        areas_a::retention::purge_pass(&st, p.as_ref()).await
                    })
                    .await;
                match r {
                    Ok(Ok(_)) => {}
                    Ok(Err(e)) => j.note("retention.failed", json!({"error": e})),
                    Err(e) => j.note("retention.failed", json!({"error": e.to_string()})),
                }
                tokio::time::sleep(Duration::from_secs(3600)).await;
            }
        }));
    }

    // sensing: the watchers per config (a daemon with its Mac parts off starts none)
    if let Some(sense) = d.b.sense.clone() {
        d.keep(tokio::spawn(async move { sense.start().await }));
    }
}

// ---------------------------------------------------------------------------------------------------------------
// the retention pass's other stores
// ---------------------------------------------------------------------------------------------------------------

/// What the retention pass deletes outside group A's stores.
pub struct Purges {
    pub home: std::path::PathBuf,
    pub redactor: Arc<ferb_sanitize::redact::Redactor>,
    pub clock: Arc<dyn ferb_clock::clock::ZonedClock>,
    pub ids: Arc<dyn ferb_core::interfaces::IdSource>,
}

impl areas_a::retention::Purges for Purges {
    fn compose_episode(
        &self,
        db: &ferb_store::Db,
        task_id: &str,
    ) -> Result<Value, (String, String)> {
        let r = self.redactor.clone();
        let clock: Arc<dyn ferb_core::interfaces::Clock> = self.clock.clone();
        let spans = ferb_store::spans::SpanStore::new(
            db,
            Box::new(ferb_gateway::gateway::StoreTime(clock)),
            Box::new(ferb_gateway::gateway::StoreIds(self.ids.clone())),
            Box::new(move |v: &Value| r.value(v)),
            Box::new(|kind: &str| ferb_policy::rules::REVERSIBLE.contains(&kind)),
        );
        let err = |e: ferb_store::Error| ("OperationalError".to_string(), e.to_string());
        let trace = json!({"task_id": task_id, "spans": spans.tree_json(task_id).map_err(err)?,
                           "outcome": spans.outcome(task_id).map_err(err)?});
        Ok(ferb_trace::compose::compose(&trace))
    }

    fn trail_accessibility(&self, days: i64) -> Result<i64, String> {
        self.with_trail(|t| {
            t.purge_source_steps("accessibility", days, None, 200)
                .try_fold(0i64, |n, r| r.map(|k| n + k as i64))
        })
    }

    fn trail(&self, days: i64) -> Result<(Vec<String>, i64), String> {
        self.with_trail(|t| {
            let (days, _sources, deletes) = t.purge_steps(days, None)?;
            let rows = deletes
                .into_iter()
                .try_fold(0i64, |n, r| r.map(|k| n + k as i64))?;
            Ok::<_, ferb_sense::trail::Error>((days, rows))
        })
    }

    fn evidence(&self, before: &str) -> i64 {
        let Ok(db) = ferb_store::Db::open(self.home.join("ferb.db")) else {
            return 0;
        };
        let r = self.redactor.clone();
        let Ok(store) = ferb_mac_hands::evidence::EvidenceStore::new(
            &self.home.join("evidence"),
            Arc::new(|| (String::new(), String::new())),
            Arc::new(String::new),
            Arc::new(move |v: &Value| r.value(v)),
            Arc::new(NoPaint),
        ) else {
            return 0;
        };
        store
            .purge_before(db.conn(), before)
            .map_or(0, |n| n as i64)
    }

    fn recovery(&self, before: &str, trash_backstop: &str) -> i64 {
        let Ok(db) = ferb_store::Db::open(self.home.join("ferb.db")) else {
            return 0;
        };
        ferb_fs::recovery::purge_before(
            before,
            db.conn(),
            &self.home.join("recovery").to_string_lossy(),
            Some(trash_backstop),
        )
        .map_or(0, |n| n as i64)
    }

    fn resources(&self, _today: chrono::NaiveDate, _days: i64) -> Result<i64, String> {
        // the resource sampler (L30) isn't composed in this build: no rows to purge
        Ok(0)
    }

    fn shell_logs(&self, _before_ts: f64) -> Result<i64, String> {
        Ok(0)
    }

    fn perception_text(&self) {}

    fn downloads_over_cap(&self) -> Result<Option<areas_a::retention::OverCap>, String> {
        Ok(None)
    }

    fn downloads_delete(&self, _ids: &[String]) -> (i64, i64) {
        (0, 0)
    }
}

impl Purges {
    fn with_trail<T, E: std::fmt::Display>(
        &self,
        f: impl FnOnce(&ferb_sense::trail::Trail<'_>) -> Result<T, E>,
    ) -> Result<T, String> {
        let db = ferb_store::Db::open(self.home.join("ferb.db")).map_err(|e| e.to_string())?;
        let ids = IdsRef(self.ids.clone());
        let trail = ferb_sense::trail::Trail::new(
            &db,
            self.clock.as_ref(),
            &ids,
            self.redactor.as_ref(),
            quiet_seams(),
            Arc::new(ferb_sense::attribution::Attribution::new(|| 0.0)),
        );
        f(&trail).map_err(|e| e.to_string())
    }
}

/// The evidence store only purges here: nothing is painted.
struct NoPaint;
impl ferb_mac_hands::ocr::Paint for NoPaint {
    fn paint(
        &self,
        _image: &[u8],
        _boxes: &[ferb_mac_hands::mask::ImgBox],
    ) -> Result<Vec<u8>, String> {
        Err("nothing is painted while purging".into())
    }
}

struct IdsRef(Arc<dyn ferb_core::interfaces::IdSource>);
impl ferb_core::interfaces::IdSource for IdsRef {
    fn new_id(&self) -> String {
        self.0.new_id()
    }
}

fn quiet_seams<'a>() -> ferb_sense::trail::Seams<'a> {
    ferb_sense::trail::Seams {
        exclusions: Box::new(ferb_sense::exclusions::Exclusions::defaults),
        publish: Box::new(|_, _| {}),
        device_id: Box::new(String::new),
        paused: Box::new(|| false),
        test_task: Box::new(|_| false),
        is_browser: Box::new(ferb_mac_hands::apps::known),
    }
}

// ---------------------------------------------------------------------------------------------------------------
// sensing and its trail
// ---------------------------------------------------------------------------------------------------------------

/// The trail on a thread of its own (its state is not shared across threads): observations come in through a
/// channel and are recorded in order, as Python's `Trail.record` on the loop.
fn trail_thread(opts: &Options, s: &Shared) -> Emit {
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<Observation>();
    let home = s.home.clone();
    let clock = opts.clock.clone();
    let ids = opts.ids.clone();
    let redactor = s.redactor.clone();
    let config = s.config.clone();
    let events = s.events.clone();
    let paused_marker = s.home.join("PAUSED");
    std::thread::Builder::new()
        .name("ferb-trail".into())
        .spawn(move || {
            let Ok(db) = ferb_store::Db::open(home.join("ferb.db")) else {
                return;
            };
            let ids = IdsRef(ids);
            let device = match config.get("device", "id") {
                Ok(Some(Value::String(s))) => s,
                _ => "unknown-device".into(),
            };
            let cfg = config.clone();
            let seams = ferb_sense::trail::Seams {
                exclusions: Box::new(move || {
                    ferb_daemon::areas_b::sensing::exclusions(&host_for(&cfg))
                        .unwrap_or_else(|_| ferb_sense::exclusions::Exclusions::defaults())
                }),
                publish: Box::new(move |topic, data| {
                    events.publish(topic, crate::basics::object(data))
                }),
                device_id: Box::new(move || device.clone()),
                paused: Box::new(move || paused_marker.exists()),
                test_task: Box::new(|_| false),
                is_browser: Box::new(ferb_mac_hands::apps::known),
            };
            let trail = ferb_sense::trail::Trail::new(
                &db,
                clock.as_ref(),
                &ids,
                redactor.as_ref(),
                seams,
                Arc::new(ferb_sense::attribution::Attribution::new(|| 0.0)),
            );
            while let Some(o) = rx.blocking_recv() {
                let _ = trail.record(&o);
            }
        })
        .expect("the trail's thread");
    Arc::new(move |o: Observation| {
        let _ = tx.send(o);
    })
}

fn host_for(config: &Arc<ferb_core::config::Config>) -> ferb_daemon::areas_b::core::Host {
    // `exclusions` reads only the config through the host
    struct Silent;
    impl ferb_daemon::areas_b::core::Journal for Silent {
        fn append(&self, _k: &str, _d: Value, _o: Option<&str>) -> Result<(), String> {
            Ok(())
        }
    }
    impl ferb_daemon::areas_b::core::Bus for Silent {
        fn publish(&self, _t: &str, _d: Value) {}
    }
    ferb_daemon::areas_b::core::Host {
        home: std::path::PathBuf::new(),
        config: config.clone(),
        redactor: Arc::new(ferb_sanitize::redact::Redactor::new(&[], &[])),
        journal: Arc::new(Silent),
        bus: Arc::new(Silent),
        clock: Arc::new(ferb_clock::clock::SystemClock::new(
            ferb_clock::zone::Zone::utc(),
        )),
    }
}

/// `_build_sense`: the watcher manager with the trail. The watchers per config are started when the Mac's parts
/// are on; Jev stays off and notices paused (no perception watcher).
pub fn sense(opts: &Options, s: &Shared) -> Sense {
    let emit = trail_thread(opts, s);
    let host = ferb_daemon::areas_b::core::Host {
        home: s.home.clone(),
        config: s.config.clone(),
        redactor: s.redactor.clone(),
        journal: s.journal.clone(),
        bus: s.events.clone(),
        clock: opts.clock.clone(),
    };
    let j2 = s.journal.clone();
    let hooks = Hooks {
        // `_on_pause` (sensing.py): journaled and published; the owner's tabs forgotten (the router's)
        on_pause: Arc::new(move |paused| {
            let _ = ferb_daemon::areas_b::sensing::on_pause(&host, paused, &|| {});
        }),
        on_fail: Arc::new(move |source, why| {
            j2.note(
                "sense.watcher_failed",
                json!({"source": source, "error": why}),
            )
        }),
        ..Hooks::default()
    };
    let watchers = if opts.native {
        native_watchers(&s.config, &opts.clock)
    } else {
        Vec::new()
    };
    Sense::new(s.home.clone(), watchers, emit, None, None, hooks)
}

/// The watchers `_build_sense` starts with `run_watchers` (sensing.py:71-76) whose native reads are composed here:
/// the frontmost app, the focused window's title (per `[sense] window_titles`) and idleness. Not composed: files
/// (FSEvents over the watch folders), app installs, the browsers' scripted tabs, perception's native watchers.
fn native_watchers(
    config: &Arc<ferb_core::config::Config>,
    clock: &Arc<dyn ferb_clock::clock::ZonedClock>,
) -> Vec<Arc<dyn ferb_sense::manager::Watcher>> {
    use ferb_sense::watchers::{
        app_focus::AppFocusWatcher, idle::IdleWatcher, windows::WindowTitleWatcher,
    };
    let cfg = config.clone();
    let titles = Arc::new(move || {
        !matches!(
            cfg.get("sense", "window_titles"),
            Ok(Some(Value::Bool(false)))
        )
    });
    let idle_after = match config.get("sense", "idle_after_s") {
        Ok(Some(v)) => v.as_f64().unwrap_or(120.0),
        _ => 120.0,
    };
    let c = clock.clone();
    vec![
        Arc::new(AppFocusWatcher::new(
            Arc::new(ferb_sense::mac::apps::focused_app),
            Arc::new(|_| {}),
        )),
        Arc::new(WindowTitleWatcher::new(
            titles,
            Arc::new(|| {
                let f = ferb_sense::mac::apps::focused_app()?;
                let title = ferb_sense::mac::apps::focused_title(f.pid);
                Some((f, title))
            }),
            Arc::new(ferb_mac_hands::apps::known),
        )),
        Arc::new(IdleWatcher::new(
            Arc::new(|| None),
            Arc::new(|| false),
            idle_after,
            Arc::new(move || c.now().timestamp_micros() as f64 / 1e6),
        )),
    ]
}
