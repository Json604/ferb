//! The adapters as ferb-worker's runtime interface (`ModelRuntime`, `EventSource`, `RunControl`), and what an adapter
//! raises as the worker's `Error` with every fact it marked: the glue a daemon would otherwise write by hand.
//!
//! The conversion keeps Python's class and attributes (worker.py reads `account`, `reason`, `overflow`, `image` by
//! `getattr`; supervisor.py:2525 reads `account` off any failure): `ProtocolError`, `AccountError` and `ProviderError`
//! arrive as [`ferb_worker::errors::AdapterError`] with their facts; FERB's own errors as themselves
//! (`RuntimeUnavailable` as the worker's `Unavailable`); `SignedOut` as itself; anything else Python let escape by
//! its class and words.

use std::sync::Arc;

use ferb_core::errors::FerbError;
use ferb_core::types::{ModelLimits, ModelRequest, RuntimeEvent, Task};
use ferb_orchestrator::steering::ContinueError;
use ferb_worker::errors::{AdapterClass, AdapterError};
use ferb_worker::ports::{BoxFuture, EventSource, ModelRuntime, StartedRun};
use serde_json::Value;

use crate::error::RuntimeError;
use crate::gen::EventStream;
use crate::run::{Adapter, RunControl};

impl From<RuntimeError> for ferb_worker::Error {
    fn from(e: RuntimeError) -> ferb_worker::Error {
        let message = e.to_string();
        match e {
            RuntimeError::Ferb(f) => ferb_worker::Error::from(f),
            RuntimeError::SignedOut { message, runtime } => {
                ferb_worker::Error::SignedOut { message, runtime }
            }
            RuntimeError::Account { ref reason, code } => {
                let mut a = AdapterError::new(AdapterClass::Account, message);
                a.account = e.account().to_string();
                a.reason = reason.clone();
                a.code = code;
                ferb_worker::Error::Adapter(a)
            }
            RuntimeError::Protocol {
                message,
                overflow,
                account,
                empty,
            } => {
                let mut a = AdapterError::new(AdapterClass::Protocol, message);
                a.overflow = overflow;
                a.account = account;
                a.empty = empty;
                ferb_worker::Error::Adapter(a)
            }
            RuntimeError::Provider {
                message,
                overflow,
                account,
                image,
                status,
            } => {
                let mut a = AdapterError::new(AdapterClass::Provider, message);
                a.overflow = overflow;
                a.account = account;
                a.image = image;
                a.status = status;
                ferb_worker::Error::Adapter(a)
            }
            RuntimeError::Raised(r) => ferb_worker::Error::other(&r.class, r.message),
        }
    }
}

/// `continue_`'s failure as steering reads it: `Unsupported` leaves the update pending; anything else by its class.
pub fn continue_error(e: &RuntimeError) -> ContinueError {
    match e {
        RuntimeError::Ferb(FerbError::Unsupported(_)) => ContinueError::Unsupported,
        other => ContinueError::Failed(other.class()),
    }
}

/// A started run's events for the worker: what the stream raised arrives as the worker's error.
pub struct WorkerEvents(pub EventStream);

impl EventSource for WorkerEvents {
    fn next(&mut self) -> BoxFuture<'_, Option<Result<RuntimeEvent, ferb_worker::Error>>> {
        Box::pin(async move { self.0.next().await.map(|i| i.map_err(Into::into)) })
    }
}

/// A run's steering and cancel for the worker.
pub struct WorkerControl(pub Arc<dyn RunControl>);

impl ferb_worker::ports::RunControl for WorkerControl {
    fn steer<'a>(&'a self, message: &'a str) -> BoxFuture<'a, Result<(), ContinueError>> {
        Box::pin(async move { self.0.steer(message).await.map_err(|e| continue_error(&e)) })
    }

    fn cancel(&self) -> BoxFuture<'_, ()> {
        self.0.cancel()
    }
}

