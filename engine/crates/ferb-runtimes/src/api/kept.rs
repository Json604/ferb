//! T-1343 (kb/plans/front-door.md, "Keeping the runtime ready"): the API loop's connection kept between messages.
//!
//! What is kept is the HTTP client with its pool of open connections to the provider, so the next message skips the
//! connection and TLS set-up; an unused connection stays open up to the keeper's 5 minutes (the provider may close it
//! sooner, and the next request then opens one as today). Nothing is sent ahead of a message: no request, no key.
//! Each message is today's run (its key read, its tools, its token) on the kept client; the client goes back to the
//! keeper once the run was closed.

use std::any::Any;
use std::sync::{Arc, Mutex};

use ferb_core::interfaces::BoxFuture;
use ferb_core::types::{ModelRequest, Task};
use serde_json::{json, Map, Value};

use super::{http_client, start, ApiRuntime};
use crate::error::RuntimeError;
use crate::keep::{Back, Keepable, Ready, IDLE_S};
use crate::run::{RunControl, Started};

/// A provider's HTTP client with its open connections.
pub struct ApiReady {
    settings: String,
    client: reqwest::Client,
}

impl Ready for ApiReady {
    fn settings(&self) -> &str {
        &self.settings
    }

    fn alive(&self) -> bool {
        true
    }

    fn end(self: Arc<Self>) -> BoxFuture<'static, ()> {
        Box::pin(async move { drop(self) })
    }

    fn as_any(self: Arc<Self>) -> Arc<dyn Any + Send + Sync> {
        self
    }
}

/// One message's run on a kept client: today's control, and the client back to the keeper when it is closed.
struct ApiLease {
    inner: Arc<dyn RunControl>,
    ready: Mutex<Option<Arc<ApiReady>>>,
    back: Back,
}

impl RunControl for ApiLease {
    fn steer<'a>(&'a self, message: &'a str) -> BoxFuture<'a, Result<(), RuntimeError>> {
        self.inner.steer(message)
    }

    fn cancel(&self) -> BoxFuture<'_, ()> {
        Box::pin(async move {
            self.inner.cancel().await;
            let ready = self.ready.lock().unwrap_or_else(|e| e.into_inner()).take();
            if let Some(r) = ready {
                (self.back)(r);
            }
        })
    }

    fn unreported_usage(&self) -> Vec<Map<String, Value>> {
        self.inner.unreported_usage()
    }
}

impl Keepable for ApiRuntime {
    /// A supervisor run: the provider, its model, the request limit and where requests go.
    fn keep_settings(&self, _request: Option<&ModelRequest>, _has_token: bool) -> Option<String> {
        if !self.supervisor {
            return None;
        }
        Some(ferb_py::json::dumps(&json!({
            "runtime": self.name(), "model": self.provider.model(), "timeout": self.timeout_s,
            "origin": self.origin,
        })))
    }

    fn prepare<'a>(
        &'a self,
        settings: &'a str,
        _back: Back,
    ) -> BoxFuture<'a, Result<Arc<dyn Ready>, RuntimeError>> {
        Box::pin(async move {
            Ok(Arc::new(ApiReady {
                settings: settings.to_string(),
                client: http_client(self.timeout_s, Some(IDLE_S))?,
            }) as Arc<dyn Ready>)
        })
    }

    fn take<'a>(
        &'a self,
        ready: Arc<dyn Ready>,
        back: Back,
        task: &'a Task,
        request: &'a ModelRequest,
        run_token: &'a str,
    ) -> BoxFuture<'a, Result<Started, RuntimeError>> {
        Box::pin(async move {
            let Ok(ready) = ready.clone().as_any().downcast::<ApiReady>() else {
                back(ready);
                return Err(RuntimeError::protocol("not an API client"));
            };
            let mut rt = self.clone();
            rt.client = Some(ready.client.clone());
            match start(&rt, task, request, run_token).await {
                Ok(mut started) => {
                    started.handle.state.insert("kept".into(), json!("ready"));
                    started.control = Arc::new(ApiLease {
                        inner: started.control,
                        ready: Mutex::new(Some(ready)),
                        back,
                    });
                    Ok(started)
                }
                Err(e) => {
                    back(ready);
                    Err(e)
                }
            }
        })
    }
}
