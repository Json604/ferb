//! The daemon's phone area (stage 3 S3, kb T-1202) in this program: ferb-daemon's `phone` on the daemon's store, clock,
//! redactor, journal and bus, FERB's questions through the gateway's one question store, and the phone link helper's
//! code-signature check (PHN-441) on the server. The helper itself (S4) is not started here yet.

use std::path::PathBuf;
use std::sync::Arc;

use ferb_core::interfaces::Clock;
use ferb_daemon::phone::files::{FileLimits, FileSeams, Judge, MadeFiles, TaskFile};
use ferb_daemon::phone::find::Made;
use ferb_daemon::phone::{Limits, MacPrompts, Phones, QuestionFacts, Questions, Seams};
use ferb_gateway::gateway::{Gateway, OwnEvent, OwnQuestion, SyncProtection};
use ferb_gateway::questions::{State, OWN_CHECKS, SECRET_KINDS};
use ferb_rpc::Server;
use ferb_sanitize::redact::Redactor;
use serde_json::{Map, Value};

use crate::basics::{Events, Journal, SharedDb};
use crate::off::off;

/// What the phone area takes from the composition.
pub struct Parts {
    pub db: SharedDb,
    pub clock: Arc<dyn Clock>,
    pub redactor: Arc<Redactor>,
    pub gateway: Arc<Gateway>,
    pub journal: Arc<Journal>,
    pub events: Arc<Events>,
    pub home: PathBuf,
    /// The real daemon: the helper's code signature is checked by the Security framework. A test daemon (the
    /// stand-ins' FERB.app answer) leaves the check unset: nobody is the helper.
    pub real_peers: bool,
    /// FERB's protection lists, as the gateway judges the model's file tools by them (stage 3 S3b: the phone's files).
    pub protection: Arc<SyncProtection>,
    /// The owner's home: the phone's uploads land in its `Downloads/FERB from phone` (T-1203's owner question).
    pub user_home: PathBuf,
    /// The real Mac's own readings (its computer name, for a phone's `phone.list`); a stand-in daemon reads nothing.
    pub native: bool,
    /// FERB's settings: the owner's choice of upload folder (`[phone] upload_folder`, PHN-581).
    pub config: Arc<ferb_core::config::Config>,
    /// PHN-828: the site's pinned keys and the Mac's own FERB account, for the pairing's account check.
    pub account: ferb_daemon::phone::account::AccountCheck,
}

/// PHN-581: where a phone's uploads land by default.
pub fn default_inbox(user_home: &std::path::Path) -> PathBuf {
    user_home.join("Downloads").join("FERB from phone")
}

/// The owner's choice in config.toml (`[phone] upload_folder`), else the default.
fn chosen_inbox(config: &ferb_core::config::Config, user_home: &std::path::Path) -> PathBuf {
    match config.get("phone", "upload_folder") {
        Ok(Some(Value::String(p))) if p.starts_with('/') => PathBuf::from(p),
        _ => default_inbox(user_home),
    }
}

/// The phone's file area judges paths by the gateway's own lists (`Protection::fs_rules`): one judge (PHN-114).
struct Lists(Arc<SyncProtection>);

impl Judge for Lists {
    fn home(&self) -> String {
        self.0 .0.home.clone()
    }
    fn ferb_home(&self) -> String {
        self.0 .0.ferb_home.clone()
    }
    fn rules(&self, path: &str, access: &str) -> Vec<String> {
        self.0 .0.fs_rules(path, access)
    }
}

/// FERB's questions as the phone area reads and asks them: the gateway's store, with its own kinds (SECRET_KINDS,
/// OWN_CHECKS) and the routine class as the gateway itself offered it (the 30-minute answer, GATE-207).
struct GatewayQuestions(Arc<Gateway>);

impl Questions for GatewayQuestions {
    fn facts(&self, approval_id: &str) -> Option<QuestionFacts> {
        let ap = self.0.approval(approval_id)?;
        Some(QuestionFacts {
            pending: ap.state == State::Pending,
            hidden: SECRET_KINDS.contains(&ap.kind.as_str()),
            own_check: OWN_CHECKS.contains(&ap.capability.as_str()),
            routine: ap
                .options
                .iter()
                .any(|o| o == ferb_gateway::words::ALLOW_WINDOW),
        })
    }

    // decision: GATE-615
    fn until_back_offered(&self, approval_id: &str) -> bool {
        self.0.until_back_offered(approval_id)
    }

    fn grant_until_back(&self, approval_id: &str, phone: &str, name: &str) -> Result<(), String> {
        self.0
            .grant_until_back(approval_id, phone, name)
            .map(|_| ())
    }

