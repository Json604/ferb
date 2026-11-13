//! The memory's background work (`areas/memory.py`): the search re-index asked at every start, the background
//! embedder (`_embed_loop`: the local model once, then new text every EMBED_EVERY_S), and the hourly memory pass
//! (`_distill_loop` / `_memory_pass`: distill the past days, refresh the episode cards, merge playbooks, the weekly
//! core-profile pass), each job in its own error scope, journaled when it fails and tried again next hour. The
//! learners' model calls go through the runtimes area's factory ([`learning_runtime`]).
//!
//! The weekly re-check of what FERB saw on this Mac (`_refresh_mac_profile`) is group A's `persona::refresh`. Not
//! composed here: downloading the embedding model for a checkout (FERB.app ships it; a daemon without it searches by
//! keywords and recency, as Python's does until it is there).

use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use ferb_daemon::areas_a::{self, State};
use ferb_daemon::areas_b::runtimes::Runtimes;
use ferb_learning::calls::Made;
use serde_json::{json, Value};

use super::Daemon;

/// EMBED_EVERY_S, EMBED_PAUSE_S (memory.py:389).
pub const EMBED_EVERY_S: u64 = 60;
pub const EMBED_PAUSE_S: f64 = 0.05;
/// The hourly pass.
pub const PASS_EVERY_S: u64 = 3600;

/// A runtime adapter shared as ferb-runtimes' `Adapter` (for `CoreRuntime`).
struct Shared(Arc<dyn ferb_runtimes::run::Adapter>);

impl ferb_runtimes::run::Adapter for Shared {
    fn name(&self) -> &str {
        self.0.name()
    }
    fn start<'a>(
        &'a self,
        task: &'a ferb_core::types::Task,
        request: &'a ferb_core::types::ModelRequest,
        run_token: &'a str,
    ) -> ferb_core::interfaces::BoxFuture<
        'a,
        Result<ferb_runtimes::run::Started, ferb_runtimes::error::RuntimeError>,
    > {
        self.0.start(task, request, run_token)
    }
}

/// `make_runtime` for the learners (the import, the profile pass, distill, playbooks): the runtimes area's
/// factory for the role, as ferb-core's `AgentRuntime`, with the model's documented limits. `area` is filled once
/// group B is built (group A's state is made first).
pub fn learning_runtime(area: Arc<OnceLock<Runtimes>>) -> Arc<areas_a::MakeRuntime> {
    Arc::new(
        move |role: &str,
              mode: &str,
              tools: &[String],
              schema: &Value|
              -> ferb_learning::Result<Made> {
            let rt = area.get().ok_or_else(|| {
                ferb_learning::Error::Runtime("FERB's model runtimes aren't set up yet".into())
            })?;
            let (build, built) = rt
                .make_runtime(role, mode, tools, schema, None)
                .map_err(|e| ferb_learning::Error::Runtime(e.to_string()))?;
            let limits =
                ferb_runtimes::catalog::limits(&build.name, Some(&build.model), &build.home).ok();
            Ok(Made {
                rt: Box::new(ferb_runtimes::run::CoreRuntime::new(Shared(built.adapter))),
                limits,
                model: (!build.model.is_empty()).then(|| build.model.clone()),
            })
        },
    )
}

/// The summary's seams (as group A's own).
struct SummarySeams;

impl ferb_learning::summary::Seams for SummarySeams {
    fn test_task(
        &self,
        store: &ferb_learning::store::Store,
        task_id: &str,
    ) -> ferb_learning::Result<bool> {
        ferb_orchestrator::test_runs::TestRuns::new(&store.db)
            .task(Some(task_id))
            .map_err(|e| ferb_learning::Error::Sql("OperationalError".into(), e.to_string()))
    }

    fn security_digest(
        &self,
        store: &ferb_learning::store::Store,
        day: &str,
    ) -> ferb_learning::Result<Value> {
        ferb_policy::stores::security_digest(&store.db, day).map_err(ferb_learning::Error::from)
    }
}

