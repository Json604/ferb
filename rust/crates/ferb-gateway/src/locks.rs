//! Ordered resource locks for tabs, accounts, files, apps and recordings (`ferb/act/locks.py`). Must not choose
//! resources or authorize actions.
//!
//! A tool says which resources one call touches (`locks(args)`); the gateway takes them after policy allowed the
//! call and releases them when the call returns: held across observe, act and verify inside one call, never across
//! model calls or the owner's answers. Several locks are always taken in one fixed order (kind, then name), so two
//! workers never hold them crosswise. A bystander (the live preview, a payment's picture) takes them only when free
//! and nobody waits (`try_hold`, DMN-403/406). In memory: nothing outlives the process.

use std::collections::{BTreeSet, HashMap};
use std::sync::{Arc, Mutex};

use tokio::sync::{Mutex as AsyncMutex, OwnedMutexGuard};

/// KIND_ORDER (`locks.py:14`).
pub const KIND_ORDER: &[(&str, u8)] = &[
    ("account", 0),
    ("app", 1),
    ("file", 2),
    ("tab", 3),
    ("recording", 4),
];

fn kind_rank(key: &str) -> Option<u8> {
    let kind = key.split(':').next().unwrap_or("");
    KIND_ORDER.iter().find(|(k, _)| *k == kind).map(|(_, r)| *r)
}

/// `ordered(keys)`: deduplicated, in the one global order; an unknown kind is refused (Python's `ValueError`).
pub fn ordered(keys: &[String]) -> Result<Vec<String>, String> {
    let uniq: BTreeSet<&String> = keys.iter().collect();
    for k in &uniq {
        if kind_rank(k).is_none() {
            return Err(format!(
                "unknown lock kind in {}",
                crate::words::str_repr(k)
            ));
        }
    }
    let mut out: Vec<String> = uniq.into_iter().cloned().collect();
    out.sort_by(|a, b| (kind_rank(a), a).cmp(&(kind_rank(b), b)));
    Ok(out)
}

#[derive(Default)]
struct State {
    locks: HashMap<String, Arc<AsyncMutex<()>>>,
    holders: HashMap<String, String>,
    reading: BTreeSet<String>,
    waiting: HashMap<String, usize>,
}

/// `ResourceLocks`.
#[derive(Clone, Default)]
pub struct ResourceLocks {
    state: Arc<Mutex<State>>,
}

/// Locks held until dropped (`hold`'s and `try_hold`'s context).
pub struct Held {
    state: Arc<Mutex<State>>,
    taken: Vec<(String, OwnedMutexGuard<()>)>,
    reading: bool,
}

impl Drop for Held {
    fn drop(&mut self) {
        let mut s = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        while let Some((k, guard)) = self.taken.pop() {
            s.holders.remove(&k);
            if self.reading {
                s.reading.remove(&k);
            }
            drop(guard);
        }
    }
}

impl ResourceLocks {
    pub fn new() -> ResourceLocks {
        ResourceLocks::default()
    }

    fn st(&self) -> std::sync::MutexGuard<'_, State> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn lock_of(&self, k: &str) -> Arc<AsyncMutex<()>> {
        self.st()
            .locks
            .entry(k.to_string())
            .or_insert_with(|| Arc::new(AsyncMutex::new(())))
            .clone()
    }

    /// `hold(keys, holder, reading)`: every lock in order, waiting for each. `reading`: the call only reads what
    /// these keys name (it declares `observation`); a bystander may look alongside it.
    pub async fn hold(&self, keys: &[String], holder: &str, reading: bool) -> Result<Held, String> {
        let order = ordered(keys)?;
        let mut held = Held {
            state: self.state.clone(),
            taken: Vec::new(),
            reading,
        };
        for k in order {
            let lock = self.lock_of(&k);
            *self.st().waiting.entry(k.clone()).or_insert(0) += 1;
            struct Waiting<'a>(&'a ResourceLocks, String);
            impl Drop for Waiting<'_> {
                fn drop(&mut self) {
                    let mut s = self.0.st();
                    if let Some(n) = s.waiting.get_mut(&self.1) {
                        *n -= 1;
                        if *n == 0 {
                            s.waiting.remove(&self.1);
                        }
                    }
                }
            }
            let mark = Waiting(self, k.clone());
            let guard = lock.lock_owned().await;
            drop(mark);
            let mut s = self.st();
            s.holders.insert(k.clone(), holder.to_string());
            if reading {
                s.reading.insert(k.clone());
            }
            drop(s);
            held.taken.push((k, guard));
        }
        Ok(held)
    }

    /// `try_hold(keys, holder, during_reads)`: every lock only if each is free and nobody waits for it, else none
    /// at once. `during_reads` (DMN-406): a key a reading call holds, with nobody waiting, counts as available; it is
    /// looked at alongside, not taken.
    pub fn try_hold(
        &self,
        keys: &[String],
        holder: &str,
        during_reads: bool,
    ) -> Result<Option<Held>, String> {
        let order = ordered(keys)?;
        let locks: Vec<(String, Arc<AsyncMutex<()>>)> =
            order.iter().map(|k| (k.clone(), self.lock_of(k))).collect();
        let mut s = self.st();
        for (k, lock) in &locks {
            let waited = s.waiting.get(k).copied().unwrap_or(0) > 0;
            let busy = lock.try_lock().is_err();
            if waited || (busy && !(during_reads && s.reading.contains(k))) {
                return Ok(None);
            }
        }
        let mut held = Held {
            state: self.state.clone(),
            taken: Vec::new(),
            reading: false,
        };
        for (k, lock) in locks {
            let Ok(guard) = lock.clone().try_lock_owned() else {
                continue; // a read holds it: looked at alongside
            };
            s.holders.insert(k.clone(), holder.to_string());
            held.taken.push((k, guard));
        }
        Ok(Some(held))
    }

    /// Who holds a key now.
    pub fn holder(&self, key: &str) -> Option<String> {
        self.st().holders.get(key).cloned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn order_is_kind_then_name() {
        let keys: Vec<String> = ["tab:b", "file:/x", "account:a", "tab:a", "tab:b"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(
            ordered(&keys).unwrap(),
            vec!["account:a", "file:/x", "tab:a", "tab:b"]
        );
        assert_eq!(
            ordered(&["disk:x".to_string()]).unwrap_err(),
            "unknown lock kind in 'disk:x'"
        );
    }

    #[tokio::test]
    async fn a_bystander_never_queues_ahead() {
        let l = ResourceLocks::new();
        let keys = vec!["tab:t1".to_string()];
        let held = l.hold(&keys, "w1", false).await.unwrap();
        assert!(l.try_hold(&keys, "preview", true).unwrap().is_none());
        drop(held);
        let read = l.hold(&keys, "w1", true).await.unwrap();
        let look = l.try_hold(&keys, "preview", true).unwrap();
        assert!(look.is_some(), "a read is looked at alongside");
        assert_eq!(l.holder("tab:t1").as_deref(), Some("w1"));
        drop(look);
        drop(read);
        assert!(l.try_hold(&keys, "preview", false).unwrap().is_some());
    }
}