    fn end_until_back(&self, why: &str, phone: Option<&str>) {
        let Some(why) = ferb_gateway::screen_use::Ended::parse(why) else {
            return;
        };
        let running = self.0.until_back();
        if running.is_some_and(|g| phone.is_none_or(|p| g.phone == p)) {
            self.0.end_until_back(why);
        }
    }

    fn screen_use_status(&self) -> Value {
        Value::Object(self.0.screen_use_status())
    }

    fn ask_check(&self, reason: &str, exact: &str, task_id: &str) -> String {
        self.0
            .ask_own(&OwnQuestion {
                capability: "ferb.confirm".into(),
                reason: reason.into(),
                options: vec!["Yes".into(), "No".into()],
                request_id: String::new(),
                approval_id: None,
                summary: Some(reason.into()),
                detail: Some(exact.into()),
                labels: Vec::new(),
                task_id: task_id.into(),
                part: String::new(),
                runtime_switch: None,
                options_only: true,
                from_task: false,
                corrects: false,
                exact: exact.into(),
                event: OwnEvent::Publish,
            })
            .id
    }
}

/// A task's outcome as stored (`tasks.outcome_json`).
fn outcome_of(db: &SharedDb, task_id: &str) -> Option<Value> {
    let db = crate::basics::lock(db);
    let text: String = db
        .conn()
        .query_row(
            "SELECT outcome_json FROM tasks WHERE id = ?",
            [task_id],
            |r| r.get(0),
        )
        .ok()?;
    serde_json::from_str(&text).ok()
}

