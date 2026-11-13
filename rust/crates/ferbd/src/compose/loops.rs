//! The daemon's background work after the start (`_build_timers`, `_build_memory`'s loops, `_build_sense`): the
//! timer loop (scheduled runs, reminders, timers) and the schedules reconciled at start, held banners flushed every
//! 10 s, the hourly retention pass starting a minute after start-up, and sensing with its trail.
//!
//! Each loop is supervised as `_supervise` does: a loop that fails is journaled and started again after a pause
//! that doubles up to a minute.

use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::SyncSender;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use ferb_core::interfaces::BoxFuture;
use ferb_daemon::areas_a;
use ferb_daemon::areas_b::core::Host;
use ferb_sanitize::redact::Redactor;
use ferb_sense::manager::{Emit, Hooks, Sense, Stop, Watcher};
use ferb_sense::observation::Observation;
use ferb_sense::perception::Banner;
use serde_json::{json, Map, Value};

use super::areas::Shared;
use super::tools::{Parts, TrailHandle};
use super::{Daemon, Options};

/// `LOOP_RESTART_S`, `LOOP_RESTART_MAX_S` (main.py:93).
pub const LOOP_RESTART_S: f64 = 1.0;
pub const LOOP_RESTART_MAX_S: f64 = 60.0;

pub fn start(d: &Arc<Daemon>) {
    // `_build_timers` (ORCH-317): timers held for the owner follow the Mac's presence; its return wakes the timers and
    // brings the day's paused-task mentions (`_owner_back`)
    if let Some(o) = d.tools.owner() {
        let presence: Arc<dyn ferb_clock::timers::Presence + Send + Sync> =
            Arc::new(super::presence::TimerPresence(o.presence.clone()));
        let _ =
            d.a.spawn(move |st| async move { st.set_presence(presence) });
        let a = d.a.clone();
        o.presence.set_on_return(Some(Arc::new(move |signal: &str| {
            let signal = signal.to_string();
            let _ = a.spawn(move |st| async move {
                areas_a::schedules::owner_back(&st, &signal);
            });
        })));
    }
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
        let purges = purges(&d.opts, &d.tools, &d.redactor);
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

    // the resource log: a sample every SAMPLE_SECONDS (the owner's "every 5 minutes"); shutdown writes the last
    if crate::basics::lock(&d.resource_log).is_some() {
        let weak = Arc::downgrade(d);
        d.keep(tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_secs(crate::resources::SAMPLE_SECONDS)).await;
                match weak.upgrade() {
                    Some(d) => d.sample_resources(),
                    None => return,
                }
            }
        }));
    }

    // the memory: the search re-index, the embedder, the hourly pass (distill, cards, playbooks, the profile)
    if d.opts.memory_loops {
        super::memory_loops::start(d);
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
    pub redactor: Arc<Redactor>,
    pub clock: Arc<dyn ferb_clock::clock::ZonedClock>,
    pub ids: Arc<dyn ferb_core::interfaces::IdSource>,
    /// The shell manager (its output logs), the browsers (FERB's downloads folder), perception (typed text).
    pub tools: Parts,
}

