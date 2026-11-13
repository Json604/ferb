//! The adapters' common shape and the `ferb_core` contract on top of it.
//!
//! Python's `AgentRuntime` (ferb/core/interfaces.py) hands back a `RunHandle` whose `state` holds the live process,
//! and `events(run)` is an async generator the caller pulls. In Rust a run owns a child process, its readers and
//! pending tool waits, which a JSON `state` can't hold, so an adapter's `start` returns [`Started`]: the handle (its
//! `state` keeps only facts such as `start_steps`), the pulled [`EventStream`], and a [`RunControl`] for steering
//! and cancel that works while the stream is being read.
//!
//! [`CoreRuntime`] implements `ferb_core::interfaces::AgentRuntime` over any [`Adapter`]: it keeps each run's stream
//! and control by run id and pumps the stream into the channel `events` returns. That channel carries events only,
//! so what Python's generator would have raised arrives as an `error` event with `raised` set to the class name
//! (a difference the core trait's shape forces; see the crate's journal entry).

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use ferb_core::errors::FerbError;
use ferb_core::interfaces::{AgentRuntime, BoxFuture};
use ferb_core::types::{ModelRequest, RunHandle, RuntimeEvent, RuntimeEventKind, Task};
use serde_json::{Map, Value};
use tokio::sync::mpsc;

use crate::error::RuntimeError;
use crate::gen::EventStream;

/// Steering and cancel for one live run.
pub trait RunControl: Send + Sync {
    /// Python `continue_`: same-run input accepted, or `Unsupported` when the run can't take it.
    fn steer<'a>(&'a self, message: &'a str) -> BoxFuture<'a, Result<(), RuntimeError>>;
    /// Ends the run: its process killed and reaped, its readers stopped, its files removed.
    fn cancel(&self) -> BoxFuture<'_, ()>;
}

pub struct Started {
    pub handle: RunHandle,
    pub events: EventStream,
    pub control: Arc<dyn RunControl>,
}

/// One runtime adapter, as the orchestrator builds it for one call.
pub trait Adapter: Send + Sync {
    fn name(&self) -> &str;
    fn start<'a>(
        &'a self,
        task: &'a Task,
        request: &'a ModelRequest,
        run_token: &'a str,
    ) -> BoxFuture<'a, Result<Started, RuntimeError>>;
}

/// A runtime event from kind and data.
pub fn event(kind: RuntimeEventKind, data: Value) -> RuntimeEvent {
    let data = match data {
        Value::Object(m) => m,
        _ => Map::new(),
    };
    RuntimeEvent { kind, data }
}

/// A started run's stream (until `events` takes it) and control.
type Live = (Option<EventStream>, Arc<dyn RunControl>);

/// `ferb_core::interfaces::AgentRuntime` over an [`Adapter`].
pub struct CoreRuntime<A: Adapter> {
    adapter: A,
    runs: Mutex<HashMap<String, Live>>,
}

impl<A: Adapter> CoreRuntime<A> {
    pub fn new(adapter: A) -> Self {
        CoreRuntime {
            adapter,
            runs: Mutex::new(HashMap::new()),
        }
    }

    fn control(&self, run: &RunHandle) -> Option<Arc<dyn RunControl>> {
        self.runs
            .lock()
            .ok()
            .and_then(|r| r.get(&run.id).map(|(_, c)| c.clone()))
    }
}

impl<A: Adapter> AgentRuntime for CoreRuntime<A> {
    fn name(&self) -> &str {
        self.adapter.name()
    }

    fn start<'a>(
        &'a self,
        task: &'a Task,
        request: &'a ModelRequest,
        run_token: &'a str,
    ) -> BoxFuture<'a, Result<RunHandle, FerbError>> {
        Box::pin(async move {
            let started = self
                .adapter
                .start(task, request, run_token)
                .await
                .map_err(|e| e.to_ferb())?;
            if let Ok(mut runs) = self.runs.lock() {
                runs.insert(
                    started.handle.id.clone(),
                    (Some(started.events), started.control),
                );
            }
            Ok(started.handle)
        })
    }

    fn events(&self, run: &RunHandle) -> mpsc::Receiver<RuntimeEvent> {
        let (tx, rx) = mpsc::channel(1);
        let stream = self
            .runs
            .lock()
            .ok()
            .and_then(|mut r| r.get_mut(&run.id).and_then(|(s, _)| s.take()));
        if let Some(mut stream) = stream {
            tokio::spawn(async move {
                while let Some(item) = stream.next().await {
                    let ev = match item {
                        Ok(ev) => ev,
                        Err(e) => event(
                            RuntimeEventKind::Error,
                            serde_json::json!({"message": e.to_string(), "raised": e.class()}),
                        ),
                    };
                    if tx.send(ev).await.is_err() {
                        stream.close().await;
                        return;
                    }
                }
            });
        }
        rx
    }

    fn continue_run<'a>(
        &'a self,
        run: &'a RunHandle,
        message: &'a str,
    ) -> BoxFuture<'a, Result<(), FerbError>> {
        Box::pin(async move {
            match self.control(run) {
                Some(c) => c.steer(message).await.map_err(|e| e.to_ferb()),
                None => Err(FerbError::Unsupported("the run has already ended".into())),
            }
        })
    }

    fn cancel<'a>(&'a self, run: &'a RunHandle) -> BoxFuture<'a, ()> {
        Box::pin(async move {
            let control = self
                .runs
                .lock()
                .ok()
                .and_then(|mut r| r.remove(&run.id).map(|(_, c)| c));
            if let Some(c) = control {
                c.cancel().await;
            }
        })
    }
}
