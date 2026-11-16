//! APP-777 (refining APP-1): a task the planner gave to FERB while FERB has no browser profile of its own works as
//! the owner in the profile they picked (supervisor.py:3004 `_browse_as`, :3036 `_await_browsing`), through the
//! daemon's browsing answer ([`Daemon::browsing`](super::Daemon::browsing), areas/browsers.py `_browsing`).
//!
//! The Rust `Task` is fixed: the worker's new identity rides on the tree's state (`as_owner`), read through
//! [`Tree::worker_task`](super::Tree::worker_task) by the worker, its checker and every member made after.
//!
//! decision: APP-777
//! decision: HAND-580
//! decision: HAND-581
//! decision: HAND-587

use std::sync::Arc;

use ferb_orchestrator::browsing;
use serde_json::{json, Map, Value};

use super::{Inner, Tree};

/// Profiles as "browser/folder", for the journal.
fn shown(profiles: &[(String, String)]) -> Vec<String> {
    profiles.iter().map(|(b, f)| format!("{b}/{f}")).collect()
}

fn text(found: &Map<String, Value>, key: &str) -> String {
    found
        .get(key)
        .filter(|v| ferb_py::value::truthy(v))
        .map(ferb_py::value::py_str)
        .unwrap_or_default()
}

impl Inner {
    /// APP-777, HAND-580: the owner's profiles the live tree of `task_id` (a tree, its worker, a step or a checker) is
    /// kept to for its browser tools (ferb-browser `Tasks::task_profiles`): whether the owner's pick is one (the tree
    /// browses as the owner there, `as_owner`), and the profiles the owner's words named for it. None: kept to none
    /// (a tree the planner gave the owner's identity that named nothing and browses where the worker says, HAND-2;
    /// FERB's own profile).
    pub fn task_profiles(&self, task_id: &str) -> Option<(bool, Vec<(String, String)>)> {
        let root = self.tree_of(task_id)?;
        let tree = self.live().tree_of.get(&root).cloned()?;
        if tree.id != root {
            return None;
        }
        let st = tree.lock();
        (st.as_owner || !st.profiles.is_empty()).then(|| (st.as_owner, st.profiles.clone()))
    }

    /// APP-777: whether the live tree of `task_id` browses as the owner in the profile they picked (`as_owner`).
    pub fn browses_as_owner(&self, task_id: &str) -> bool {
        self.task_profiles(task_id).is_some_and(|(pick, _)| pick)
    }

