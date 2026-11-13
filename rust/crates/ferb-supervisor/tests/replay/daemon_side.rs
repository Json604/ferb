//! The daemon's own side a chat scenario reaches, as the daemon program composes it: ferb-daemon's group A (its
//! real handlers for `schedules.*`, `reminders.*`, `timers.*`, its timer loop's pass, a schedule's run and its
//! bookkeeping) on its own loop thread, with the supervisor plugged into its seams (ferb-supervisor's
//! `daemon_seams`: `SupervisorTasks`, `ResultsOwner`, `DaemonSchedules`), served by a real ferb-rpc server on a
//! socket in the scenario's folder. The test talks to it with a blocking client, so nothing on the replay's paused
//! clock runs while a call is answered.
//!
//! Group B's notifier (`_notify`, the notifications stream) is the one stand-in: each banner group A or the owner's
//! side gives it becomes the `notifications.subscribe.event` line Python's notifier sends a connected client.

use std::collections::{BTreeSet, HashMap};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use ferb_daemon::areas_a::seams::{Ask, LoginError, Logins, Questions};
use ferb_daemon::areas_a::State;
use ferb_daemon::shared::Local;
use ferb_supervisor::flow::daemon_seams::{Banner, ResultsOwner, SupervisorTasks};
use ferb_supervisor::flow::Supervisor;
use serde_json::{json, Value};

/// FERB's own questions outside a task: none in these scenarios.
struct NoQuestions;

impl Questions for NoQuestions {
    fn ask(&self, _ask: Ask<'_>) -> Result<String, String> {
        Err("no own question in this scenario".into())
    }
    fn supersede(&self, _approval_id: &str) -> bool {
        false
    }
    fn waiting(&self, _approval_id: &str) -> bool {
        false
    }
}

struct NoLogins;

impl Logins for NoLogins {
    fn clear_usernames(&self, _sources: &BTreeSet<String>) -> Result<Vec<String>, LoginError> {
        Ok(Vec::new())
    }
}

struct RpcClock;

impl ferb_rpc::Clock for RpcClock {
    fn monotonic(&self) -> f64 {
        1000.0
    }
}

/// What the daemon's side was built from.
pub struct Parts {
    pub home: PathBuf,
    pub user: PathBuf,
    pub db_path: PathBuf,
    pub repo: PathBuf,
    pub clock: Arc<dyn ferb_clock::clock::ZonedClock>,
    pub ids: Arc<dyn ferb_core::interfaces::IdSource>,
    pub redactor: Arc<ferb_sanitize::Redactor>,
    pub publish: ferb_daemon::areas_a::Publish,
    /// The timer claim token the recorder's loop was at (`t%015d`).
    pub first_token: u64,
    pub supervisor: Supervisor,
}

pub struct DaemonSide {
    pub local: Local<State>,
    socket: PathBuf,
    stop: Option<tokio::sync::oneshot::Sender<()>>,
    thread: Option<std::thread::JoinHandle<()>>,
    conns: Mutex<HashMap<String, (BufReader<UnixStream>, UnixStream)>>,
    /// The notifications a connected client got, as their lines.
    pub notes: Arc<Mutex<Vec<String>>>,
}

