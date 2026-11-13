//! A task's live previews (`ferb/daemon/task_preview.py`, built as `call_hooks.py:79` builds them): the gateway's
//! hooks declare each call's surface, FERB.app asks for frames (`tasks.preview`), the resource log counts them. A
//! capture takes the surface's locks only when they are free: the gateway's own `try_hold` with `during_reads`
//! (DMN-403, DMN-406), so a preview never queues behind, or ahead of, a worker's call. The frame goes through the
//! same image masking as every image (L13), off the runtime's threads.

use std::collections::BTreeSet;
use std::sync::{Arc, OnceLock};
use std::time::Instant;

use ferb_daemon::areas_b::task_preview::{self as tp, BoxFuture, PreviewClock, TaskPreviews};
use ferb_gateway::locks::ResourceLocks;

/// One monotonic origin for the process: the previews' clock and the Mac driver's (`time.monotonic()` in Python is
/// one clock), so a capture's deadline (`PREVIEW`) means the same instant to both.
pub fn epoch() -> Instant {
    static EPOCH: OnceLock<Instant> = OnceLock::new();
    *EPOCH.get_or_init(Instant::now)
}

/// Seconds since [`epoch`].
pub fn monotonic() -> f64 {
    epoch().elapsed().as_secs_f64()
}

struct Clock;

impl PreviewClock for Clock {
    fn monotonic(&self) -> f64 {
        monotonic()
    }
    fn sleep(&self, seconds: f64) -> BoxFuture<()> {
        Box::pin(tokio::time::sleep(std::time::Duration::from_secs_f64(
            seconds.max(0.0),
        )))
    }
}

/// What the previews are built over: the gateway's resource locks (once it is built: `Gateway::locks`), the image
/// masking, the Screen Recording check and the journal.
pub struct Over {
    pub locks: Arc<dyn Fn() -> Option<ResourceLocks> + Send + Sync>,
    pub images: Arc<dyn ferb_gateway::call_hooks::ImageSanitizer>,
    pub screen_recording: Arc<dyn Fn() -> bool + Send + Sync>,
    pub note: tp::Note,
}

/// `TaskPreviews(self._frame_sanitized, lambda keys, holder: self.gw.locks.try_hold(keys, holder,
/// during_reads=True), _screen_recording_allowed, note=self._note_preview)`.
pub fn build(over: Over) -> TaskPreviews {
    let images = over.images;
    let sanitize: tp::Sanitize = Arc::new(move |image: Vec<u8>| {
        let images = images.clone();
        Box::pin(async move { images.sanitize(&image).await })
    });
    let locks = over.locks;
    let try_hold: tp::TryHold = Arc::new(move |keys: &[String], holder: &str| {
        // before the gateway exists no call runs; the surface counts as busy until it does
        let Some(locks) = locks() else {
            return Ok(None);
        };
        match locks.try_hold(keys, holder, true) {
            Ok(Some(held)) => Ok(Some(Box::new(held) as Box<dyn Send>)),
            Ok(None) => Ok(None),
            Err(_) => Err("ValueError".to_string()),
        }
    });
    TaskPreviews::new(
        sanitize,
        try_hold,
        over.screen_recording,
        tp::PREVIEW_LIMIT_S,
        over.note,
        Arc::new(Clock),
    )
}

/// The previews behind the gateway's hooks (`CallHooks.previews`): a call's surface declared, a task's dropped.
pub struct GatewayPreviews(pub TaskPreviews);

impl ferb_gateway::call_hooks::Previews for GatewayPreviews {
    fn declare(
        &self,
        root: &str,
        task: &str,
        target: Option<ferb_gateway::tool::PreviewTarget>,
        locks: &[String],
    ) {
        let target = target.map(|t| ferb_mac_hands::point::PreviewTarget {
            scope: t.scope,
            label: t.label,
            resolve: t.capture.and_then(|c| {
                c.downcast_ref::<ferb_mac_hands::point::ResolveFn>()
                    .cloned()
            }),
        });
        self.0.declare(root, task, target, locks);
    }