    /// supervisor.py:3004 `_browse_as`: before the tree's first worker starts. HAND-580 first: the profiles the owner's
    /// words named for this task (the planner's `profiles`, each resolved to one of the owner's profiles) are where it
    /// browses, whatever the pick; nothing is asked and the pick isn't applied. Else ORCH-648: a tree the planner gave
    /// the owner's identity whose task names sites (it will browse) is one too, unless the owner's words name one of
    /// their profiles: without a pick FERB's one browsing question is asked first and its answer saved; with a pick
    /// the tree browses there (`as_owner`). Python asked only for identity "ferb", so an owner-identity task let the
    /// worker take any paired profile and a plan asked which one as its own step.
    pub(crate) async fn browse_as(&self, tree: &Arc<Tree>) {
        let spec = tree.spec();
        if !spec.profiles.is_empty() {
            let resolved = self.deps.daemon.owner_profiles(&spec.profiles);
            let named = self.owners_own(tree, resolved);
            if !named.is_empty() {
                if spec.identity != "owner" {
                    self.make_owners(tree);
                }
                tree.lock().profiles = named.clone();
                self.journal(
                    "task.browses_in",
                    json!({"task_id": tree.id, "profiles": shown(&named), "how": "planned"}),
                    None,
                );
                return;
            }
            // a name that resolves to none of the owner's profiles: as if none was named (the pick, else the question)
            self.journal(
                "task.profiles_unresolved",
                json!({"task_id": tree.id, "named": spec.profiles}),
                None,
            );
        }
        let as_owner_already = match spec.identity.as_str() {
            "ferb" => false,
            "owner" => {
                let words = self.deps.daemon.request(&tree.worker.request_id).words;
                if spec.targets.sites.is_empty() || self.deps.daemon.names_profile(&words) {
                    return;
                }
                true
            }
            _ => return,
        };
        let task = tree.worker_task();
        let attended = !self.unattended_task(&task);
        let mut found = self.deps.daemon.browsing(attended, &task.request_id);
        let asked = text(&found, "ask");
        if !asked.is_empty() {
            self.await_browsing(tree, &asked).await;
            if self.stopped(tree) {
                return;
            }
            // asked once per task: no answer goes on as is
            found = self.deps.daemon.browsing(false, &task.request_id);
        }
        if found.get("as_owner").is_some_and(ferb_py::value::truthy) {
            if !as_owner_already {
                self.make_owners(tree);
            }
            tree.lock().as_owner = true;
            self.journal("task.browses_as_owner", json!({"task_id": tree.id}), None);
            if !asked.is_empty() {
                let line = browsing::CHOSEN.replace("{label}", &text(&found, "label"));
                tree.lock().side_lines.push(line);
            }
        } else if found.get("declined").is_some_and(ferb_py::value::truthy) && !asked.is_empty() {
            tree.lock()
                .side_lines
                .push(browsing::NONE_CHOSEN.to_string());
        } else if found.get("note").is_some_and(ferb_py::value::truthy) && !as_owner_already {
            // (a task as the owner with nobody here and no pick goes on as before: it browses where the worker says)
            let note = text(&found, "note");
            tree.lock().side_lines.push(note);
        }
    }

    /// HAND-587 (the owner, 2026-11-16: "yes, only from my own words"): of the planner's resolved profiles, the ones
    /// the owner's own words give this task. This message's words first: a profile they touch is kept. Only when none
    /// of the planned profiles is this message's, one the owner's own remembered words name (memory of origin
    /// owner_stated) is kept, if it is paired now. Any other (only FERB's own memory names it, or nothing ties it to
    /// the owner's words) is dropped and journaled; with none kept the task goes on as if none was named.
    fn owners_own(
        &self,
        tree: &Arc<Tree>,
        resolved: Vec<(String, String)>,
    ) -> Vec<(String, String)> {
        if resolved.is_empty() {
            return resolved;
        }
        let words = self.deps.daemon.request(&tree.worker.request_id).words;
        let src = self
            .deps
            .daemon
            .profile_sources(&tree.worker.request_id, &words);
        let said: Vec<(String, String)> = resolved
            .iter()
            .filter(|p| src.said.contains(p))
            .cloned()
            .collect();
        let mut kept = said.clone();
        let mut remembered = Vec::new();
        let mut dropped = Vec::new();
        for p in resolved.iter().filter(|p| !said.contains(p)) {
            let why = if src.owner_memory.contains(p) {
                if !said.is_empty() {
                    "this message names another profile"
                } else if !src.paired.contains(p) {
                    "the owner's remembered profile isn't paired"
                } else {
                    remembered.push(p.clone());
                    kept.push(p.clone());
                    continue;
                }
            } else if src.other_memory.contains(p) {
                "only FERB's own memory names it, not the owner's words"
            } else {
                "nothing in the owner's words names it"
            };
            dropped.push(json!({"profile": format!("{}/{}", p.0, p.1), "why": why}));
        }
        if !remembered.is_empty() {
            self.journal(
                "task.profiles_remembered",
                json!({"task_id": tree.id, "profiles": shown(&remembered)}),
                None,
            );
        }
        if !dropped.is_empty() {
            self.journal(
                "task.profiles_dropped",
                json!({"task_id": tree.id, "dropped": dropped}),
                None,
            );
        }
        kept
    }