/// Where the embedding model's files are (`_embedding_root`): `[memory] embedding_model`, else the copy FERB.app
/// ships (Contents/Resources/models), else FERB's own copy under its home.
pub fn embedding_root(d: &Daemon) -> std::path::PathBuf {
    if let Ok(Some(Value::String(s))) = d.config.get("memory", "embedding_model") {
        let s = s.trim();
        if !s.is_empty() {
            return ferb_fs::pypath::expanduser(s, &d.opts.user_home.to_string_lossy()).into();
        }
    }
    match &d.opts.app_bundle {
        Some(b) => ferb_memory::embeddings::default_root(
            &b.join(ferb_device::install::BUNDLE_MODELS)
                .parent()
                .map(std::path::Path::to_path_buf)
                .unwrap_or_default(),
        ),
        None => ferb_memory::embeddings::default_root(&d.opts.home),
    }
}

/// Starts the memory's background work.
pub fn start(d: &Arc<Daemon>) {
    let stop = Arc::new(AtomicBool::new(false));
    let _ = d.memory_stop.set(stop.clone());
    reindex(d, stop.clone());
    embedder(d, stop);
    let a = d.a.clone();
    let j = d.journal.clone();
    let device = device_id(d);
    let root = embedding_root(d);
    let _ = a.spawn(move |st| async move {
        loop {
            memory_pass(&st, &j, &device, &root).await;
            if st.shutting_down.get() {
                return;
            }
            tokio::time::sleep(Duration::from_secs(PASS_EVERY_S)).await;
        }
    });
}

fn device_id(d: &Daemon) -> String {
    match d.config.get("device", "id") {
        Ok(Some(Value::String(s))) if !s.is_empty() => s,
        _ => "dev".into(),
    }
}

/// `_reindex_search`: in a thread of its own on its own connection, in short batches; journaled only when nodes
/// were re-indexed; a failure is journaled and search stays as it is until the next start.
fn reindex(d: &Arc<Daemon>, stop: Arc<AtomicBool>) {
    let path = crate::basics::lock(&d.db).path().to_path_buf();
    let (j, r) = (d.journal.clone(), d.redactor.clone());
    std::thread::Builder::new()
        .name("ferb-reindex".into())
        .spawn(move || {
            let work = || -> ferb_learning::Result<(bool, bool)> {
                let db = ferb_store::Db::open(&path)
                    .map_err(|e| ferb_learning::Error::Sql("OperationalError".into(), e.to_string()))?;
                let _ = db.conn().execute_batch("PRAGMA busy_timeout = 60000");
                let store = ferb_learning::store::Store::new(Rc::new(db));
                let did = ferb_learning::learning::reindex_search(
                    &store,
                    ferb_learning::notes::REINDEX_BATCH_BYTES,
                    &mut || stop.load(Ordering::SeqCst),
                )?;
                Ok((did, ferb_learning::learning::search_indexed(&store)?))
            };
            match work() {
                Ok((true, finished)) => j.note(
                    "memory.search_reindexed",
                    json!({"version": ferb_learning::notes::SEARCH_TEXT_VERSION, "finished": finished}),
                ),
                Ok(_) => {}
                Err(e) => j.note(
                    "memory.search_reindex_failed",
                    json!({"error": r.text(&format!("{}: {e}", e.class()))}),
                ),
            }
        })
        .expect("the re-index thread");
}

/// The local model held by the embed thread, lent to group A's learning store (its `Rc<dyn Embedder>`).
struct SharedModel(Arc<ferb_memory::embeddings::StaticEmbedder>);

impl ferb_memory::embeddings::Embedder for SharedModel {
    fn name(&self) -> &str {
        self.0.name()
    }
    fn dimensions(&self) -> usize {
        self.0.dimensions()
    }
    fn encode(&self, text: &str) -> Option<Vec<i8>> {
        self.0.encode(text)
    }
}

