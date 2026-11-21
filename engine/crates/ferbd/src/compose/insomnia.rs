//! Insomnia mode (kb T-1274, APP-980..APP-986, HAND-645) in this program: ferb-daemon's `insomnia` over the Mac's power
//! assertions and facts (`ferb_platform_macos::power`, only with the Mac's own parts on; a test daemon holds nothing
//! and reads nothing), the store's running tasks, the presence watcher's unlock, the journal (the mode kept across a
//! restart), the bus and the owner's notices. Its seams reach the gateway (the one Yes, APP-984), the phone gate
//! (`chat.status`), and the Mac hands (the cover a foreground step passes, the apps FERB brings forward).

use std::sync::Arc;

use ferb_core::interfaces::{Clock, IdSource};
use ferb_core::power::{
    HelperState, Hold, KeepAwake, LockMac, PowerFacts, PowerReading, RootHelper,
};
use ferb_core::when::{self, DateTime};
use ferb_daemon::insomnia::{Insomnia, Persist, Saved, Seams, JOURNAL_KIND};
use ferb_rpc::Server;
use serde_json::Value;

use crate::basics::{lock, Events, Journal, SharedDb};

/// What the area takes from the composition.
pub struct Parts {
    pub db: SharedDb,
    pub journal: Arc<Journal>,
    pub events: Arc<Events>,
    pub clock: Arc<dyn Clock>,
    pub ids: Arc<dyn IdSource>,
    pub config: Arc<ferb_core::config::Config>,
    pub notifier: Arc<ferb_daemon::areas_b::notify::Notifier>,
    /// The Mac's own parts: the real power assertions, facts and lock. Off (a test daemon): none.
    pub native: bool,
}

/// A daemon without the Mac's own parts holds nothing and reads nothing (TEST-508).
struct Absent;

impl KeepAwake for Absent {
    fn hold(&self, _hold: Hold, _reason: &str, _lease_s: u64) -> Result<(), String> {
        Ok(())
    }
    fn release(&self, _hold: Hold) {}
}

impl PowerFacts for Absent {
    fn read(&self) -> PowerReading {
        PowerReading::default()
    }
}

impl RootHelper for Absent {
    fn state(&self) -> HelperState {
        HelperState::Absent
    }
    fn lease(&self, _seconds: u64) -> Result<(), String> {
        Err("no helper in this FERB".into())
    }
    fn release(&self) -> Result<(), String> {
        Ok(())
    }
    fn wakes(&self) -> Result<Vec<i64>, String> {
        Err("no helper in this FERB".into())
    }
    fn wake(&self, _at: i64) -> Result<(), String> {
        Err("no helper in this FERB".into())
    }
    fn unwake(&self, _at: i64) -> Result<(), String> {
        Err("no helper in this FERB".into())
    }
}

impl LockMac for Absent {
    fn lock_now(&self) -> Result<bool, String> {
        Err("this FERB runs without the Mac's own parts".into())
    }
}

/// The journal keeps the mode across a restart: its last `insomnia.state` row says whether one was on.
struct JournalPersist(Arc<Journal>);

impl Persist for JournalPersist {
    fn load(&self) -> Option<Saved> {
        let rows = self.0.of_kind(JOURNAL_KIND, "", "");
        let (_, last) = rows.last()?;
        match last.get("state").and_then(Value::as_str) {
            Some("started" | "resumed") => Saved::from_json(last),
            _ => None,
        }
    }
    fn note(&self, kind: &str, data: Value) {
        self.0.note(kind, data);
    }
}

/// Root tasks running now (each tree once).
fn working(db: &SharedDb) -> usize {
    let d = lock(db);
    d.conn()
        .query_row(
            "SELECT COUNT(DISTINCT tree_id) FROM tasks WHERE status = 'running'",
            [],
            |r| r.get::<_, i64>(0),
        )
        .map(|n| n.max(0) as usize)
        .unwrap_or(0)
}

