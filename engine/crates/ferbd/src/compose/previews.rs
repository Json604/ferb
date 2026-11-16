//! A task's live previews (`ferb/daemon/task_preview.py`, built as `call_hooks.py:79` builds them): the gateway's
//! hooks declare each call's surface, FERB.app asks for frames (`tasks.preview`), the resource log counts them. A
//! capture takes the surface's locks only when they are free: the gateway's own `try_hold` with `during_reads`
//! (DMN-403, DMN-406), so a preview never queues behind, or ahead of, a worker's call. APP-922 (owner): the frame is
//! shown as captured, an `OwnerOnlyFrame` for FERB.app's eyes only; it never passes the image masking the tools,
//! evidence and recordings go through (L13), and nothing of it is stored or sent anywhere else.

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

/// What the previews are built over: the gateway's resource locks (once it is built: `Gateway::locks`), the Screen
/// Recording check and the journal.
pub struct Over {
    pub locks: Arc<dyn Fn() -> Option<ResourceLocks> + Send + Sync>,
    pub screen_recording: Arc<dyn Fn() -> bool + Send + Sync>,
    pub note: tp::Note,
}

/// `TaskPreviews(lambda keys, holder: self.gw.locks.try_hold(keys, holder, during_reads=True),
/// _screen_recording_allowed, note=self._note_preview)`; Python's frame sanitizer is gone (APP-922).
pub fn build(over: Over) -> TaskPreviews {
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
/// left as its limit. APP-922: the frame is the page as the browser sent it (`capture_for_owner`), never painted.
pub fn browser_resolve(
    driver: Arc<ferb_browser::driver::Driver>,
    tab: String,
    owner: String,
) -> ferb_mac_hands::point::ResolveFn {
    use ferb_mac_hands::point::{OwnerOnlyFrame, PreviewFrameFn, ResolveFn};
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
            let frames: PreviewFrameFn = Arc::new(move || {
                let r = recording.clone();
                Box::pin(async move {
                    let left = ferb_mac_hands::point::preview_left(monotonic());
                    r.capture_for_owner(left)
                        .await
                        .map(|image| {
                            (
                                OwnerOnlyFrame::captured(image.jpeg().to_vec()),
                                String::new(),
                            )
                        })
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
    use ferb_mac_hands::point::{OwnerOnlyFrame, PreviewFrameFn, ResolveFn};
    use serde_json::{Map, Value};
    use std::sync::Mutex;

    /// An invented window whose pixels show a one-time code (the test's stand-in for a secret on screen).
    const SECRET_WINDOW: &[u8] = b"window: your verification code is 482913";

    fn resolve(frames: Arc<Mutex<u32>>) -> ResolveFn {
        Arc::new(move || {
            let frames = frames.clone();
            Box::pin(async move {
                let capture: PreviewFrameFn = Arc::new(move || {
                    let frames = frames.clone();
                    Box::pin(async move {
                        *frames.lock().unwrap() += 1;
                        Ok((
                            OwnerOnlyFrame::captured(SECRET_WINDOW.to_vec()),
                            String::new(),
                        ))
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

    fn shown(a: &tp::PreviewAnswer) -> Option<Vec<u8>> {
        a.image.as_ref().map(|i| i.for_owner_app().to_vec())
    }

    // decision: DMN-403, DMN-406, APP-922
    #[tokio::test(flavor = "current_thread")]
    async fn a_declared_surface_is_captured_through_the_gateways_locks_and_never_queues() {
        let locks = ResourceLocks::new();
        let notes = Arc::new(Mutex::new(vec![]));
        let p = previews(&locks, notes.clone());
        let frames = Arc::new(Mutex::new(0));
        declare(&p, &frames);
        let keys = ["app:com.example.Notes".to_string()];

        // the capability's resolve came through the gateway's type-erased target: a fresh frame, as captured
        let first = p.0.frame("t1", None, None).await;
        assert_eq!(shown(&first).as_deref(), Some(SECRET_WINDOW));
        assert_eq!(first.fields["fresh"], json_true());
        assert_eq!(*frames.lock().unwrap(), 1);

        // a worker's acting call holds the app: busy at once, the last frame kept, nothing captured
        let acting = locks.hold(&keys, "t1.worker", false).await.unwrap();
        let busy = p.0.frame("t1", None, None).await;
        assert_eq!(busy.fields.get("busy"), Some(&json_true()));
        assert_eq!(shown(&busy).as_deref(), Some(SECRET_WINDOW));
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

    /// APP-922: the owner's preview shows the window as captured, code and all, and the answer's only image is that
    /// frame; the journal gets nothing of it. The frame is FERB.app's alone: an `OwnerOnlyFrame`, whose Debug never
    /// shows its bytes.
    // decision: APP-922
    #[tokio::test(flavor = "current_thread")]
    async fn the_preview_shows_the_frame_as_captured_and_keeps_its_bytes_to_itself() {
        let locks = ResourceLocks::new();
        let notes = Arc::new(Mutex::new(vec![]));
        let p = previews(&locks, notes.clone());
        let frames = Arc::new(Mutex::new(0));
        declare(&p, &frames);
        let got = p.0.frame("t1", None, None).await;
        assert_eq!(shown(&got).as_deref(), Some(SECRET_WINDOW));
        assert!(got.fields.get("problem").is_none());
        let debug = format!("{got:?}");
        assert!(!debug.contains("482913"), "{debug}");
        assert!(notes.lock().unwrap().is_empty());
        let snapshot = Value::Object(p.0.snapshot()).to_string();
        assert!(
            !snapshot.contains("482913") && !snapshot.contains("ocr"),
            "{snapshot}"
        );
    }

    /// The text reader and painter of the tool path, faked: the window's text is the code line; painting marks it.
    struct ReadsCode;
    impl ferb_mac_hands::ocr::Recognize for ReadsCode {
        fn recognize(
            &self,
            _image: &[u8],
        ) -> Result<Vec<ferb_mac_hands::ocr::TextRegion>, ferb_mac_hands::ocr::Fault> {
            Ok(vec![ferb_mac_hands::ocr::TextRegion {
                text: "Your verification code is 482913".into(),
                bbox: (0, 0, 100, 20),
            }])
        }
    }
    struct Fails;
    impl ferb_mac_hands::ocr::Recognize for Fails {
        fn recognize(
            &self,
            _image: &[u8],
        ) -> Result<Vec<ferb_mac_hands::ocr::TextRegion>, ferb_mac_hands::ocr::Fault> {
            Err(ferb_mac_hands::ocr::Fault::new("com.apple.Vision", 9))
        }
    }
    struct Paints;
    impl ferb_mac_hands::ocr::Paint for Paints {
        fn paint(
            &self,
            _image: &[u8],
            boxes: &[ferb_mac_hands::mask::ImgBox],
        ) -> Result<Vec<u8>, String> {
            Ok(format!("painted {} box(es)", boxes.len()).into_bytes())
        }
    }

    /// APP-922 with rule 7: the same window that the owner's preview shows as captured is, on the tool path (what a
    /// model, evidence or a recording gets), painted over, or withheld when its text can't be read.
    // decision: APP-922, HAND-531
    #[tokio::test(flavor = "current_thread")]
    async fn the_same_image_is_shown_to_the_owner_but_masked_for_everything_else() {
        let locks = ResourceLocks::new();
        let p = previews(&locks, Arc::new(Mutex::new(vec![])));
        let frames = Arc::new(Mutex::new(0));
        declare(&p, &frames);
        let preview = p.0.frame("t1", None, None).await;
        assert_eq!(shown(&preview).as_deref(), Some(SECRET_WINDOW));
        let tool =
            |reader: Arc<dyn ferb_mac_hands::ocr::Recognize>| ferb_mac_hands::ocr::ImageRedactor {
                redactor: Arc::new(ferb_sanitize::Redactor::new(&[], &[])),
                recognize: vec![ferb_mac_hands::ocr::Attempt {
                    step: "vision",
                    recognize: reader,
                }],
                paint: Arc::new(Paints),
            };
        let masked = tool(Arc::new(ReadsCode))
            .redact(SECRET_WINDOW, &[])
            .unwrap();
        assert_eq!(masked, b"painted 1 box(es)");
        assert_eq!(
            tool(Arc::new(Fails)).redact(SECRET_WINDOW, &[]),
            Err(ferb_mac_hands::ocr::UNAVAILABLE.to_string())
        );
    }

    /// APP-922: preview frames are FERB.app's alone. `tasks.preview` is the cli role's and app_only (the signed
    /// FERB.app peer), never a model's tool bridge (mcp_shim) or the extension's channel (native_host).
    // decision: APP-922, DMN-23
    #[test]
    fn only_ferb_app_may_ask_for_preview_frames() {
        use ferb_rpc::auth::{method, Role};
        let m = method("tasks.preview").expect("the method");
        assert_eq!(m.roles, &[Role::Cli]);
        assert!(m.app_only);
    }

    /// APP-922: the unmasked frame path stays apart from the masked image path. An `OwnerOnlyFrame` is made only by
    /// the preview's own captures and its bytes are read only by the `tasks.preview` answer: no other source file in
    /// the engine calls either, so no tool result, evidence, recording, trace, log or store can be handed one. PHN-454:
    /// the paired phone's reader (`for_paired_phone`) is read in the `tasks.preview` answer alone, and its grant
    /// (`PairedPhoneLink`) is made by the phone gate alone (`Phones::preview_link`).
    // decision: APP-922
    // decision: PHN-454
    // decision: PHN-512
    #[test]
    fn owner_only_frames_are_made_and_read_in_the_preview_path_only() {
        let crates = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
        let allowed_make = [
            "ferb-mac-hands/src/point.rs",
            "ferb-mac-hands/src/driver/mod.rs",
            "ferbd/src/compose/previews.rs",
        ];
        let allowed_read = [
            "ferb-mac-hands/src/point.rs",
            "ferb-daemon/src/areas_b/task_preview.rs",
            "ferbd/src/areas_c/records.rs",
            "ferbd/src/compose/previews.rs",
            // GATE-579: the payment question's picture, taken once by FERB.app (questions.picture, app_only)
            "ferbd/src/compose/tools/mac.rs",
            "ferbd/src/compose/tools/browser.rs",
        ];
        // PHN-454: the phone's reader, and who may make its grant
        // (this file names both only in this test's own search)
        let allowed_phone_read = [
            "ferb-mac-hands/src/point.rs",
            "ferbd/src/areas_c/records.rs",
            "ferbd/src/compose/previews.rs",
        ];
        let allowed_grant = [
            "ferb-mac-hands/src/point.rs",
            "ferb-daemon/src/phone/mod.rs",
            "ferbd/src/compose/previews.rs",
        ];
        let mut stack = vec![crates.clone()];
        let (mut makes, mut reads) = (vec![], vec![]);
        let (mut phone_reads, mut grants) = (vec![], vec![]);
        while let Some(dir) = stack.pop() {
            for e in std::fs::read_dir(&dir).unwrap().flatten() {
                let path = e.path();
                let name = e.file_name().to_string_lossy().into_owned();
                if path.is_dir() {
                    // sources only; build folders and tests (fakes make frames) aren't the engine
                    if name != "target" && name != "tests" && !name.starts_with('.') {
                        stack.push(path);
                    }
                    continue;
                }
                if !name.ends_with(".rs") {
                    continue;
                }
                let rel = path
                    .strip_prefix(&crates)
                    .unwrap()
                    .to_string_lossy()
                    .into_owned();
                if !rel.contains("/src/") {
                    continue;
                }
                let text = std::fs::read_to_string(&path).unwrap();
                if text.contains("OwnerOnlyFrame::captured(") {
                    makes.push(rel.clone());
                }
                if text.contains(".for_paired_phone(") {
                    phone_reads.push(rel.clone());
                }
                if text.contains("PairedPhoneLink::after_the_phone_gate_s_checks(") {
                    grants.push(rel.clone());
                }
                if text.contains(".for_owner_app()") {
                    reads.push(rel);
                }
            }
        }
        makes.sort();
        reads.sort();
        for m in &makes {
            assert!(
                allowed_make.contains(&m.as_str()),
                "a preview frame made in {m}"
            );
        }
        for r in &reads {
            assert!(
                allowed_read.contains(&r.as_str()),
                "a preview frame's bytes read in {r}"
            );
        }
        assert!(
            makes.iter().any(|m| m.ends_with("driver/mod.rs")),
            "{makes:?}"
        );
        assert!(reads.iter().any(|r| r.ends_with("records.rs")), "{reads:?}");
        for r in &phone_reads {
            assert!(
                allowed_phone_read.contains(&r.as_str()),
                "a preview frame's bytes read for a phone in {r}"
            );
        }
        for g in &grants {
            assert!(
                allowed_grant.contains(&g.as_str()),
                "a phone's preview grant made in {g}"
            );
        }
        assert!(
            phone_reads.iter().any(|r| r.ends_with("records.rs"))
                && grants.iter().any(|g| g.ends_with("phone/mod.rs")),
            "{phone_reads:?} {grants:?}"
        );
    }

    fn json_true() -> Value {
        Value::Bool(true)
    }
}