    fn remove(&self, task_ids: &BTreeSet<String>) {
        let roots: Vec<String> = task_ids.iter().cloned().collect();
        self.0.remove(&roots);
    }
}

/// A capability's resolve as the gateway's `PreviewTarget` carries it (type-erased; [`GatewayPreviews`] reads it
/// back).
pub fn capture_of(
    resolve: ferb_mac_hands::point::ResolveFn,
) -> Arc<dyn std::any::Any + Send + Sync> {
    Arc::new(resolve)
}

/// A browser driver's error as a preview capture's (`_error_name` reads its class: a tab gone or no longer the task's
/// is `gone`, a page whose secret fields couldn't be painted is `withheld`).
pub fn hand_error(e: &ferb_browser::error::Error) -> ferb_mac_hands::error::HandError {
    use ferb_mac_hands::error::HandError;
    let m = e.message();
    match e.class() {
        "Refused" => HandError::Refused(m),
        "StaleRef" => HandError::StaleRef(m),
        "NoTarget" => HandError::NoTarget(m),
        "Withheld" => HandError::Withheld(m),
        class => HandError::Named { class, message: m },
    }
}

/// `BrowserTools.preview_target`'s resolve (browser/tools.py:189): the tab, only while it is `owner`'s (a checker
/// reaches its worker's), and its recording source; each frame checks ownership again and has the preview's time
/// left as its limit.
pub fn browser_resolve(
    driver: Arc<ferb_browser::driver::Driver>,
    tab: String,
    owner: String,
) -> ferb_mac_hands::point::ResolveFn {
    use ferb_mac_hands::point::{FrameFn, ResolveFn};
    let resolve: ResolveFn = Arc::new(move || {
        let (driver, tab, owner) = (driver.clone(), tab.clone(), owner.clone());
        Box::pin(async move {
            let held = driver
                .owned_by(&tab, &owner, true)
                .map_err(|e| hand_error(&e))?;
            let (target, recording) = driver
                .recording_source(&held, &owner)
                .map_err(|e| hand_error(&e))?;
            let recording = Arc::new(recording);
            let frames: FrameFn = Arc::new(move || {
                let r = recording.clone();
                Box::pin(async move {
                    let left = ferb_mac_hands::point::preview_left(monotonic());
                    r.capture(left)
                        .await
                        .map(|(jpeg, note)| (jpeg, note.unwrap_or_default()))
                        .map_err(|e| hand_error(&e))
                })
            });
            Ok((target, frames))
        })
    });
    resolve
}

#[cfg(test)]
mod tests {
    use super::*;
    use ferb_gateway::call_hooks::Previews;
    use ferb_mac_hands::point::{FrameFn, ResolveFn};
    use serde_json::{Map, Value};
    use std::sync::Mutex;