/// The presence watcher's last unlock (`mac.back_in_use`, signal `unlock`) came after `since`: the owner is back.
fn unlocked_after(journal: &Journal, since: &DateTime) -> bool {
    let rows = journal.of_kind("mac.back_in_use", "signal", "unlock");
    rows.last()
        .and_then(|(at, _)| when::parse(at))
        .is_some_and(|at| at > *since)
}

/// Builds the area, registers `insomnia.*` on `server` and resumes a mode that was on when FERB stopped.
pub fn build(server: &Server, p: Parts) -> Insomnia {
    let (keep, facts, lock_mac): (Arc<dyn KeepAwake>, Arc<dyn PowerFacts>, Arc<dyn LockMac>) =
        native_power(p.native);
    let root = native_root(p.native);
    let db = p.db.clone();
    let j = p.journal.clone();
    let config = p.config.clone();
    let events = p.events.clone();
    let notifier = p.notifier.clone();
    let clock = p.clock.clone();
    let clock2 = p.clock.clone();
    let ids = p.ids.clone();
    // the daemon's runtime, for a notice told from the power watch's own thread (DMN-614)
    let runtime = tokio::runtime::Handle::try_current().ok();
    let seams = Seams {
        keep,
        root,
        facts,
        lock: lock_mac,
        working: Arc::new(move || working(&db)),
        unlocked_after: Arc::new(move |since: &DateTime| unlocked_after(&j, since)),
        settings: Arc::new(move || ferb_daemon::areas_a::settings::insomnia(&config)),
        persist: Arc::new(JournalPersist(p.journal.clone())),
        publish: Arc::new(move |topic: &str, data: Value| {
            events.publish(topic, crate::basics::object(data))
        }),
        tell: Arc::new(move |ident: &str, text: &str| {
            let (n, ident, text) = (notifier.clone(), ident.to_owned(), text.to_owned());
            // the owner's notice, once per ident; off the caller's thread (a look runs anywhere)
            if let Some(rt) = tokio::runtime::Handle::try_current()
                .ok()
                .or_else(|| runtime.clone())
            {
                rt.spawn(async move {
                    let _ = n
                        .tell_owner(
                            &ident,
                            &[],
                            &text,
                            ferb_sanitize::origin::FERB,
                            "alert",
                            "",
                            &[],
                            &Value::Null,
                            None,
                        )
                        .await;
                });
            }
        }),
        now: Arc::new(move || clock.now()),
        mono: Arc::new(move || clock2.monotonic()),
        new_id: Arc::new(move || ids.new_id()),
    };
    let ins = Insomnia::new(seams);
    ins.register(server);
    ins
}

#[cfg(target_os = "macos")]
fn native_power(native: bool) -> (Arc<dyn KeepAwake>, Arc<dyn PowerFacts>, Arc<dyn LockMac>) {
    use ferb_platform_macos::power::{Assertions, DisplaySleepLock, Facts};
    if native {
        return (
            Arc::new(Assertions::new()),
            Arc::new(Facts),
            Arc::new(DisplaySleepLock),
        );
    }
    (Arc::new(Absent), Arc::new(Absent), Arc::new(Absent))
}

#[cfg(target_os = "macos")]
fn native_watch(handler: ferb_core::power::PowerHandler) -> bool {
    use ferb_core::power::PowerWatch;
    ferb_platform_macos::power::Watch.start(handler)
}

#[cfg(not(target_os = "macos"))]
fn native_watch(_handler: ferb_core::power::PowerHandler) -> bool {
    false
}

/// APP-986: FERB's root helper (its socket answers only when the owner approved it and it runs); a test daemon has none.
#[cfg(target_os = "macos")]
fn native_root(native: bool) -> Arc<dyn RootHelper> {
    if native {
        return Arc::new(ferb_platform_macos::power::HelperClient::system());
    }
    Arc::new(Absent)
}

#[cfg(not(target_os = "macos"))]
fn native_root(_native: bool) -> Arc<dyn RootHelper> {
    Arc::new(Absent)
}