/// `_embed_loop` on a thread of its own: the local model once (a failure journaled; search then runs on keywords
/// and recency), then new text embedded every EMBED_EVERY_S on its own connection; a pass of a batch or more is
/// journaled with its cost.
fn embedder(d: &Arc<Daemon>, stop: Arc<AtomicBool>) {
    let root = embedding_root(d);
    let a = d.a.clone();
    let path = crate::basics::lock(&d.db).path().to_path_buf();
    let (j, r, clock) = (d.journal.clone(), d.redactor.clone(), d.opts.clock.clone());
    std::thread::Builder::new()
        .name("ferb-embed".into())
        .spawn(move || {
            let model = match ferb_memory::embeddings::StaticEmbedder::open(
                &root,
                &ferb_memory::embeddings::model(),
            ) {
                Ok(m) => {
                    // `self.learning.embedder = StaticEmbedder(...)` (`_load_embedder`): the queries' vectors (the
                    // prompt's memory, chat.history, memory.read, memory.learned) come from the same model, shared
                    let m = Arc::new(m);
                    let shared = m.clone();
                    let _ = a.call_blocking(move |st: std::rc::Rc<ferb_daemon::areas_a::State>| async move {
                        st.learning.set_embedder(std::rc::Rc::new(SharedModel(shared)));
                    });
                    m
                }
                Err(e) => {
                    let why: String = r.text(&format!("{e:?}")).chars().take(300).collect();
                    j.note(
                        "memory.embedding_model",
                        json!({"state": "unavailable", "error": why}),
                    );
                    return;
                }
            };
            struct Now(Arc<dyn ferb_clock::clock::ZonedClock>);
            impl ferb_memory::search::Clock for Now {
                fn now(&self) -> ferb_memory::times::DateTime {
                    ferb_daemon::areas_a::to_py(&self.0.now())
                }
            }
            let now = Now(clock);
            while !stop.load(Ordering::SeqCst) {
                let started = std::time::Instant::now();
                let pass = (|| -> Result<Value, String> {
                    let db = ferb_store::Db::open(&path).map_err(|e| e.to_string())?;
                    let _ = db.conn().execute_batch("PRAGMA busy_timeout = 60000");
                    use ferb_memory::embeddings::Embedder;
                    let mut search = ferb_memory::search::MemorySearch::new(
                        db.conn(),
                        &now,
                        Some(model.name().to_string()),
                    );
                    let out = search
                        .embed_pending(
                            model.as_ref(),
                            &ferb_learning::notes::search_text,
                            &|| stop.load(Ordering::SeqCst),
                            &mut || std::thread::sleep(Duration::from_secs_f64(EMBED_PAUSE_S)),
                        )
                        .map_err(|e| format!("{e:?}"))?;
                    Ok(json!({"embedded": out.embedded, "batches": out.batches,
                              "encode_cpu_s": out.encode_cpu_s, "removed": out.removed,
                              "wall_s": (started.elapsed().as_secs_f64() * 1000.0).round() / 1000.0}))
                })();
                match pass {
                    Ok(out) => {
                        if out["embedded"].as_u64().unwrap_or(0)
                            >= ferb_memory::search::EMBED_BATCH as u64
                        {
                            j.note("memory.embedded", out);
                        }
                    }
                    Err(e) => j.note("memory.embed_failed", json!({"error": r.text(&e)})),
                }
                // the wait is cut short by a stop (checked each second)
                for _ in 0..EMBED_EVERY_S {
                    if stop.load(Ordering::SeqCst) {
                        return;
                    }
                    std::thread::sleep(Duration::from_secs(1));
                }
            }
        })
        .expect("the embedder thread");
}