/// One built adapter as the worker's `ModelRuntime`, with the two facts the worker reads off it by attribute: its
/// documented limits (`runtime_limits(rt)`) and the schema it checks its answer against (`output_schema`).
pub struct WorkerRuntime {
    pub adapter: Arc<dyn Adapter>,
    /// `catalog::limits(name, model, home)` for this build, or `ModelLimitsUnknown`.
    pub limits: Result<ModelLimits, FerbError>,
    pub output_schema: Option<Value>,
}

impl ModelRuntime for WorkerRuntime {
    fn name(&self) -> String {
        self.adapter.name().to_string()
    }

    fn limits(&self) -> Result<ModelLimits, FerbError> {
        self.limits.clone()
    }

    fn output_schema(&self) -> Option<Value> {
        self.output_schema.clone()
    }

    fn start<'a>(
        &'a self,
        task: &'a Task,
        request: &'a ModelRequest,
        run_token: &'a str,
    ) -> BoxFuture<'a, Result<StartedRun, ferb_worker::Error>> {
        Box::pin(async move {
            let started = self.adapter.start(task, request, run_token).await?;
            Ok(StartedRun {
                handle: started.handle,
                events: Box::new(WorkerEvents(started.events)),
                control: Arc::new(WorkerControl(started.control)),
            })
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_adapters_error_reaches_the_worker_with_its_class_and_facts() {
        let e: ferb_worker::Error = RuntimeError::account_error("auth_required", Some(7)).into();
        assert_eq!(e.class(), "AccountError", "Python's class");
        assert!(e.is_runtime_unavailable() && e.is_value_error());
        assert_eq!(e.account(), "signed_out");
        assert!(
            matches!(&e, ferb_worker::Error::Adapter(a) if a.reason == "auth_required" && a.code == Some(7))
        );
        assert_eq!(
            e.to_string(),
            RuntimeError::account_error("auth_required", Some(7)).to_string()
        );
        let e: ferb_worker::Error = RuntimeError::SignedOut {
            message: "Claude isn't signed in".into(),
            runtime: "claude".into(),
        }
        .into();
        assert_eq!(
            (e.class().as_str(), e.account()),
            ("SignedOut", "signed_out")
        );
        let e: ferb_worker::Error = RuntimeError::Protocol {
            message: "turn failed".into(),
            overflow: true,
            account: "usage_limit".into(),
            empty: false,
        }
        .into();
        assert_eq!(
            (
                e.class().as_str(),
                e.is_value_error(),
                e.overflow(),
                e.account()
            ),
            ("ProtocolError", true, true, "usage_limit")
        );
        assert_eq!(e.to_string(), "turn failed");
        let e: ferb_worker::Error = RuntimeError::Provider {
            message: "image refused".into(),
            overflow: false,
            account: String::new(),
            image: true,
            status: 400,
        }
        .into();
        assert_eq!(
            (e.class().as_str(), e.is_value_error(), e.image()),
            ("ProviderError", false, true),
            "a ProviderError is an Exception, not a ValueError"
        );
        let e: ferb_worker::Error = RuntimeError::unsupported("no").into();
        assert_eq!(e.class(), "Unsupported");
        let e: ferb_worker::Error = FerbError::RuntimeUnavailable("down".into()).into();
        assert_eq!(e.class(), "RuntimeUnavailable");
        let e: ferb_worker::Error =
            RuntimeError::Raised(crate::error::Raised::key_error("x")).into();
        assert_eq!(
            (e.class().as_str(), e.to_string().as_str()),
            ("KeyError", "'x'")
        );
    }

    #[test]
    fn steering_reads_unsupported_and_other_failures_by_class() {
        assert_eq!(
            continue_error(&RuntimeError::unsupported("closed")),
            ContinueError::Unsupported
        );
        assert_eq!(
            continue_error(&RuntimeError::protocol("x")),
            ContinueError::Failed("ProtocolError".into())
        );
    }
}