/// The retention pass's other stores over this daemon's parts (the pass in [`start`], and group A's `purges`).
pub fn purges(
    opts: &Options,
    tools: &Parts,
    redactor: &Arc<Redactor>,
) -> Arc<dyn areas_a::retention::Purges> {
    Arc::new(Purges {
        home: opts.home.clone(),
        redactor: redactor.clone(),
        clock: opts.clock.clone(),
        ids: opts.ids.clone(),
        tools: tools.clone(),
    })
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
            // only purged here: nothing is stored, so no app or task is named and nothing is painted
            crate::off::off(
                "evidence.purge_only",
                Arc::new(|| (String::new(), String::new())),
            ),
            crate::off::off("evidence.purge_only", Arc::new(String::new)),
            Arc::new(move |v: &Value| r.value(v)),
            crate::off::off("evidence.purge_only", Arc::new(NoPaint)),
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

    fn resources(&self, today: chrono::NaiveDate, days: i64) -> Result<i64, String> {
        let db = ferb_store::Db::open(self.home.join("ferb.db")).map_err(|e| e.to_string())?;
        crate::resources::purge(&db, today, days)
    }

    /// retention.py:218-230 `_purge_shell_logs`: finished commands' output logs older than the cut-off (none without
    /// a shell manager).
    fn shell_logs(&self, before_ts: f64) -> Result<i64, String> {
        Ok(self
            .tools
            .shells()
            .map_or(0, |shells| shells.purge_logs(before_ts) as i64))
    }

    /// retention.py:214 `perception.purge_text()` (D14/P8): typed and copied text expires after 7 days. It runs on
    /// the trail's thread, where perception lives, after the pass's own deletes.
    fn perception_text(&self) {
        if let Some(p) = self.tools.perception() {
            p.purge_text();
        }
    }

    /// retention.py:250 `downloads.over_cap()` (HAND-211); none without the browsers.
    fn downloads_over_cap(&self) -> Result<Option<areas_a::retention::OverCap>, String> {
        let Some(b) = self.tools.browsers() else {
            return Ok(None);
        };
        b.downloads
            .over_cap()
            .map(|over| {
                over.map(|o| areas_a::retention::OverCap {
                    ids: o.ids,
                    total: o.total,
                    cap: o.cap,
                    bytes: o.bytes,
                })
            })
            .map_err(|f| f.typed())
    }

    /// retention.py:296 `downloads.delete(ids)`: (files, bytes) deleted, only inside the folder. A failure deletes
    /// nothing more and counts what it reached as none (Python's exception ended the answer there).
    fn downloads_delete(&self, ids: &[String]) -> (i64, i64) {
        self.tools
            .browsers()
            .and_then(|b| b.downloads.delete(ids).ok())
            .unwrap_or((0, 0))
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
            self.tools.attribution(),
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

/// The seams of a trail that only reads and purges (the retention pass's): `record` never runs on it, so nothing it
/// would publish, attribute, pause or exclude is reached.
fn quiet_seams<'a>() -> ferb_sense::trail::Seams<'a> {
    use crate::off::off;
    ferb_sense::trail::Seams {
        exclusions: Box::new(ferb_sense::exclusions::Exclusions::defaults),
        publish: off("trail.reads_only", Box::new(|_, _| {})),
        device_id: off("trail.reads_only", Box::new(String::new)),
        paused: off("trail.reads_only", Box::new(|| false)),
        test_task: off("trail.reads_only", Box::new(|_| false)),
        is_browser: Box::new(ferb_mac_hands::apps::known),
    }
}

// ---------------------------------------------------------------------------------------------------------------
// sensing, its trail and perception
// ---------------------------------------------------------------------------------------------------------------

/// What the trail's thread does, in the order it arrives (Python's loop): record an observation, or one of
/// perception's calls (`ferb/sense/perception.py`), answered on the reply channel where it has an answer.
enum Work {
    Record(Observation),
    Notification(Observation),
    Seen(String, SyncSender<bool>),
    Floor(Vec<Observation>, SyncSender<Vec<Observation>>),
    Installed(SyncSender<Vec<(String, String)>>),
    Status(SyncSender<Result<Value, String>>),
    BannerPosted(String, String, String, SyncSender<Banner>),
    BannerFailed(Banner),
    ClipboardAccess(String),
    PurgeText,
}

/// Perception (`areas/sensing.py:65` `self.perception = Perception(self)`), built always: it lives on the trail's
/// thread (it borrows the trail and the store there); this handle reaches it. It also holds what `sense.status`
/// reports beside it: the watch folders `_watch_folders` skipped and the batcher's wakes for the resource log.
#[derive(Clone)]
pub struct Perception {
    tx: tokio::sync::mpsc::UnboundedSender<Work>,
    /// `self.watch_problems`.
    pub watch_problems: Arc<Mutex<Vec<String>>>,
    /// The perception batcher's wakes since start (`batcher.wakes`, for the resource log, L30).
    wakes: Arc<AtomicU64>,
}

impl Perception {
    /// A call with an answer; None when the trail's thread is gone (its store couldn't be opened, or it ended).
    fn ask<T>(&self, work: impl FnOnce(SyncSender<T>) -> Work) -> Option<T> {
        let (tx, rx) = std::sync::mpsc::sync_channel(1);
        self.tx.send(work(tx)).ok()?;
        rx.recv().ok()
    }

    /// perception.py:52.
    pub fn seen(&self, external_id: &str) -> bool {
        self.ask(|r| Work::Seen(external_id.to_string(), r))
            .unwrap_or(false)
    }

    /// perception.py:56 (I1).
    pub fn notification_floor(&self, observations: Vec<Observation>) -> Vec<Observation> {
        self.ask(|r| Work::Floor(observations, r))
            .unwrap_or_default()
    }

    /// perception.py:100: recorded on the trail's thread, in order with every other observation.
    pub fn notification(&self, o: Observation) {
        let _ = self.tx.send(Work::Notification(o));
    }

    /// perception.py:115.
    pub fn installed(&self) -> Vec<(String, String)> {
        self.ask(Work::Installed).unwrap_or_default()
    }

    /// perception.py:159 `status()`.
    pub fn status(&self) -> Result<Value, String> {
        self.ask(Work::Status)
            .unwrap_or_else(|| Err("perception isn't running".into()))
    }

    /// perception.py:70: FERB is posting a banner with this (redacted) title and body under `poster`.
    pub fn banner_posted(&self, title: &str, body: &str, poster: &str) -> Option<Banner> {
        self.ask(|r| Work::BannerPosted(title.into(), body.into(), poster.into(), r))
    }

    /// perception.py:77.
    pub fn banner_failed(&self, banner: Banner) {
        let _ = self.tx.send(Work::BannerFailed(banner));
    }

    /// perception.py:48 (I3).
    pub fn clipboard_access(&self, state: &str) {
        let _ = self.tx.send(Work::ClipboardAccess(state.to_string()));
    }

    /// perception.py:131 (D14/P8).
    pub fn purge_text(&self) {
        let _ = self.tx.send(Work::PurgeText);
    }

    /// retention.py:350 `_perception_counts`: the batcher's wakes and its batches by flush reason since start (none
    /// while Jev is off: SENSE-3).
    pub fn counts(&self) -> (i64, BTreeMap<String, i64>) {
        (self.wakes.load(Ordering::SeqCst) as i64, BTreeMap::new())
    }
}

/// A banner as the notifier keeps it between posting and a failure (`perception.banner_posted`'s dict).
pub fn banner_json(b: &Banner) -> Value {
    json!({"title": b.title, "body": b.body, "poster": b.poster, "at": b.at})
}

/// [`banner_json`] read back.
pub fn banner_of(v: &Value) -> Option<Banner> {
    let s = |k: &str| v.get(k).and_then(Value::as_str).map(str::to_string);
    Some(Banner {
        title: s("title")?,
        body: s("body")?,
        poster: s("poster")?,
        at: s("at")?,
    })
}

/// The trail on a thread of its own (its state is not shared across threads), with perception beside it:
/// observations and perception's calls come in through one channel and are handled in order, as Python's
/// `Trail.record` and `Perception` run on the loop. The trail's last event is kept for the time header (`signals`).
fn trail_thread(opts: &Options, s: &Shared, host: &Host) -> (TrailHandle, Perception) {
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<Work>();
    let last_event: Arc<Mutex<Option<ferb_core::when::DateTime>>> = Arc::new(Mutex::new(None));
    let home = s.home.clone();
    let clock = opts.clock.clone();
    let ids = opts.ids.clone();
    let redactor = s.redactor.clone();
    let config = s.config.clone();
    let events = s.events.clone();
    let journal = s.journal.clone();
    let host = host.clone();
    let attribution = s.tools.attribution();
    let paused_marker = s.home.join("PAUSED");
    let last = last_event.clone();
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
            // GATE-550 (trail.py:157-158): a test run's own activity isn't another run's context; a failed check
            // records nothing, as Python's raise did
            let runs = ferb_orchestrator::test_runs::TestRuns::new(&db);
            let marker = paused_marker.clone();
            let ev = events.clone();
            let seams = ferb_sense::trail::Seams {
                // `_exclusions`: the [privacy] lists as they are now (the config is read again when it changes)
                exclusions: Box::new(move || {
                    ferb_daemon::areas_b::sensing::exclusions(&host)
                        .unwrap_or_else(|_| ferb_sense::exclusions::Exclusions::defaults())
                }),
                publish: Box::new(move |topic, data| {
                    ev.publish(topic, crate::basics::object(data))
                }),
                device_id: Box::new(move || device.clone()),
                paused: Box::new(move || marker.exists()),
                test_task: Box::new(|task| runs.task(task).unwrap_or(true)),
                is_browser: Box::new(ferb_mac_hands::apps::known),
            };
            let trail = ferb_sense::trail::Trail::new(
                &db,
                clock.as_ref(),
                &ids,
                redactor.as_ref(),
                seams,
                attribution,
            );
            let plain: Arc<dyn ferb_core::interfaces::Clock> = clock.clone();
            let activity = ferb_sense::daily::DailyActivity::new(
                home.join("activity").join("today.jsonl"),
                {
                    let c = clock.clone();
                    move || c.now()
                },
                clock.zone(),
            );
            let (j, marker) = (journal.clone(), paused_marker.clone());
            let perception = ferb_sense::perception::Perception::new(
                &db,
                plain.as_ref(),
                &trail,
                ferb_sense::notifications::NotificationPolicy::new(
                    &db,
                    plain.as_ref(),
                    &ids,
                    redactor.as_ref(),
                ),
                ferb_sense::perception::Seams {
                    journal: Box::new(move |kind, data| j.note(kind, data)),
                    paused: Box::new(move || marker.exists()),
                    roll_over: Box::new(|| activity.roll_over()),
                    expire_derived: Box::new(|refs, replacement, now| {
                        let refs: Vec<String> = refs.iter().cloned().collect();
                        let _ = ferb_orchestrator::attention::expire_derived(
                            &db,
                            &refs,
                            replacement,
                            now,
                        );
                    }),
                },
            );
            // the batcher (Jev's, switched off: SENSE-2, SENSE-3), for its unsent count in `status`
            let cfg = config.clone();
            let mut batcher = ferb_sense::batches::EventBatcher::new(
                &db,
                plain.as_ref(),
                &ids,
                // decision: SENSE-3: Jev is off, so the batcher has no Jev
                crate::off::off("perception.jev", || None),
                ferb_sense::notifications::NotificationPolicy::new(
                    &db,
                    plain.as_ref(),
                    &ids,
                    redactor.as_ref(),
                ),
                |topic: &str, data: Value| events.publish(topic, crate::basics::object(data)),
            );
            batcher.oldest_seconds = Box::new(move || match cfg.get("sense", "batch_age_s") {
                Ok(Some(v)) => v.as_f64().unwrap_or(ferb_sense::batches::OLDEST_SECONDS),
                _ => ferb_sense::batches::OLDEST_SECONDS,
            });
            let purge = |perception: &ferb_sense::perception::Perception<'_, '_>| {
                if let Err(e) = perception.purge_text() {
                    journal.note(
                        "retention.failed",
                        json!({"error": redactor.text(&format!("OperationalError: {e}"))}),
                    );
                }
            };
            // sensing.py:68: typed and copied text past its 7 days expires at start too
            purge(&perception);
            while let Some(work) = rx.blocking_recv() {
                match work {
                    Work::Record(o) => {
                        let _ = trail.record(&o);
                        *crate::basics::lock(&last) = trail.last_event_at.get();
                    }
                    Work::Notification(o) => {
                        let _ = perception.notification(&o);
                        *crate::basics::lock(&last) = trail.last_event_at.get();
                    }
                    Work::Seen(id, reply) => {
                        let _ = reply.send(perception.seen(&id).unwrap_or(false));
                    }
                    Work::Floor(observations, reply) => {
                        let _ = reply.send(
                            perception
                                .notification_floor(observations)
                                .unwrap_or_default(),
                        );
                    }
                    Work::Installed(reply) => {
                        let _ = reply.send(perception.installed().unwrap_or_default());
                    }
                    Work::Status(reply) => {
                        let status = batcher
                            .pending_count()
                            .and_then(|n| perception.status(n, (batcher.oldest_seconds)()))
                            .map_err(|e| e.to_string());
                        let _ = reply.send(status);
                    }
                    Work::BannerPosted(title, body, poster, reply) => {
                        let _ = reply.send(perception.banner_posted(&title, &body, &poster));
                    }
                    Work::BannerFailed(b) => perception.banner_failed(&b),
                    Work::ClipboardAccess(state) => perception.clipboard_access(&state),
                    Work::PurgeText => purge(&perception),
                }
            }
        })
        .expect("the trail's thread");
    let record = tx.clone();
    let emit: Emit = Arc::new(move |o: Observation| {
        let _ = record.send(Work::Record(o));
    });
    let perception = Perception {
        tx,
        watch_problems: Arc::new(Mutex::new(Vec::new())),
        wakes: Arc::new(AtomicU64::new(0)),
    };
    (TrailHandle { emit, last_event }, perception)
}

