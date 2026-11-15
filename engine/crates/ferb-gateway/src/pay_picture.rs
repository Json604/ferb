//! The payment question's one-time picture (`ferb/daemon/pay_picture.py`; GATE-578, GATE-579, APP-828): the page a
//! payment control is on, captured when FERB asks, for the owner's eyes only.
//!
//! Owner-only by construction: the bytes live in this process's memory only, keyed by the question's id. They never
//! go into a model call, a tool result, a span, the journal, a log or the database; FERB.app takes them once, and they
//! go when the question is answered, set aside or closed, when its task ends, or when the daemon stops (nothing
//! survives a restart). The capture itself is the tool's (`Tool::pay_picture`), under the call's locks.

use std::collections::HashMap;

pub const PICTURE_LIMIT_S: f64 = 8.0;
pub const NOT_CAPTURED: &str = "FERB couldn't capture the payment page.";

/// `PayPictures`: by question id, with the task tree each belongs to.
#[derive(Default)]
pub struct PayPictures {
    held: HashMap<String, (String, Vec<u8>)>,
}

impl std::fmt::Debug for PayPictures {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "PayPictures({} held)", self.held.len())
    }
}

impl PayPictures {
    pub fn hold(&mut self, approval_id: &str, root: &str, image: Vec<u8>) {
        self.held
            .insert(approval_id.to_string(), (root.to_string(), image));
    }

    pub fn has(&self, approval_id: &str) -> bool {
        self.held.contains_key(approval_id)
    }

    /// The picture, once: FERB.app's one fetch removes it.
    pub fn take(&mut self, approval_id: &str) -> Option<Vec<u8>> {
        self.held.remove(approval_id).map(|(_, b)| b)
    }

    pub fn drop_picture(&mut self, approval_id: &str) {
        self.held.remove(approval_id);
    }

    /// A task tree ended: its pictures go, taken or not.
    pub fn forget(&mut self, roots: &[String]) {
        self.held.retain(|_, (root, _)| {
            !roots
                .iter()
                .any(|r| root == r || root.starts_with(&format!("{r}.")))
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // decision: GATE-579
    #[test]
    fn taken_once_and_gone_with_its_tree() {
        let mut p = PayPictures::default();
        p.hold("ap-1", "t1.2", vec![1, 2]);
        p.hold("ap-2", "t2", vec![3]);
        assert_eq!(p.take("ap-1"), Some(vec![1, 2]));
        assert_eq!(p.take("ap-1"), None);
        p.hold("ap-3", "t1.4", vec![4]);
        p.forget(&["t1".to_string()]);
        assert!(!p.has("ap-3"));
        assert!(p.has("ap-2"));
    }
}