impl DaemonSide {
    /// Group A on its loop and a server on a thread of its own; the supervisor's seams on the replay's runtime.
    pub fn start(p: Parts) -> DaemonSide {
        let socket = p.home.join("d.sock");
        let notes: Arc<Mutex<Vec<String>>> = Arc::default();
        let runtime = tokio::runtime::Handle::current();
        let notify = {
            let notes = notes.clone();
            Arc::new(move |b: Banner| {
                // notifier.py's line to a connected FERB.app (`notifications.subscribe.event`)
                let mut params = json!({"id": b.ident, "category": b.category, "title": b.title, "body": b.text,
                    "target": b.target, "when_hidden": b.when_hidden});
                if !b.card.is_null() {
                    params["card"] = b.card.clone();
                }
                notes.lock().unwrap().push(ferb_py::json::dumps(
                    &json!({"jsonrpc": "2.0", "method": "notifications.subscribe.event", "params": params}),
                ));
                Box::pin(async {}) as ferb_orchestrator::ports::BoxFuture<'static, ()>
            })
        };
        let tokens = Arc::new(AtomicU64::new(p.first_token.saturating_sub(1)));
        let deps = ferb_daemon::areas_a::Deps {
            home: p.home.clone(),
            user_home: p.user.clone(),
            db_path: p.db_path.clone(),
            loop_thread: None,
            prompts: p.repo.join("ferb"),
            clock: p.clock.clone(),
            ids: p.ids.clone(),
            config: Arc::new(ferb_core::config::Config::new(&p.home)),
            redactor: p.redactor.clone(),
            publish: p.publish.clone(),
            timer_tokens: Some(Arc::new(move || {
                format!("t{:015}", tokens.fetch_add(1, Ordering::SeqCst) + 1)
            })),
            make_runtime: None,
            app_bundle: None,
            persona_scan: None,
            tasks: Arc::new(SupervisorTasks {
                supervisor: p.supervisor.clone(),
                runtime,
            }),
            owner: Arc::new(ResultsOwner {
                supervisor: p.supervisor.clone(),
                notify,
                // a FERB.app client is subscribed to notifications (authorized): it posts
                posters_connected: Arc::new(|| true),
                client_shows: Arc::new(|| true),
                tell: Arc::new(|_, _, _, _, _| {}),
            }),
            questions: Arc::new(NoQuestions),
            logins: Arc::new(NoLogins),
            purges: None,
        };
        let (tx, rx) = std::sync::mpsc::channel();
        let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
        let path = socket.clone();
        let thread = std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            rt.block_on(async move {
                let mut server = ferb_rpc::Server::new(
                    path,
                    Arc::new(ferb_rpc::RunTokens::new(Arc::new(RpcClock))),
                );
                let local = ferb_daemon::areas_a::register(&mut server, deps).expect("group A");
                server.set_is_app(|_| true);
                server.start().await.expect("the server");
                tx.send(local).unwrap();
                let _ = stopped.await;
                server.close().await;
            });
        });
        let local = rx.recv().expect("group A's state");
        DaemonSide {
            local,
            socket,
            stop: Some(stop),
            thread: Some(thread),
            conns: Mutex::default(),
            notes,
        }
    }

    /// One recorded request on connection `conn` (hello'd once as the recorder's was): the reply line.
    pub fn call(&self, conn: &str, line: &str, id: &Value) -> String {
        let mut conns = self.conns.lock().unwrap();
        let (reader, writer) = conns.entry(conn.to_string()).or_insert_with(|| {
            let s = UnixStream::connect(&self.socket).expect("the daemon's socket");
            let mut w = s.try_clone().unwrap();
            let mut r = BufReader::new(s);
            w.write_all(
                b"{\"jsonrpc\": \"2.0\", \"id\": 0, \"method\": \"auth.hello\", \"params\": {\"role\": \"cli\"}}\n",
            )
            .unwrap();
            let mut hello = String::new();
            r.read_line(&mut hello).unwrap();
            (r, w)
        });
        writer.write_all(format!("{line}\n").as_bytes()).unwrap();
        loop {
            let mut got = String::new();
            if reader.read_line(&mut got).unwrap() == 0 {
                return "(the connection closed)".into();
            }
            let got = got.trim_end_matches('\n').to_string();
            if serde_json::from_str::<Value>(&got).is_ok_and(|v| v["id"] == *id) {
                return got;
            }
        }
    }

    /// One pass of the timer loop (`Timers.tick` with `_timer_fired`), on group A's loop: how many it handed over.
    pub fn tick(&self) -> usize {
        self.local
            .call_blocking(|st| async move {
                ferb_daemon::areas_a::schedules::tick(&st)
                    .await
                    .unwrap_or_default()
            })
            .unwrap_or_default()
    }
}

impl Drop for DaemonSide {
    fn drop(&mut self) {
        self.conns.lock().unwrap().clear();
        if let Some(s) = self.stop.take() {
            let _ = s.send(());
        }
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}