/// `_build_sense` (areas/sensing.py:44-83): the attribution's view of FERB's running commands, the trail with
/// perception on their thread, and the watcher manager: perception's batcher always, and with the Mac's parts on
/// (`run_watchers`) the frontmost app, window titles, idleness, the watch folders, app installs, the browsers'
/// scripted tabs and perception's native watchers. Jev stays off (notices paused: SENSE-2).
pub fn sense(opts: &Options, s: &Shared) -> Sense {
    // sensing.py:51-53: the folders FERB's running commands work in (a file event there is FERB's)
    let attribution = s.tools.attribution();
    if let Some(shells) = s.tools.shells() {
        *crate::basics::lock(&attribution.shell_cwds) = Box::new(move || {
            shells
                .list()
                .into_iter()
                .filter(|p| p.running)
                .map(|p| (p.cwd, p.owner_task))
                .collect()
        });
    }
    let host = Host {
        home: s.home.clone(),
        config: s.config.clone(),
        redactor: s.redactor.clone(),
        journal: s.journal.clone(),
        bus: s.events.clone(),
        clock: opts.clock.clone(),
    };
    let (trail, perception) = trail_thread(opts, s, &host);
    let emit = trail.emit.clone();
    s.tools.set_trail(trail);
    s.tools.set_perception(perception.clone());
    let j2 = s.journal.clone();
    let h_tap = host.clone();
    let h_pause = host.clone();
    let parts = s.tools.downgrade();
    let events = s.events.clone();
    let hooks = Hooks {
        // `_on_pause` (sensing.py:127-131): the owner's tabs forgotten, journaled and published
        on_pause: Arc::new(move |paused| {
            let forget = || {
                if let Some(b) = parts.upgrade().as_ref().and_then(|p| p.browsers()) {
                    b.hub.forget_owner_tabs();
                }
            };
            let _ = ferb_daemon::areas_b::sensing::on_pause(&h_pause, paused, &forget);
        }),
        on_fail: Arc::new(move |source, why| {
            j2.note(
                "sense.watcher_failed",
                json!({"source": source, "error": why}),
            )
        }),
        // `_tap_changed` (SENSE-406): the owner-input tap went down (why) or came back up
        on_tap: Arc::new(move |why: &str| {
            let _ = ferb_daemon::areas_b::sensing::tap_changed(&h_tap, why);
        }),
        // manager.py:111 `self.trail.publish`: the daemon's `_publish`
        publish: Arc::new(move |topic: &str, data: Value| {
            events.publish(topic, crate::basics::object(data))
        }),
    };
    let mut watchers: Vec<Arc<dyn Watcher>> = vec![Arc::new(JevBatches {
        wakes: perception.wakes.clone(),
    })];
    if opts.native {
        watchers.extend(native_watchers(opts, s, &host, &perception));
    }
    // `watch_owner_input`: the owner-input tap and the Mac's presence start and stop with sensing
    let (tap, presence) = match s.tools.owner() {
        Some(o) => (
            Some(
                Arc::new(ferb_sense::watchers::owner_input::Tap(o.input.clone()))
                    as Arc<dyn ferb_sense::manager::OwnerInputTap>,
            ),
            Some(
                Arc::new(ferb_sense::watchers::presence::Presence(o.presence.clone()))
                    as Arc<dyn ferb_sense::manager::PresenceWatch>,
            ),
        ),
        None => (None, None),
    };
    Sense::new(s.home.clone(), watchers, emit, tap, presence, hooks)
}

