//! compose.py `_wire_events` (ferb-daemon `areas_b::compose::wire_events`): who hears a published event, in Python's
//! order. Python's `Events.publish` calls its consumers one after another inside the publish, in the order they were
//! added: the plans that wait for an event, the day's activity, a delegated conversation's observation, the bus,
//! a question's banner when no chat shows it (L9), a refused sealed value told (GATE-562), `_question_record`, a
//! task's pause and resume (ORCH-312). The first three are added here before the bus ([`wire_first`]) and take their
//! parts once those exist; the bus is added next (compose/mod.rs), then the supervisor's seam adds the turn's bus and
//! `_question_record` (compose/supervisor.rs), then [`wire`] the rest.

use std::sync::{Arc, OnceLock, Weak};

use serde_json::{Map, Value};

use super::desktop_seams::DesktopSeams;

/// What the consumers added before the bus take once it exists.
#[derive(Default)]
pub struct First {
    /// The supervisor's seam (`self.supervisor` is set once its supervisor is built): set when the seam is made.
    pub daemon: OnceLock<Weak<super::supervisor::SupDaemon>>,
    /// The day's activity and the conversations' store, set by [`wire`].
    later: OnceLock<Later>,
}

struct Later {
    sup: ferb_supervisor::flow::Supervisor,
    env: Arc<super::env_seams::EnvSeams>,
    host: ferb_daemon::areas_b::core::Host,
    device_id: String,
    device: ferb_daemon::areas_b::compose::ActivityDevice,
}

/// compose.py:217-230, the consumers Python runs before the bus: added first, so each one's effects (a plan woken, the
/// activity row written, a conversation's evidence and its own `conversation.paused`) come before the bus hands the
/// event on, as in Python.
pub fn wire_first(events: &crate::basics::Events) -> Arc<First> {
    let first = Arc::new(First::default());
    // compose.py:213 `plans`: a plan step (or loop) waiting for this event
    {
        let first = Arc::downgrade(&first);
        events.events.on(
            None,
            Arc::new(move |topic: &str, data: Map<String, Value>| {
                let sup = first
                    .upgrade()
                    .and_then(|f| f.daemon.get().and_then(Weak::upgrade))
                    .and_then(|d| d.sup.get().cloned());
                if let Some(sup) = sup {
                    sup.inner().notify_plan_event(topic, &Value::Object(data));
                }
            }),
        );
    }
    // compose.py:225-229: each trail event and action observation into the day's activity (SENSE-1: accessibility
    // rows stay in the trail only), named with the device; written inside the publish, as Python's consumer writes it
    // before `publish` returns: the row is in the file (its `when` read from the clock) before the publisher goes on
    {
        let first = Arc::downgrade(&first);
        events.events.on(
            Some(&["trail.event", "action.observed"]),
            Arc::new(move |topic: &str, data: Map<String, Value>| {
                let Some(first) = first.upgrade() else {
                    return;
                };
                let Some(l) = first.later.get() else {
                    return;
                };
                if data.get("source") == Some(&Value::from("accessibility")) {
                    return;
                }
                let what = match data.get("what") {
                    Some(w) => w.clone(),
                    None => data
                        .get("source")
                        .cloned()
                        .unwrap_or_else(|| Value::from(topic)),
                };
                let mut row = data;
                row.insert("what".into(), what);
                row.insert("device_id".into(), l.device.get(&l.host, &l.device_id));
                let _ = l.env.activity.record(&row);
            }),
        );
    }
    // compose.py:230: a trail event a delegated conversation is waiting for
    {
        let first = Arc::downgrade(&first);
        events.events.on(
            Some(&["trail.event"]),
            Arc::new(move |_topic: &str, data: Map<String, Value>| {
                if let Some(l) = first.upgrade().as_deref().and_then(|f| f.later.get()) {
                    let _ = l.sup.inner().conversations().observe(&Value::Object(data));
                }
            }),
        );
    }
    // compose.py:231: `perception.attention` goes to `_attention_arrived`, which hands it to Jev; Jev is off (notices
    // paused), so nothing publishes it
    crate::off::off("events.attention_arrived", ());
    first
}

/// Gives the first consumers their parts and adds the ones Python runs after the bus.
#[allow(clippy::too_many_arguments)] // the parts the consumers hand events to
pub fn wire(
    events: &crate::basics::Events,
    first: &First,
    notifier: Arc<ferb_daemon::areas_b::notify::Notifier>,
    sup: ferb_supervisor::flow::Supervisor,
    env: Arc<super::env_seams::EnvSeams>,
    host: ferb_daemon::areas_b::core::Host,
    device_id: String,
    desktop: Arc<DesktopSeams>,
) {
    let _ = first.later.set(Later {
        sup,
        env,
        host,
        device_id,
        device: ferb_daemon::areas_b::compose::ActivityDevice::default(),
    });
    // compose.py:238 `_approval_notice`: a question's banner when no FERB client shows it
    {
        let n = notifier.clone();
        events.events.on(
            Some(&["approval.requested"]),
            Arc::new(move |_topic: &str, data: Map<String, Value>| {
                if let Some((banner, urgent)) = n.approval_notice(&Value::Object(data)) {
                    let n = n.clone();
                    if let Ok(rt) = tokio::runtime::Handle::try_current() {
                        rt.spawn(async move { n.notify(banner, urgent).await });
                    }
                }
            }),
        );
    }
    // compose.py:239 (GATE-562): a page asked again for a value FERB filled sealed
    {
        let n = notifier;
        events.events.on(
            Some(&["secret.refused"]),
            Arc::new(move |_topic: &str, data: Map<String, Value>| {
                let n = n.clone();
                if let Ok(rt) = tokio::runtime::Handle::try_current() {
                    rt.spawn(async move {
                        let _ = n.tell_refused(&data).await;
                    });
                }
            }),
        );
    }
    // compose.py:244 (ORCH-312)
    super::desktop_seams::on_task_events(desktop, events);
}