    /// `_browse_as`'s identity change: the tree, its plan's FERB conversations and its worker become the owner's, so
    /// the owner's browser tools and checks apply unchanged (HAND-426).
    fn make_owners(&self, tree: &Arc<Tree>) {
        {
            let mut st = tree.lock();
            if let Some(spec) = st.spec.as_mut() {
                spec.identity = "owner".into();
                if let Some(plan) = spec.plan.as_mut() {
                    for step in &mut plan.steps {
                        if let Some(scope) = step.conversation.as_mut() {
                            if scope.identity == "ferb" {
                                scope.identity = "owner".into();
                            }
                        }
                    }
                }
            }
        }
        let _ = self.tasks(|t| t.set_identity(&tree.worker.id, "owner"));
    }

    /// HAND-581: profiles the owner's words gave a running task after it started (a correction's `profiles`, an
    /// answer to the task's own question naming them, the owner's Yes to its Yes/No question naming one) join the
    /// ones it may use, for this task only (the saved pick is unchanged). Only a tree kept to some profiles takes
    /// them: one kept to none already reaches every paired profile. The ones added; each is journaled.
    pub(crate) fn add_task_profiles(
        &self,
        tree: &Tree,
        found: &[(String, String)],
        how: &str,
    ) -> Vec<(String, String)> {
        let added: Vec<(String, String)> = {
            let mut st = tree.lock();
            if !st.as_owner && st.profiles.is_empty() {
                return Vec::new();
            }
            let new: Vec<(String, String)> = found
                .iter()
                .filter(|p| !st.profiles.contains(p))
                .cloned()
                .collect();
            for p in &new {
                if !st.profiles.contains(p) {
                    st.profiles.push(p.clone());
                }
            }
            new
        };
        if !added.is_empty() {
            self.journal(
                "task.profiles_added",
                json!({"task_id": tree.id, "profiles": shown(&added), "how": how}),
                None,
            );
        }
        added
    }

    /// HAND-581: the owner answered a question of the task `task_id` asked them (never one the orchestrator answered
    /// from context): the profiles their words name join the task's, and so do the ones the question itself names
    /// when it was a Yes/No question (the task's options exactly "Yes" and "No") answered Yes.
    pub fn owner_answered_task(
        &self,
        task_id: &str,
        question: &str,
        options: &[String],
        answer: &str,
    ) {
        let Some(root) = self.tree_of(task_id) else {
            return;
        };
        let Some(tree) = self.live().tree_of.get(&root).cloned() else {
            return;
        };
        if tree.id != root {
            return;
        }
        let mut found = self.deps.daemon.profiles_in_words(answer);
        let how = if options == ["Yes", "No"] && answer == "Yes" {
            for p in self.deps.daemon.profiles_in_words(question) {
                if !found.contains(&p) {
                    found.push(p);
                }
            }
            "answered_yes"
        } else {
            "answered"
        };
        if !found.is_empty() {
            self.add_task_profiles(&tree, &found, how);
        }
    }

    /// supervisor.py:3036 `_await_browsing`: the task shows paused while FERB's browsing question waits; the answer
    /// is applied once, by whichever of its paths saw it first. A stop leaves the question open for the next task.
    async fn await_browsing(&self, tree: &Arc<Tree>, approval_id: &str) {
        self.set_status(&tree.worker.id, "paused", None);
        self.set_status(&tree.id, "paused", None);
        while !self.stopped(tree) {
            match self.deps.daemon.approval(approval_id) {
                Some(ap) if ap.state == ferb_gateway::questions::State::Pending => {}
                Some(ap) => {
                    self.deps.daemon.question_answered(&ap);
                    break;
                }
                None => break,
            }
            let poll = self.deps.daemon.hold_poll_s();
            tokio::time::sleep(std::time::Duration::from_secs_f64(poll)).await;
        }
        if tree.lock().stop.is_empty() {
            self.set_status(&tree.worker.id, "planned", None);
            self.set_status(&tree.id, "running", None);
        }
    }
}