/// perception.py:28 `run` (source `jev_batches`): the batcher's loop. Jev is off (SENSE-2, SENSE-3), so it wakes
/// once at its start and then only waits for the stop: no batcher work.
struct JevBatches {
    wakes: Arc<AtomicU64>,
}

impl Watcher for JevBatches {
    fn source(&self) -> &str {
        "jev_batches"
    }

    fn run(self: Arc<Self>, _emit: Emit, stop: Stop) -> BoxFuture<'static, Result<(), String>> {
        Box::pin(async move {
            // decision: SENSE-3
            self.wakes.fetch_add(1, Ordering::SeqCst);
            stop.wait().await;
            Ok(())
        })
    }
}

/// desktop.py:362 `_automation_granted`: Automation for an app the owner already granted; never asks (the prompt
/// would appear out of nowhere). An answer is kept 60 seconds; while a check is still running this answers false
/// (a stuck native call never holds the watcher).
#[derive(Default)]
pub struct AutomationGranted {
    cache: Mutex<HashMap<String, (Instant, bool)>>,
    checks: Mutex<HashMap<String, Arc<OnceLock<bool>>>>,
}

impl AutomationGranted {
    pub fn granted(&self, bundle_id: &str, check: impl FnOnce() -> bool + Send + 'static) -> bool {
        if let Some((at, ok)) = crate::basics::lock(&self.cache).get(bundle_id) {
            if at.elapsed() < Duration::from_secs(60) {
                return *ok;
            }
        }
        let probe = crate::basics::lock(&self.checks)
            .entry(bundle_id.to_string())
            .or_insert_with(|| {
                let cell = Arc::new(OnceLock::new());
                let answer = cell.clone();
                let _ = std::thread::Builder::new()
                    .name("ferb-automation-check".into())
                    .spawn(move || {
                        let _ = answer.set(check());
                    });
                cell
            })
            .clone();
        let Some(ok) = probe.get().copied() else {
            return false;
        };
        crate::basics::lock(&self.checks).remove(bundle_id);
        crate::basics::lock(&self.cache).insert(bundle_id.to_string(), (Instant::now(), ok));
        ok
    }
}