/// `_memory_pass`: one hourly pass on group A's thread. Each job has its own error scope (L16).
pub async fn memory_pass(
    st: &Rc<State>,
    journal: &crate::basics::Journal,
    device: &str,
    model_root: &std::path::Path,
) {
    let make = st.make_runtime.clone();
    let make_rt = move |a: &str, b: &str, c: &[String], d: &Value| -> ferb_learning::Result<Made> {
        match &make {
            Some(m) => m(a, b, c, d),
            None => Err(ferb_learning::Error::Runtime(
                "no model runtime is set up for the learners".into(),
            )),
        }
    };
    let fail = |kind: &str, e: ferb_learning::Error| {
        journal.note(kind, json!({"error": format!("{}: {e}", e.class())}));
    };
    // distill: every past day not done yet, then the summarized days purged
    {
        let w = Rc::downgrade(st);
        let distiller = ferb_learning::distill::Distiller {
            store: st.store.clone(),
            journal: st.journal.clone(),
            memory: st.memory.clone(),
            make_runtime: Box::new(make_rt.clone()),
            clock: st.learning.clock.clone(),
            zone: st.learning.zone.clone(),
            prompts: st.prompts.clone(),
            summary: Rc::new(SummarySeams),
            purge: Box::new(|| {}),
            device_id: device.to_string(),
            record_usage: Box::new(|_, _| {}),
            spans: None,
            tell: Box::new(move |n| {
                if let Some(st) = w.upgrade() {
                    let fut = st
                        .owner
                        .tell_owner(&n.id, n.refs, &n.text, &n.origin, n.cards);
                    tokio::task::spawn_local(fut);
                }
            }),
        };
        if let Err(e) = distiller.catch_up().await {
            fail("memory.distill_failed", e);
        }
    }
    // the episode cards whose inputs changed or that were never captured, one at a time, only while no task runs
    if let Err(e) = refresh_cards(st, journal).await {
        fail("memory.cards_failed", e);
    }
    // playbooks: only once the local model is there (it finds the candidates) and while no task runs
    if !st.tasks.running() {
        if let Ok(model) = ferb_memory::embeddings::StaticEmbedder::open(
            model_root,
            &ferb_memory::embeddings::model(),
        ) {
            let j = st.journal.clone();
            let merged = ferb_learning::playbooks::merge(
                &st.learning,
                &make_rt,
                &st.prompts,
                Some(&model),
                device,
                None,
                &move |kind: &str, data: Value| j.append(kind, Some(data), None).map(|_| ()),
                &|_, _| {},
            )
            .await;
            if let Err(e) = merged {
                fail("memory.playbooks_failed", e);
            }
        }
    }
    // the weekly core-profile pass, when due
    if let Err(e) = areas_a::memory::profile_pass(st, false).await {
        fail("memory.profile_failed", e);
    }
    // `_refresh_mac_profile` (MEM-13): weekly, with the profile pass's cadence, what FERB saw on this Mac is gathered
    // again and interpreted again only when it changed; quiet
    if let Err(e) = areas_a::persona::refresh(st).await {
        journal.note("memory.mac_profile_failed", json!({"error": e}));
    }
}

/// `_refresh_cards`: each card to read is captured again (`_capture_card`: its reads off the loop), one at a time and
/// only while no task runs; one that can't be built is tried once more, then waits for its inputs to change
/// (MEM-422); then the day summaries are indexed for search.
async fn refresh_cards(
    st: &Rc<State>,
    journal: &crate::basics::Journal,
) -> ferb_learning::Result<()> {
    let l = &st.learning;
    for task_id in l.cards_to_read()? {
        if st.tasks.running() {
            return Ok(()); // a task started: the next pass reads the rest
        }
        match areas_a::cards::capture_card(st, &task_id, None, false).await {
            Ok(_) => {
                let read: bool = st
                    .store
                    .db
                    .conn()
                    .query_row(
                        "SELECT 1 FROM learning_card_inputs WHERE task_id=?",
                        [&task_id],
                        |_| Ok(true),
                    )
                    .unwrap_or(false);
                if !read {
                    let inputs = l
                        .card_inputs(Some(&task_id))?
                        .get(&task_id)
                        .cloned()
                        .unwrap_or_default();
                    l.card_read(&task_id, &inputs, None)?;
                }
            }
            Err(areas_a::cards::CardError::Refused(why)) => {
                let why = if why.is_empty() {
                    "refused".to_string()
                } else {
                    st.redact_text(&why)
                };
                let inputs = l
                    .card_inputs(Some(&task_id))?
                    .get(&task_id)
                    .cloned()
                    .unwrap_or_default();
                let status = l.card_read(&task_id, &inputs, Some(&why))?;
                journal.note(
                    "memory.cards_failed",
                    json!({"task_id": task_id, "error": why,
                           "next": if status == "parked" { "waits until its task changes" } else { "tried once more next hour" }}),
                );
            }
            // FERB is stopping: the next start's pass reads the rest
            Err(areas_a::cards::CardError::Deferred) => return Ok(()),
            Err(areas_a::cards::CardError::Failed(e)) => return Err(e),
        }
        tokio::task::yield_now().await;
    }
    ferb_learning::learning::index_summaries(l, &SummarySeams, &mut || false)
}
