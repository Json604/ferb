//! The daemon's phone area (stage 3 S3, kb T-1202) in this program: ferb-daemon's `phone` on the daemon's store, clock,
//! redactor, journal and bus, FERB's questions through the gateway's one question store, and the phone link helper's
//! code-signature check (PHN-441) on the server. The helper itself (S4) is not started here yet.

use std::path::PathBuf;
use std::sync::Arc;

use ferb_core::interfaces::Clock;
use ferb_daemon::phone::files::{FileLimits, FileSeams, Judge};
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
        },
        Limits::default(),
    );
    phones.register(server);
    // stage 3 S3b (T-1203): files both ways, judged by the same lists as the model's file tools
    phones.register_files(
        server,
        FileSeams {
            judge: Arc::new(Lists(p.protection.clone())),
            inbox: p.user_home.join("Downloads").join("FERB from phone"),
            free_bytes: FileSeams::system_free_bytes(),
            added_places: Vec::new(),
            task_file: off("phone.no_task_file", None),
            hook: None,
        },
        FileLimits::default(),
    );
    if p.real_peers {
        server.set_is_link(ferb_platform_macos::rpc::link_system(&p.home).into_check());
    }
    phones
}