/// The terminal rules text capture shares with the typing floor (text_input.py:27-43, D31): an app is a terminal by
/// its bundle id (built in or the owner's `[protection] terminal_apps`), its Info.plist's Shell role or a terminal
/// panel it can't show; a field is inside one by its own and its ancestors' facts, never its value.
struct CaptureTerminals {
    config: Arc<ferb_core::config::Config>,
    apps: ferb_mac_hands::native::apps::NativeApps,
}

impl CaptureTerminals {
    fn owner_apps(&self) -> Vec<String> {
        match self.config.get("protection", "terminal_apps") {
            Ok(Some(Value::Array(a))) => a
                .iter()
                .filter_map(Value::as_str)
                .map(String::from)
                .collect(),
            _ => ferb_policy::protection::DEFAULT_TERMINAL_APPS
                .iter()
                .map(|s| s.to_string())
                .collect(),
        }
    }
}

impl ferb_sense::watchers::text_input::Terminals for CaptureTerminals {
    fn app(&self, app: &ferb_sense::watchers::base::Focused) -> bool {
        use ferb_mac_hands::driver::Apps;
        let shell = self.apps.declares_shell(&app.bundle_id);
        ferb_policy::terminal::surface(&app.bundle_id, &[], &self.owner_apps(), shell, false)
            .is_some()
    }
    fn field(
        &self,
        app: &ferb_sense::watchers::base::Focused,
        facts: &[Map<String, Value>],
    ) -> bool {
        ferb_policy::terminal::surface(&app.bundle_id, facts, &self.owner_apps(), false, false)
            .is_some()
    }
}

