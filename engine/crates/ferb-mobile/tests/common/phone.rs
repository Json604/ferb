//! The app's seams for tests: a fixed clock, counted ids, a job queue run by hand, a listener that records.
#![allow(dead_code)]

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use ferb_mobile::{Change, Clock, Config, Env, Ids, Listener, Mobile, Spawner};
use tempfile::TempDir;

pub struct FixedClock;
impl Clock for FixedClock {
    fn now_iso(&self) -> String {
        "2026-01-02T10:00:00+00:00".into()
    }
}

#[derive(Default)]
pub struct Counted(AtomicU64);
impl Ids for Counted {
    fn new_id(&self) -> String {
        format!("sub-{}", self.0.fetch_add(1, Ordering::SeqCst) + 1)
    }
}

/// Jobs wait until the test runs them, so nothing races.
#[derive(Default)]
pub struct Jobs(pub Mutex<Vec<Box<dyn FnOnce() + Send>>>);
impl Spawner for Jobs {
    fn spawn(&self, job: Box<dyn FnOnce() + Send>) {
        self.0.lock().unwrap().push(job);
    }
}
impl Jobs {
    pub fn pending(&self) -> usize {
        self.0.lock().unwrap().len()
    }
    /// Runs every job, and the jobs those start.
    pub fn run(&self) -> usize {
        let mut n = 0;
        loop {
            let jobs: Vec<_> = std::mem::take(&mut *self.0.lock().unwrap());
            if jobs.is_empty() {
                return n;
            }
            for j in jobs {
                j();
                n += 1;
            }
        }
    }
}

#[derive(Default)]
pub struct Heard(pub Mutex<Vec<Change>>);
impl Listener for Heard {
    fn changed(&self, change: Change) {
        self.0.lock().unwrap().push(change);
    }
}

pub const KEY: [u8; 32] = [7u8; 32];

pub struct Phone {
    pub dir: TempDir,
    pub jobs: Arc<Jobs>,
    pub heard: Arc<Heard>,
    pub env: Env,
    pub config: Config,
}

impl Phone {
    pub fn new() -> Phone {
        let jobs = Arc::new(Jobs::default());
        let heard = Arc::new(Heard::default());
        let env = Env {
            clock: Arc::new(FixedClock),
            ids: Arc::new(Counted::default()),
            spawner: jobs.clone(),
            listener: heard.clone(),
        };
        Phone {
            dir: tempfile::tempdir().unwrap(),
            jobs,
            heard,
            env,
            config: Config::new("phone-1"),
        }
    }

    pub fn path(&self) -> std::path::PathBuf {
        self.dir.path().join("ferb-phone.db")
    }

    pub fn open(&self) -> Mobile {
        Mobile::open(
            self.path(),
            KEY.to_vec(),
            self.config.clone(),
            self.env.clone(),
        )
        .unwrap()
    }
}

pub fn online(tier: &str) -> String {
    format!(r#"{{"type":"state","phase":"online","tier":"{tier}"}}"#)
}

pub const OFFLINE: &str = r#"{"type":"state","phase":"dialing"}"#;