fn artifacts(outcome: &Value) -> Vec<String> {
    outcome["artifacts"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(Value::as_str)
                .filter(|p| p.starts_with('/'))
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// `phone.read {artifact: {task_id, index}}`: the index-th file the task's outcome names (the result card's files,
/// sorted as stored). The file area judges it as any path.
pub fn task_file(db: SharedDb) -> Arc<TaskFile> {
    Arc::new(move |task_id: &str, index: u64| {
        let outcome = outcome_of(&db, task_id)?;
        artifacts(&outcome)
            .into_iter()
            .nth(index as usize)
            .map(PathBuf::from)
    })
}

/// `phone.recent`'s first source: the files FERB's root tasks made, newest task first, at most `most`.
pub fn made_files(db: SharedDb) -> Arc<MadeFiles> {
    Arc::new(move |most: usize| {
        let rows = {
            let db = crate::basics::lock(&db);
            ferb_store::task_results::TaskResults::new(&db)
                .page("9999", most as i64)
                .unwrap_or_default()
        };
        let mut out = Vec::new();
        for r in rows {
            let outcome: Value = r
                .get("outcome_json")
                .and_then(Value::as_str)
                .and_then(|t| serde_json::from_str(t).ok())
                .unwrap_or(Value::Null);
            let id = r.get("id").and_then(Value::as_str).unwrap_or_default();
            let goal = r.get("goal").and_then(Value::as_str).unwrap_or_default();
            let name =
                ferb_orchestrator::names::shown_name(r.get("name").and_then(Value::as_str), goal);
            let at = r
                .get("updated_at")
                .and_then(Value::as_str)
                .unwrap_or_default();
            for path in artifacts(&outcome) {
                out.push(Made {
                    path,
                    at: at.to_string(),
                    task_id: id.to_string(),
                    task_name: name.clone(),
                });
                if out.len() >= most {
                    return out;
                }
            }
        }
        out
    })
}

/// Builds the phone area, registers its methods and gate on `server`, and puts the helper's check in place.
pub fn register(server: &Server, p: Parts) -> Phones {
    let redactor = p.redactor.clone();
    let journal = p.journal.clone();
    let events = p.events.clone();
    let phones = Phones::new(
        Seams {
            db: p.db.clone(),
            clock: p.clock.clone(),
            nonce: Seams::system_nonce(),
            redact: Arc::new(move |v: &Value| redactor.value(v)),
            questions: Arc::new(GatewayQuestions(p.gateway.clone())),
            prompts: off("phone.no_prompts", Option::<Arc<dyn MacPrompts>>::None),
            journal: Arc::new(move |kind: &str, data: Value| journal.note(kind, data)),
            publish: Arc::new(move |topic: &str, data: Map<String, Value>| {
                events.publish(topic, data)
            }),
            account: p.account,
        },
        Limits::default(),
    );
    phones.register(server);
    // T-1221: the Mac's name, for the phone's link picture and "Paired with" (asked behind the phone's unlock)
    let native = p.native;
    phones.set_mac_name(Arc::new(move || {
        crate::compose::device::computer_name(native)
    }));
    // stage 3 S3b (T-1203): files both ways, judged by the same lists as the model's file tools
    phones.register_files(
        server,
        FileSeams {
            judge: Arc::new(Lists(p.protection.clone())),
            inbox: chosen_inbox(&p.config, &p.user_home),
            free_bytes: FileSeams::system_free_bytes(),
            added_places: Vec::new(),
            // T-1211: a finished task's files (its outcome's `artifacts`), for result cards and recent files
            task_file: Some(task_file(p.db.clone())),
            made_files: Some(made_files(p.db.clone())),
            hook: None,
        },
        FileLimits::default(),
    );
    // PHN-581: the owner's choice of folder is kept in config.toml's `[phone] upload_folder`
    let config = p.config.clone();
    phones.set_upload_place(
        default_inbox(&p.user_home),
        Arc::new(move |path: Option<&str>| {
            let done = match path {
                Some(path) => config.set("phone", "upload_folder", &Value::String(path.into())),
                None => config.remove("phone", &["upload_folder"]),
            };
            done.map_err(|e| e.to_string())
        }),
    );
    register_screen_use(server, p.gateway.clone());
    if p.real_peers {
        server.set_is_link(ferb_platform_macos::rpc::link_system(&p.home).into_check());
    }
    phones
}

/// GATE-615: `screen_use.status` (what is in force, whether it can be offered) and `screen_use.end` (the owner turns
/// it off, from FERB.app or a phone; journaled with who).
fn register_screen_use(server: &Server, gateway: Arc<Gateway>) {
    let gw = gateway.clone();
    server.register_unary("screen_use.status", move |_s, _p: Map<String, Value>| {
        let out = gw.screen_use_status();
        async move { Ok(out) }
    });
    server.register_unary(
        "screen_use.end",
        move |s: ferb_rpc::Session, _p: Map<String, Value>| {
            let by = s
                .phone
                .as_ref()
                .map_or_else(|| "mac".to_owned(), |p| p.device.clone());
            let ended = gateway
                .end_until_back(ferb_gateway::screen_use::Ended::TurnedOff)
                .is_some();
            let mut out = gateway.screen_use_status();
            out.insert("ended".into(), Value::Bool(ended));
            out.insert("by".into(), Value::String(by));
            async move { Ok(out) }
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    fn db() -> (tempfile::TempDir, SharedDb) {
        let dir = tempfile::tempdir().unwrap();
        let db = ferb_store::Db::open(dir.path().join("ferb.db")).unwrap();
        db.migrate().unwrap();
        let c = db.conn();
        for (id, parent, goal, outcome, at) in [
            (
                "t1",
                None,
                "Download the October invoice",
                r#"{"artifacts": ["/Users/o/Downloads/Invoice-Oct.pdf", "/Users/o/Downloads/receipt.txt"]}"#,
                "2026-11-16T09:00:00+05:30",
            ),
            (
                "t1.1",
                Some("t1"),
                "a step",
                r#"{"artifacts": ["/Users/o/step.txt"]}"#,
                "2026-11-16T09:30:00+05:30",
            ),
            (
                "t2",
                None,
                "Compare desks",
                r#"{"artifacts": ["relative.txt"]}"#,
                "2026-11-16T10:00:00+05:30",
            ),
        ] {
            c.execute(
                "INSERT INTO tasks (id, tree_id, parent_id, request_id, goal, identity, role, scopes_json, status, \
                 outcome_json, created_at, updated_at) VALUES (?, 't1', ?, 'r', ?, 'owner', 'mixed', '[]', 'done', ?, ?, ?)",
                rusqlite::params![id, parent, goal, outcome, at, at],
            )
            .unwrap();
        }
        (dir, Arc::new(Mutex::new(db)))
    }

    /// T-1211: a finished task's files reach the phone: by index for its result card (`phone.read {artifact}`) and
    /// as recent files (root tasks only, whole paths only, newest task first).
    // decision: PHN-528
    #[test]
    fn a_finished_task_s_files_are_the_phone_s_by_index_and_as_recent_files() {
        let (_dir, db) = db();
        let file = task_file(db.clone());
        assert_eq!(
            file("t1", 1),
            Some(PathBuf::from("/Users/o/Downloads/receipt.txt"))
        );
        assert_eq!(file("t1", 2), None);
        assert_eq!(file("t2", 0), None, "only a whole path");
        assert_eq!(file("nope", 0), None);
        let made = made_files(db)(10);
        let paths: Vec<&str> = made.iter().map(|m| m.path.as_str()).collect();
        assert_eq!(
            paths,
            vec![
                "/Users/o/Downloads/Invoice-Oct.pdf",
                "/Users/o/Downloads/receipt.txt"
            ]
        );
        assert_eq!(made[0].task_id, "t1");
        assert!(!made[0].task_name.is_empty());
    }
}
