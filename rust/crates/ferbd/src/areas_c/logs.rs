//! `logs.subscribe` (rpc.py:587, `ferb logs`): the daemon's events as they are published, by filter, until the caller
//! leaves. A subscriber that falls behind on critical events gets `bus.overflow` and the stream ends; dropped
//! telemetry is told as a count (`bus.dropped`).

use ferb_core::bus::{BusError, Subscription, DROPPED};
use ferb_core::interfaces::BoxFuture;
use ferb_rpc::server::{EventStream, HandlerResult};
use ferb_rpc::{HandlerError, Params, Server, Session};
use serde_json::{json, Map, Value};

use super::Deps;

/// `LOG_FILTERS` (rpc.py:26): `ferb logs` filter flags -> bus topic patterns.
pub const LOG_FILTERS: [(&str, &[&str]); 7] = [
    ("tasks", &["task.*"]),
    ("trail", &["trail.event"]),
    ("tools", &["tool.*"]),
    ("browsers", &["browser.*"]),
    ("policy", &["approval.*", "stop.*", "security.*"]),
    ("actions", &["action.*"]),
    ("timers", &["timer.*"]),
];

pub fn register(server: &Server, d: &Deps) {
    let d = d.clone();
    server.register_stream("logs.subscribe", move |_s: Session, p: Params| open(&d, &p));
}

/// One event as the stream writes it.
fn line(
    d: &Deps,
    topic: &str,
    at: &ferb_core::when::DateTime,
    class: &str,
    data: &Map<String, Value>,
) -> Map<String, Value> {
    crate::basics::object(
        json!({"topic": topic, "at": ferb_core::when::isoformat(at), "class": class,
                                 "data": d.redactor.value(&Value::Object(data.clone()))}),
    )
}

struct Logs {
    d: Deps,
    sub: Option<Subscription>,
    refused: Option<HandlerError>,
}

impl Drop for Logs {
    fn drop(&mut self) {
        if let Some(s) = &self.sub {
            s.close();
        }
    }
}

impl EventStream for Logs {
    fn next(&mut self) -> BoxFuture<'_, Option<HandlerResult>> {
        Box::pin(async move {
            if let Some(e) = self.refused.take() {
                return Some(Err(e));
            }
            let sub = self.sub.as_ref()?;
            match sub.next().await {
                Ok(Some(ev)) => {
                    let class = if ev.topic == DROPPED {
                        "notice"
                    } else {
                        ev.class().as_str()
                    };
                    Some(Ok(line(&self.d, &ev.topic, &ev.at, class, &ev.data)))
                }
                Ok(None) => None,
                Err(BusError::Overflow) | Err(BusError::UnknownTopic(_)) => {
                    let mut data = Map::new();
                    data.insert(
                        "message".into(),
                        json!("fell behind on critical events; restart `ferb logs`"),
                    );
                    let out = line(
                        &self.d,
                        "bus.overflow",
                        &self.d.clock.now(),
                        "notice",
                        &data,
                    );
                    if let Some(s) = self.sub.take() {
                        s.close();
                    }
                    Some(Ok(out))
                }
            }
        })
    }
}

fn open(d: &Deps, p: &Params) -> Box<dyn EventStream> {
    let filters: Vec<String> = match p.get("filters") {
        Some(Value::Array(a)) => a.iter().map(ferb_py::value::py_str).collect(),
        Some(Value::String(s)) => s.chars().map(String::from).collect(),
        _ => Vec::new(),
    };
    let unknown: Vec<&str> = filters
        .iter()
        .map(String::as_str)
        .filter(|f| !LOG_FILTERS.iter().any(|(n, _)| n == f))
        .collect();
    if !unknown.is_empty() {
        // never "everything" for a filter FERB lacks
        let names: Vec<&str> = LOG_FILTERS.iter().map(|(n, _)| *n).collect();
        return Box::new(Logs {
            d: d.clone(),
            sub: None,
            refused: Some(HandlerError::refusal(
                "FerbError",
                format!(
                    "unknown log filter {}; use {}",
                    unknown.join(", "),
                    names.join(", ")
                ),
            )),
        });
    }
    let wanted: Vec<&str> = LOG_FILTERS
        .iter()
        .filter(|(n, _)| filters.iter().any(|f| f == n))
        .flat_map(|(_, pats)| pats.iter().copied())
        .collect();
    let maxsize = p
        .get("maxsize")
        .and_then(Value::as_u64)
        .map_or(ferb_core::bus::Bus::DEFAULT_MAXSIZE, |n| n as usize);
    let sub = d.bus.subscribe(
        if wanted.is_empty() {
            None
        } else {
            Some(&wanted)
        },
        maxsize,
    );
    Box::new(Logs {
        d: d.clone(),
        sub: Some(sub),
        refused: None,
    })
}