    struct Plain;
    impl ferb_gateway::call_hooks::ImageSanitizer for Plain {
        fn sanitize<'a>(
            &'a self,
            image: &'a [u8],
        ) -> ferb_core::interfaces::BoxFuture<'a, Result<Vec<u8>, String>> {
            let mut out = image.to_vec();
            out.extend_from_slice(b"+masked");
            Box::pin(async move { Ok(out) })
        }
    }

    fn resolve(frames: Arc<Mutex<u32>>) -> ResolveFn {
        Arc::new(move || {
            let frames = frames.clone();
            Box::pin(async move {
                let capture: FrameFn = Arc::new(move || {
                    let frames = frames.clone();
                    Box::pin(async move {
                        *frames.lock().unwrap() += 1;
                        Ok((b"window".to_vec(), String::new()))
                    })
                });
                Ok((Map::new(), capture))
            })
        })
    }

    fn previews(locks: &ResourceLocks, notes: Arc<Mutex<Vec<String>>>) -> GatewayPreviews {
        let l = locks.clone();
        GatewayPreviews(build(Over {
            locks: Arc::new(move || Some(l.clone())),
            images: Arc::new(Plain),
            screen_recording: Arc::new(|| true),
            note: Arc::new(move |kind: &str, _d: Value| notes.lock().unwrap().push(kind.into())),
        }))
    }

    fn declare(p: &GatewayPreviews, frames: &Arc<Mutex<u32>>) {
        p.declare(
            "t1",
            "t1.worker",
            Some(ferb_gateway::tool::PreviewTarget {
                scope: "app:com.example.Notes".into(),
                label: "Notes".into(),
                capture: Some(capture_of(resolve(frames.clone()))),
            }),
            &["app:com.example.Notes".to_string()],
        );
    }

    // decision: DMN-403, DMN-406
    #[tokio::test(flavor = "current_thread")]
    async fn a_declared_surface_is_captured_through_the_gateways_locks_and_never_queues() {
        let locks = ResourceLocks::new();
        let notes = Arc::new(Mutex::new(vec![]));
        let p = previews(&locks, notes.clone());
        let frames = Arc::new(Mutex::new(0));
        declare(&p, &frames);
        let keys = ["app:com.example.Notes".to_string()];

        // the capability's resolve came through the gateway's type-erased target: a fresh, masked frame
        let first = p.0.frame("t1", None, None).await;
        assert_eq!(first.image.as_deref(), Some(&b"window+masked"[..]));
        assert_eq!(first.fields["fresh"], json_true());
        assert_eq!(*frames.lock().unwrap(), 1);

        // a worker's acting call holds the app: busy at once, the last frame kept, nothing captured
        let acting = locks.hold(&keys, "t1.worker", false).await.unwrap();
        let busy = p.0.frame("t1", None, None).await;
        assert_eq!(busy.fields.get("busy"), Some(&json_true()));
        assert_eq!(busy.image.as_deref(), Some(&b"window+masked"[..]));
        assert_eq!(*frames.lock().unwrap(), 1);
        drop(acting);

        // a call that only reads: the preview looks alongside it (DMN-406)
        let reading = locks.hold(&keys, "t1.worker", true).await.unwrap();
        let alongside = p.0.frame("t1", None, None).await;
        assert_eq!(alongside.fields["fresh"], json_true());
        assert_eq!(*frames.lock().unwrap(), 2);
        assert_eq!(locks.holder(&keys[0]).as_deref(), Some("t1.worker"));
        drop(reading);

        // the task ends: no frame
        p.remove(&["t1".to_string()].into_iter().collect());
        let after = p.0.frame("t1", None, None).await;
        assert!(after.image.is_none());
        assert_eq!(*frames.lock().unwrap(), 2);
        assert!(notes.lock().unwrap().is_empty());
    }

    // decision: DMN-403
    #[tokio::test(flavor = "current_thread")]
    async fn before_the_gateway_is_built_the_surface_counts_as_busy() {
        let p = GatewayPreviews(build(Over {
            locks: Arc::new(|| None),
            images: Arc::new(Plain),
            screen_recording: Arc::new(|| true),
            note: Arc::new(|_k: &str, _d: Value| {}),
        }));
        let frames = Arc::new(Mutex::new(0));
        declare(&p, &frames);
        let a = p.0.frame("t1", None, None).await;
        assert_eq!(a.fields.get("busy"), Some(&json_true()));
        assert_eq!(*frames.lock().unwrap(), 0);
    }

    #[test]
    fn a_browser_drivers_error_keeps_its_class_for_the_journal() {
        let e = ferb_browser::error::Error::driver("the tab hung");
        let h = hand_error(&e);
        assert_eq!(h.class(), "DriverError");
        assert_eq!(h.to_string(), "the tab hung");
        let r = hand_error(&ferb_browser::error::Error::refused("not this task's tab"));
        assert!(matches!(r, ferb_mac_hands::error::HandError::Refused(_)));
    }

    fn json_true() -> Value {
        Value::Bool(true)
    }
}
