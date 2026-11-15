//! Ordered resource locks and the GUI lock (ferb/act/locks.py, ferb/act/gui_lock.py). A capability names the
//! resources a call touches (`locks(args)`); the gateway takes them after policy allowed the call, in one global
//! order (kind, then name), so two workers never hold them crosswise. Never held across model calls or approval
//! waits. Must not choose resources or authorize actions.

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};

use tokio::sync::oneshot;

pub const KIND_ORDER: [(&str, u8); 5] = [
    ("account", 0),
    ("app", 1),
    ("file", 2),
    ("tab", 3),
    ("recording", 4),
];

fn kind_rank(key: &str) -> Option<u8> {
    let kind = key.split(':').next().unwrap_or("");
    KIND_ORDER.iter().find(|k| k.0 == kind).map(|k| k.1)
}

/// locks.py:17: 'kind:name' keys, deduplicated, in the one global order.
pub fn ordered(keys: &[String]) -> Result<Vec<String>, String> {
    let mut uniq: Vec<String> = vec![];
    for k in keys {
        if !uniq.contains(k) {
            uniq.push(k.clone());
        }
    }
    for k in &uniq {
        if kind_rank(k).is_none() {
            return Err(format!("unknown lock kind in {}", ferb_py_repr(k)));
        }
    }
    uniq.sort_by(|a, b| (kind_rank(a), a).cmp(&(kind_rank(b), b)));
    Ok(uniq)
}

/// Python's repr of a str (single quotes unless the text holds one and no double quote).
pub fn ferb_py_repr(s: &str) -> String {
    ferb_sanitize::parity::repr(&ferb_sanitize::Py::Str(ferb_sanitize::PyStr::new(s)))
}

#[derive(Default)]
struct Slot {
    held: bool,
    holder: Option<String>,
    reading: bool,
    waiters: VecDeque<oneshot::Sender<()>>,
}

/// locks.py:32.
#[derive(Clone, Default)]
pub struct ResourceLocks {
    slots: Arc<Mutex<HashMap<String, Slot>>>,
}

/// The keys a `hold` or `try_hold` took; released on drop.
pub struct Held {
    locks: ResourceLocks,
    taken: Vec<String>,
}

impl Drop for Held {
    fn drop(&mut self) {
        let mut slots = self.locks.slots.lock().expect("lock");
        for k in self.taken.iter().rev() {
            if let Some(s) = slots.get_mut(k) {
                s.holder = None;
                s.reading = false;
                loop {
                    match s.waiters.pop_front() {
                        // hand the lock straight to the next live waiter (asyncio.Lock's FIFO wake)
                        Some(w) => {
                            if w.send(()).is_ok() {
                                break;
                            }
                        }
                        None => {
                            s.held = false;
                            break;
                        }
                    }
                }
            }
        }
    }
}

impl ResourceLocks {
    pub fn new() -> Self {
        Self::default()
    }

    /// locks.py:38: every key, in order, waiting for each. `reading`: the call only reads what they name.
    pub async fn hold(&self, keys: &[String], holder: &str, reading: bool) -> Result<Held, String> {
        let order = ordered(keys)?;
        let mut held = Held {
            locks: self.clone(),
            taken: vec![],
        };
        for k in order {
            let wait = {
                let mut slots = self.slots.lock().expect("lock");
                let s = slots.entry(k.clone()).or_default();
                if !s.held {
                    s.held = true;
                    None
                } else {
                    let (tx, rx) = oneshot::channel();
                    s.waiters.push_back(tx);
                    Some(rx)
                }
            };
            if let Some(rx) = wait {
                // a dropped waiter's sender fails, and the release passes the lock on
                if rx.await.is_err() {
                    return Err("lock released without a hand-over".into());
                }
            }
            let mut slots = self.slots.lock().expect("lock");
            let s = slots.get_mut(&k).expect("slot");
            s.holder = Some(holder.into());
            if reading {
                s.reading = true;
            }
            held.taken.push(k);
        }
        Ok(held)
    }

    /// locks.py:57: every lock only if each is free and nobody waits for it (or, `during_reads`, held by a call that
    /// only reads, with nobody waiting), else none: None at once. A bystander never queues ahead of a worker.
    pub fn try_hold(
        &self,
        keys: &[String],
        holder: &str,
        during_reads: bool,
    ) -> Result<Option<Held>, String> {
        let order = ordered(keys)?;
        let mut slots = self.slots.lock().expect("lock");
        for k in &order {
            let s = slots.entry(k.clone()).or_default();
            let waited = s.waiters.iter().any(|w| !w.is_closed());
            if waited || (s.held && !(during_reads && s.reading)) {
                return Ok(None);
            }
        }
        let mut taken = vec![];
        for k in order {
            let s = slots.get_mut(&k).expect("slot");
            if s.held {
                continue;
            }
            s.held = true;
            s.holder = Some(holder.into());
            taken.push(k);
        }
        drop(slots);
        Ok(Some(Held {
            locks: self.clone(),
            taken,
        }))
    }

    pub fn holder(&self, key: &str) -> Option<String> {
        self.slots
            .lock()
            .expect("lock")
            .get(key)
            .and_then(|s| s.holder.clone())
    }
}

/// gui_lock.py: one GUI actor at a time for its observe -> act -> verify operation.
#[derive(Clone, Default)]
pub struct GuiLock {
    lock: Arc<tokio::sync::Mutex<()>>,
    holder: Arc<Mutex<Option<String>>>,
}

pub struct GuiHeld {
    _guard: tokio::sync::OwnedMutexGuard<()>,
    holder: Arc<Mutex<Option<String>>>,
}

impl Drop for GuiHeld {
    fn drop(&mut self) {
        *self.holder.lock().expect("lock") = None;
    }
}

impl GuiLock {
    pub fn new() -> Self {
        Self::default()
    }
    pub async fn hold(&self, holder: &str) -> GuiHeld {
        let guard = self.lock.clone().lock_owned().await;
        *self.holder.lock().expect("lock") = Some(holder.into());
        GuiHeld {
            _guard: guard,
            holder: self.holder.clone(),
        }
    }
    pub fn holder(&self) -> Option<String> {
        self.holder.lock().expect("lock").clone()
    }
}