/// The next runs of the owner's active schedules (POSIX seconds), soonest first.
fn next_runs(db: &SharedDb) -> Vec<i64> {
    let d = lock(db);
    let Ok(mut st) = d.conn().prepare(
        "SELECT next_run FROM schedules WHERE state = 'active' AND next_run IS NOT NULL ORDER BY next_run",
    ) else {
        return Vec::new();
    };
    let rows = st.query_map([], |r| r.get::<_, String>(0));
    rows.map(|r| {
        r.flatten()
            .filter_map(|t| when::parse(&t))
            .map(|t| t.timestamp())
            .collect()
    })
    .unwrap_or_default()
}

#[cfg(not(target_os = "macos"))]
fn native_power(_native: bool) -> (Arc<dyn KeepAwake>, Arc<dyn PowerFacts>, Arc<dyn LockMac>) {
    (Arc::new(Absent), Arc::new(Absent), Arc::new(Absent))
}

/// APP-984: the Mac hands' foreground step passes insomnia mode's cover (none before the area is built).
pub struct Cover(pub super::tools::WeakParts);

impl ferb_mac_hands::driver::ForegroundCover for Cover {
    fn begin<'a>(
        &'a self,
        pid: i32,
        bundle: &'a str,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<Option<String>, String>> + Send + 'a>,
    > {
        let ins = self.0.upgrade().and_then(|p| p.insomnia());
        Box::pin(async move {
            match ins {
                Some(i) => i.step_begin(pid, bundle).await,
                None => Ok(None),
            }
        })
    }

    fn end(&self, step: &str) {
        if let Some(i) = self.0.upgrade().and_then(|p| p.insomnia()) {
            i.step_end(step);
        }
    }
}

/// The insomnia area's loops: a look every `TICK_S` (the lease renewed, the facts, the grace, the warnings) and the
/// camera's watchdog every second.
pub fn start_loops(d: &Arc<super::Daemon>) {
    let Some(ins) = d.tools.insomnia() else {
        return;
    };
    ins.resume();
    // DMN-613: the power source, the lid and sleep are told at once, not at the next look (the Mac's own parts only)
    if d.opts.native {
        let i = ins.clone();
        let watching = native_watch(Arc::new(move |ev| i.power_event(ev)));
        if !watching {
            d.journal
                .note("insomnia.watch_failed", serde_json::json!({}));
        }
    }
    let i = ins.clone();
    d.spawn_loop("insomnia", move |_| {
        let i = i.clone();
        async move {
            loop {
                let j = i.clone();
                let _ = tokio::task::spawn_blocking(move || j.look()).await;
                tokio::time::sleep(std::time::Duration::from_secs(
                    ferb_daemon::insomnia::TICK_S,
                ))
                .await;
            }
        }
    });
    // APP-986: FERB's own wakes follow the owner's schedules (through the root helper, when approved), every 5 minutes
    if d.opts.native {
        let (db, j) = (d.db.clone(), d.journal.clone());
        let root = native_root(true);
        d.spawn_loop("schedule_wakes", move |_| {
            let (db, j, root) = (db.clone(), j.clone(), root.clone());
            async move {
                let mut said_absent = false;
                loop {
                    let (db2, root2) = (db.clone(), root.clone());
                    let now = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|d| d.as_secs() as i64)
                        .unwrap_or(0);
                    let r = tokio::task::spawn_blocking(move || {
                        ferb_daemon::insomnia::sync_wakes(root2.as_ref(), &next_runs(&db2), now)
                    })
                    .await
                    .unwrap_or_else(|e| Err(e.to_string()));
                    match r {
                        Ok((0, 0)) => {}
                        Ok((added, cancelled)) => j.note(
                            "insomnia.wakes",
                            serde_json::json!({"scheduled": added, "cancelled": cancelled}),
                        ),
                        Err(e) if !said_absent => {
                            said_absent = true;
                            j.note(
                                "insomnia.wakes_unavailable",
                                serde_json::json!({"error": e}),
                            );
                        }
                        Err(_) => {}
                    }
                    tokio::time::sleep(std::time::Duration::from_secs(300)).await;
                }
            }
        });
    }
    d.spawn_loop("insomnia_camera", move |_| {
        let i = ins.clone();
        async move {
            loop {
                tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                i.camera_check();
            }
        }
    });
}