/// `excluded(bundle_id, app_name)`: an app on the owner's exclusion list.
type Excluded = Arc<dyn Fn(&str, &str) -> bool + Send + Sync>;

/// The watchers `_build_sense` starts with `run_watchers` (sensing.py:71-77) and perception's native watchers
/// (perception.py:31-45).
fn native_watchers(
    opts: &Options,
    s: &Shared,
    host: &Host,
    perception: &Perception,
) -> Vec<Arc<dyn Watcher>> {
    use ferb_sense::mac::streams::{AccessibilityWatcher, MacFsEvents, WorkspaceWatcher};
    use ferb_sense::watchers::{
        app_focus::AppFocusWatcher, app_installs::AppInstallsWatcher,
        browser_tabs::ScriptedTabsWatcher, clipboard::ClipboardWatcher, files::FilesWatcher,
        idle::IdleWatcher, notifications::NotificationWatcher, software::SoftwareWatcher,
        text_input::TextInputWatcher, windows::WindowTitleWatcher,
    };
    let config = &s.config;
    let clock = &opts.clock;
    let user_home = opts.user_home.to_string_lossy().into_owned();
    let redact: Arc<dyn ferb_sense::observation::Redact> = s.redactor.clone();
    let monotonic: Arc<dyn Fn() -> f64 + Send + Sync> = {
        let c = clock.clone();
        Arc::new(move || c.monotonic())
    };
    let focused: ferb_sense::watchers::base::ReadFocused =
        Arc::new(ferb_sense::mac::apps::focused_app);
    // `d._exclusions().app(bundle, name)`: the owner's [privacy] lists as they are now
    let excluded: Excluded = {
        let h = host.clone();
        Arc::new(move |bundle: &str, name: &str| {
            ferb_daemon::areas_b::sensing::exclusions(&h)
                .unwrap_or_else(|_| ferb_sense::exclusions::Exclusions::defaults())
                .app(Some(bundle), Some(name))
        })
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
    let mut out: Vec<Arc<dyn Watcher>> = vec![
        // app_focus.py:15: `_build_sense` leaves its `on_change` at the default, which does nothing
        Arc::new(AppFocusWatcher::new(
            focused.clone(),
            crate::off::off("sense.app_focus_on_change", Arc::new(|_| {})),
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
    ];
    // idle.py: the owner-input tap's last owner event and whether it runs (sensing.py:73)
    if let Some(o) = s.tools.owner() {
        let (last, running) = (o.input.clone(), o.input.clone());
        let c = clock.clone();
        out.push(Arc::new(IdleWatcher::new(
            Arc::new(move || last.last_owner_wall()),
            Arc::new(move || running.is_running()),
            idle_after,
            Arc::new(move || c.now().timestamp_micros() as f64 / 1e6),
        )));
    }
    // files.py: the `[sense] watch_folders` that may be watched, each skipped one journaled and kept for status
    {
        let (h, problems, home) = (
            host.clone(),
            perception.watch_problems.clone(),
            user_home.clone(),
        );
        out.push(Arc::new(FilesWatcher::new(
            Arc::new(move || {
                ferb_daemon::areas_b::sensing::watch_folders(&h, &home, &problems)
                    .unwrap_or_default()
            }),
            Arc::new(MacFsEvents::default()),
            monotonic.clone(),
        )));
    }
    // app_installs.py: each change read from that app's Info.plist; desktop.py `_app_changed` off the stream's
    // thread updates the device profile and tells clients (`device.changed`)
    {
        let h = host.clone();
        out.push(Arc::new(AppInstallsWatcher::new(
            Arc::new(move |change: &str, info, path: &str| {
                let info = info.map(|i: &ferb_sense::watchers::app_installs::AppInfo| {
                    ferb_device::profile::AppInfo::new(
                        &i.bundle_id,
                        &i.name,
                        &i.version,
                        &i.path,
                        i.scriptable,
                    )
                });
                let (h, change, path) = (h.clone(), change.to_string(), path.to_string());
                let _ = std::thread::Builder::new()
                    .name("ferb-device-profile".into())
                    .spawn(move || {
                        let _ = ferb_daemon::areas_b::desktop::app_changed(
                            &h,
                            &change,
                            info.as_ref(),
                            &path,
                        );
                    });
            }),
            &ferb_sense::watchers::app_installs::app_folders(&user_home),
            Arc::new(MacFsEvents::default()),
            Arc::new(|path: &str| {
                ferb_device::scan::read_app(std::path::Path::new(path)).map(|a| {
                    ferb_sense::watchers::app_installs::AppInfo {
                        bundle_id: a.bundle_id,
                        name: a.name,
                        version: a.version,
                        path: a.path,
                        scriptable: a.scriptable,
                    }
                })
            }),
        )));
    }
    // browser_tabs.py: the AppleScript read of the front tab, for browsers the extension doesn't cover; only when
    // the owner already granted Automation for that browser
    {
        let automation = Arc::new(AutomationGranted::default());
        let parts = s.tools.downgrade();
        out.push(Arc::new(ScriptedTabsWatcher::new(
            Arc::new(move |bundle: &str| {
                let b = bundle.to_string();
                automation.granted(bundle, move || {
                    ferb_mac_hands::native::permissions::automation(&b, false)
                        == ferb_mac_hands::permissions::Status::Granted
                })
            }),
            // sensing.py:136 `_any_extension_connected`
            Arc::new(move |browser: &str| {
                parts
                    .upgrade()
                    .as_ref()
                    .and_then(|p| p.browsers())
                    .is_some_and(|b| {
                        b.hub
                            .bridges()
                            .iter()
                            .any(|br| br.is_connected() && br.binding.browser == browser)
                    })
            }),
            focused.clone(),
            Arc::new(ferb_sense::mac::system::front_tab),
        )));
    }
    // perception.py:31-45: its native watchers
    out.push(Arc::new(WorkspaceWatcher {
        redact: redact.clone(),
        excluded: excluded.clone(),
    }));
    out.push(Arc::new(AccessibilityWatcher {
        redact: redact.clone(),
        excluded: excluded.clone(),
        is_browser: Arc::new(ferb_mac_hands::apps::known),
    }));
    {
        let p = perception.clone();
        out.push(Arc::new(ClipboardWatcher::new(
            redact.clone(),
            excluded.clone(),
            Arc::new(move |state: &str| p.clipboard_access(state)),
            Arc::new(ferb_sense::mac::system::GeneralPasteboard),
            focused.clone(),
        )));
    }
    if let Some(o) = s.tools.owner() {
        out.push(Arc::new(TextInputWatcher::new(
            o.input.clone(),
            redact.clone(),
            excluded.clone(),
            Arc::new(CaptureTerminals {
                config: config.clone(),
                apps: ferb_mac_hands::native::apps::NativeApps::new(Arc::new(
                    ferb_mac_hands::apps::WebHandlers::default(),
                )),
            }),
            Arc::new(ferb_sense::mac::apps::MacFieldReads),
            focused.clone(),
            monotonic.clone(),
        )));
    }
    {
        let (seen, record, floor) = (perception.clone(), perception.clone(), perception.clone());
        let native = opts.native;
        let notification_center = NotificationWatcher::new(
            Arc::new(
                ferb_sense::watchers::notifications::NotificationDatabase::new(
                    ferb_sense::watchers::notifications::database(&user_home).into(),
                    redact.clone(),
                    excluded.clone(),
                ),
            ),
            Arc::new(move |id: &str| seen.seen(id)),
            Arc::new(move |o: Observation| record.notification(o)),
            Arc::new(move |observations| floor.notification_floor(observations)),
            // `d.data_access`: Full Disk Access (SENSE-405: nothing is read before it)
            Arc::new(move || {
                native
                    && ferb_mac_hands::native::permissions::full_disk_access()
                        == ferb_mac_hands::permissions::Status::Granted
            }),
        );
        out.push(Arc::new(notification_center));
    }
    {
        let p = perception.clone();
        out.push(Arc::new(SoftwareWatcher::new(
            Arc::new(move || p.installed()),
            redact,
            Arc::new(ferb_sense::mac::system::MacSoftwareReads {
                home: opts.user_home.clone(),
            }),
        )));
    }
    out
}
