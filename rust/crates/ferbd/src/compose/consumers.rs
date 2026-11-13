//! compose.py `_wire_events` (ferb-daemon `areas_b::compose::wire_events`): the consumers of published events beyond
//! the three the supervisor's seam registers itself (the plans that wait for an event, the bus, `_question_record`,
//! compose/supervisor.rs): the day's activity, a delegated conversation's observation, a question's banner when no
//! chat shows it (L9), a refused sealed value told (GATE-562), and a task's pause and resume (ORCH-312).

use std::sync::Arc;

use serde_json::{Map, Value};

use super::desktop_seams::DesktopSeams;

/// Registers each consumer on the daemon's events.
pub fn wire(
    events: &crate::basics::Events,
    notifier: Arc<ferb_daemon::areas_b::notify::Notifier>,
    sup: ferb_supervisor::flow::Supervisor,
    env: Arc<super::env_seams::EnvSeams>,
    host: ferb_daemon::areas_b::core::Host,
    device_id: String,
    desktop: Arc<DesktopSeams>,
) {
    // compose.py:225-229: each trail event and action observation into the day's activity (SENSE-1: accessibility
    // rows stay in the trail only), named with the device
    {
        let device = Arc::new(ferb_daemon::areas_b::compose::ActivityDevice::default());
        // the rows are written in order on a thread of their own: the file's write never holds up the publish that
        // brought the event (a tool call's, a chat turn's)
        let (rows, written) = std::sync::mpsc::channel::<Map<String, Value>>();
        let _ = std::thread::Builder::new()
            .name("ferb-activity".into())
            .spawn(move || {
                while let Ok(row) = written.recv() {
                    let _ = env.activity.record(&row);
                }
            });
        let rows = std::sync::Mutex::new(rows);
        events.events.on(
            Some(&["trail.event", "action.observed"]),
            Arc::new(move |topic: &str, data: Map<String, Value>| {
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
                row.insert("device_id".into(), device.get(&host, &device_id));
                let _ = crate::basics::lock(&rows).send(row);
            }),
        );
    }
    // compose.py:230: a trail event a delegated conversation is waiting for
    {
        let sup = sup.clone();
        events.events.on(
            Some(&["trail.event"]),
            Arc::new(move |_topic: &str, data: Map<String, Value>| {
                let _ = sup.inner().conversations().observe(&Value::Object(data));
            }),
        );
    }
    // compose.py:231: `perception.attention` goes to `_attention_arrived`, which hands it to Jev; Jev is off (notices
    // paused), so nothing publishes it
    crate::off::off("events.attention_arrived", ());
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
