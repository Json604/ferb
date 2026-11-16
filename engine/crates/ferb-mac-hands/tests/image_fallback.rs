//! HAND-531: an image's text is read by the first of several attempts that can (Vision in the process, again on the
//! CPU, then a fresh child process); with none the image is withheld as before. Each failure is kept as its step and
//! error domain and code, never the image or text read from it. Fakes stand in for Vision and the painter; the child
//! process tests run small shell programs, never Vision.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use ferb_mac_hands::mask::ImgBox;
use ferb_mac_hands::ocr::{
    domain, Attempt, Fault, ImageRedactor, Paint, Recognize, TextRegion, UNAVAILABLE,
};
use ferb_mac_hands::vision_child::{self, Helper};
use std::path::Path;

/// A recognizer that fails with `fault`, or reads `regions`; counts its calls.
struct Fake {
    fault: Option<Fault>,
    regions: Vec<TextRegion>,
    calls: AtomicUsize,
}

impl Fake {
    fn fails(domain: &str, code: i64) -> Arc<Fake> {
        Arc::new(Fake {
            fault: Some(Fault::new(domain, code)),
            regions: vec![],
            calls: AtomicUsize::new(0),
        })
    }
    fn reads(regions: Vec<TextRegion>) -> Arc<Fake> {
        Arc::new(Fake {
            fault: None,
            regions,
            calls: AtomicUsize::new(0),
        })
    }
    fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

impl Recognize for Fake {
    fn recognize(&self, _image: &[u8]) -> Result<Vec<TextRegion>, Fault> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        match &self.fault {
            Some(f) => Err(f.clone()),
            None => Ok(self.regions.clone()),
        }
    }
}

/// A painter that marks the image with the boxes it painted, or fails.
struct Painter {
    fail: bool,
    painted: Mutex<Vec<Vec<ImgBox>>>,
}

impl Paint for Painter {
    fn paint(&self, image: &[u8], boxes: &[ImgBox]) -> Result<Vec<u8>, String> {
        if self.fail {
            return Err("couldn't paint".into());
        }
        self.painted.lock().unwrap().push(boxes.to_vec());
        let mut out = image.to_vec();
        out.extend_from_slice(b"+painted");
        Ok(out)
    }
}

fn painter(fail: bool) -> Arc<Painter> {
    Arc::new(Painter {
        fail,
        painted: Mutex::new(vec![]),
    })
}

const SECRET_LINE: &str = "Your verification code is 482913";

fn secret_region() -> Vec<TextRegion> {
    vec![TextRegion {
        text: SECRET_LINE.into(),
        bbox: (4, 6, 120, 18),
    }]
}

fn redactor(attempts: Vec<(&'static str, Arc<Fake>)>, paint: Arc<Painter>) -> ImageRedactor {
    ImageRedactor {
        redactor: Arc::new(ferb_sanitize::Redactor::new(&[], &[])),
        recognize: attempts
            .into_iter()
            .map(|(step, r)| Attempt {
                step,
                recognize: r as Arc<dyn Recognize>,
            })
            .collect(),
        paint,
    }
}

fn vision_fault() -> (&'static str, i64) {
    ("com.apple.Vision", 9)
}

/// The facts FERB records for a fault: only its step, domain and code (and the underlying error's).
fn fact_keys(f: &Fault) -> Vec<String> {
    f.facts().as_object().unwrap().keys().cloned().collect()
}

// decision: HAND-531
#[test]
fn a_failed_first_attempt_is_retried_on_the_cpu_and_the_image_is_still_painted() {
    let (d, c) = vision_fault();
    let first = Fake::fails(d, c);
    let cpu = Fake::reads(secret_region());
    let child = Fake::reads(vec![]);
    let paint = painter(false);
    let r = redactor(
        vec![
            ("vision", first.clone()),
            ("vision_cpu", cpu.clone()),
            ("child", child.clone()),
        ],
        paint.clone(),
    );
    let got = r.redact_noted(b"jpeg", &[]).expect("read on the CPU");
    assert_eq!(got.image, b"jpeg+painted");
    assert_eq!(got.read_by, "vision_cpu");
    assert_eq!(
        paint.painted.lock().unwrap().as_slice(),
        &[vec![(4, 6, 120, 18)]]
    );
    assert_eq!(got.faults.len(), 1);
    assert_eq!(got.faults[0].step, "vision");
    assert_eq!((got.faults[0].domain.as_str(), got.faults[0].code), (d, c));
    // the child is never started once an attempt read the image
    assert_eq!((first.calls(), cpu.calls(), child.calls()), (1, 1, 0));
}

// decision: HAND-531
#[test]
fn when_both_in_process_attempts_fail_the_child_reads_the_image() {
    let mut under = Fault::new("com.apple.Vision", 9);
    under.under = Some(("com.apple.e5rt".into(), 13));
    let first = Arc::new(Fake {
        fault: Some(under),
        regions: vec![],
        calls: AtomicUsize::new(0),
    });
    let cpu = Fake::fails("com.apple.Vision", 3);
    let child = Fake::reads(secret_region());
    let r = redactor(
        vec![
            ("vision", first),
            ("vision_cpu", cpu),
            ("child", child.clone()),
        ],
        painter(false),
    );
    let got = r.redact_noted(b"jpeg", &[]).unwrap();
    assert_eq!(got.read_by, "child");
    assert_eq!(got.image, b"jpeg+painted");
    let steps: Vec<&str> = got.faults.iter().map(|f| f.step.as_str()).collect();
    assert_eq!(steps, ["vision", "vision_cpu"]);
    assert_eq!(
        got.faults[0].facts(),
        serde_json::json!({"step": "vision", "domain": "com.apple.Vision", "code": 9,
                           "under_domain": "com.apple.e5rt", "under_code": 13})
    );
    assert_eq!(child.calls(), 1);
}

// decision: HAND-531
#[test]
fn when_every_attempt_fails_the_image_is_withheld_with_every_fault_and_the_same_text() {
    let r = redactor(
        vec![
            ("vision", Fake::fails("com.apple.Vision", 9)),
            ("vision_cpu", Fake::fails(domain::NO_CPU, 0)),
            ("child", Fake::fails(domain::TIMEOUT, 150)),
        ],
        painter(false),
    );
    let w = r.redact_noted(b"jpeg", &[]).unwrap_err();
    let got: Vec<(String, String, i64)> = w
        .faults
        .iter()
        .map(|f| (f.step.clone(), f.domain.clone(), f.code))
        .collect();
    assert_eq!(
        got,
        [
            ("vision".into(), "com.apple.Vision".into(), 9),
            ("vision_cpu".into(), domain::NO_CPU.into(), 0),
            ("child".into(), domain::TIMEOUT.into(), 150),
        ]
    );
    // the models' and clients' text is Python's, unchanged
    assert_eq!(w.to_string(), UNAVAILABLE);
    assert_eq!(r.redact(b"jpeg", &[]), Err(UNAVAILABLE.to_string()));
}

// decision: HAND-531
#[test]
fn an_image_read_but_not_painted_is_withheld_and_its_text_is_never_recorded() {
    let r = redactor(
        vec![
            ("vision", Fake::fails("com.apple.Vision", 9)),
            ("vision_cpu", Fake::reads(secret_region())),
        ],
        painter(true),
    );
    let w = r.redact_noted(b"jpeg", &[]).unwrap_err();
    let steps: Vec<&str> = w.faults.iter().map(|f| f.step.as_str()).collect();
    assert_eq!(steps, ["vision", "paint"]);
    assert_eq!(w.faults[1].domain, domain::PAINT);
    for f in &w.faults {
        assert_eq!(fact_keys(f), ["step", "domain", "code"]);
        let shown = format!("{} {:?}", f.facts(), f);
        assert!(
            !shown.contains("482913") && !shown.contains("verification"),
            "{shown}"
        );
    }
    assert!(!format!("{w:?}").contains("482913"));
}

// decision: HAND-531
#[test]
fn the_first_attempt_that_reads_is_the_only_one_run() {
    let first = Fake::reads(vec![]);
    let cpu = Fake::reads(vec![]);
    let r = redactor(
        vec![("vision", first.clone()), ("vision_cpu", cpu.clone())],
        painter(false),
    );
    let got = r.redact_noted(b"plain", &[]).unwrap();
    // nothing to paint: the image as it was
    assert_eq!(got.image, b"plain");
    assert_eq!(got.read_by, "vision");
    assert!(got.faults.is_empty());
    assert_eq!((first.calls(), cpu.calls()), (1, 0));
}

#[test]
fn the_childs_answer_carries_regions_or_its_fault_and_nothing_else_is_accepted() {
    let regions = secret_region();
    assert_eq!(
        vision_child::parse_reply(&vision_child::reply(&Ok(regions.clone()))),
        Ok(regions)
    );
    let mut f = Fault::new("com.apple.Vision", 9);
    f.under = Some(("com.apple.e5rt".into(), 13));
    let back = vision_child::parse_reply(&vision_child::reply(&Err(f.clone()))).unwrap_err();
    assert_eq!(
        (back.domain, back.code, back.under),
        (f.domain, f.code, f.under)
    );
    for bad in [
        &b""[..],
        b"not json",
        b"{}",
        b"{\"regions\": [{\"text\": \"a\", \"box\": [1, 2, 3]}]}",
    ] {
        assert_eq!(
            vision_child::parse_reply(bad).unwrap_err().domain,
            domain::REPLY,
            "{}",
            String::from_utf8_lossy(bad)
        );
    }
}

#[test]
fn the_child_side_reads_the_image_from_its_input_with_its_own_retry() {
    let (first, cpu) = (
        Fake::fails("com.apple.Vision", 9),
        Fake::reads(secret_region()),
    );
    let mut out = Vec::new();
    let status = vision_child::serve(&mut &b"jpeg"[..], &mut out, &[&*first, &*cpu]);
    assert_eq!(status, 0);
    assert_eq!(vision_child::parse_reply(&out), Ok(secret_region()));
    let failing = Fake::fails("com.apple.Vision", 3);
    let mut out = Vec::new();
    assert_eq!(
        vision_child::serve(&mut &b"jpeg"[..], &mut out, &[&*failing]),
        0
    );
    assert_eq!(vision_child::parse_reply(&out).unwrap_err().code, 3);
    // an empty input is refused without reading anything
    let mut out = Vec::new();
    assert_eq!(
        vision_child::serve(&mut &b""[..], &mut out, &[&*failing]),
        65
    );
    assert!(out.is_empty());
}

/// A stand-in for `ferb read-image-text --serve` in plain sh: each started copy adds a line to `starts`; each image
/// (read byte by byte with dd, so nothing past it is consumed) adds a line to `reads` and is answered with `$REPLY`,
/// framed by its length. `MODE`: `ok` answers every image; `one` answers one image, then ends at the next with status
/// 3 without answering; `dead` ends at once.
fn helper_script(dir: &Path) -> String {
    format!(
        r#"echo start >> '{d}/starts'
[ "$MODE" = dead ] && exit 3
n_seen=0
while :; do
  hdr=$(dd bs=1 count=4 2>/dev/null | od -An -tu1)
  [ -z "$hdr" ] && exit 0
  set -- $hdr
  n=$(( ($1 << 24) | ($2 << 16) | ($3 << 8) | $4 ))
  dd bs=1 count=$n of=/dev/null 2>/dev/null
  echo read >> '{d}/reads'
  n_seen=$((n_seen + 1))
  [ "$MODE" = one ] && [ $n_seen -gt 1 ] && exit 3
  len=$(printf %s "$REPLY" | wc -c | tr -d ' ')
  printf "\\000\\000\\000\\$(printf %o $len)"
  printf %s "$REPLY"
done
"#,
        d = dir.display()
    )
}

fn lines(dir: &Path, name: &str) -> usize {
    std::fs::read_to_string(dir.join(name))
        .map(|t| t.lines().count())
        .unwrap_or(0)
}

/// A helper over the stand-in with a hand-moved clock.
fn helper(dir: &Path, mode: &str, reply: &str, clock: Arc<Mutex<Duration>>) -> Helper {
    let c = clock.clone();
    let mut h = Helper::with_clock(
        "/usr/bin/env".into(),
        vec![
            format!("MODE={mode}"),
            format!("REPLY={reply}"),
            "/bin/sh".into(),
            "-c".into(),
            helper_script(dir),
        ],
        Duration::from_secs(20),
        Arc::new(move || *c.lock().unwrap()),
    );
    h.timeout = Duration::from_secs(20);
    h
}

fn empty_reply() -> String {
    String::from_utf8(vision_child::reply(&Ok(vec![]))).unwrap()
}

/// HAND-626: the helper is started at the first image and kept: three images, one start; Vision's model loads once.
// decision: HAND-627
#[test]
fn the_kept_helper_reads_one_image_after_another_from_one_start() {
    let dir = tempfile::tempdir().unwrap();
    let clock = Arc::new(Mutex::new(Duration::ZERO));
    let h = helper(dir.path(), "ok", &empty_reply(), clock);
    assert!(!h.running());
    for _ in 0..3 {
        assert_eq!(h.read(b"jpeg"), Ok(vec![]));
    }
    assert!(h.running());
    assert_eq!(
        (lines(dir.path(), "starts"), lines(dir.path(), "reads")),
        (1, 3)
    );
    // stopped (the daemon's own Vision reads again): the next image starts it again
    h.stop();
    assert!(!h.running());
    assert_eq!(h.read(b"jpeg"), Ok(vec![]));
    assert_eq!(lines(dir.path(), "starts"), 2);
}

/// A helper that ends without answering is a fault for that image, and the next image starts a fresh one; one that
/// answers with Vision's fault is ended too (its Vision may be the broken one).
// decision: HAND-627
#[test]
fn a_helper_that_ends_or_faults_is_started_again_for_the_next_image() {
    let dir = tempfile::tempdir().unwrap();
    let clock = Arc::new(Mutex::new(Duration::ZERO));
    let h = helper(dir.path(), "one", &empty_reply(), clock);
    assert_eq!(h.read(b"a"), Ok(vec![]));
    assert_eq!(h.read(b"b").unwrap_err(), Fault::new(domain::EXIT, 3));
    assert!(!h.running());
    assert_eq!(h.read(b"c"), Ok(vec![]));
    assert_eq!(lines(dir.path(), "starts"), 2);

    let dir = tempfile::tempdir().unwrap();
    let mut vision = Fault::new("com.apple.Vision", 9);
    vision.step = "vision".into();
    let faulty = String::from_utf8(vision_child::reply(&Err(vision))).unwrap();
    let h = helper(
        dir.path(),
        "ok",
        &faulty,
        Arc::new(Mutex::new(Duration::ZERO)),
    );
    assert_eq!(h.read(b"a").unwrap_err().domain, "com.apple.Vision");
    assert!(!h.running(), "a helper whose Vision failed is ended");
    let _ = h.read(b"b");
    assert_eq!(lines(dir.path(), "starts"), 2);
}

/// HAND-627: after its first failure in a row the helper starts again at once; after the next ones it waits
/// RESTART_BASE, doubled each time up to RESTART_CEILING (no fixed window that blocks pictures); within the wait an image
/// is withheld at once with the seconds left (`ferb.backoff`); a read resets it.
// decision: HAND-627
#[test]
fn the_helper_starts_again_after_a_growing_wait() {
    use vision_child::{backoff, RESTART_BASE, RESTART_CEILING};
    assert_eq!(backoff(1), Duration::ZERO);
    assert_eq!(backoff(2), RESTART_BASE);
    assert_eq!(backoff(3), RESTART_BASE * 2);
    assert_eq!(backoff(4), RESTART_BASE * 4);
    assert_eq!(backoff(30), RESTART_CEILING);
    let dir = tempfile::tempdir().unwrap();
    let clock = Arc::new(Mutex::new(Duration::ZERO));
    let h = helper(dir.path(), "dead", &empty_reply(), clock.clone());
    assert_eq!(h.read(b"jpeg").unwrap_err().domain, domain::EXIT);
    assert_eq!(h.read(b"jpeg").unwrap_err().domain, domain::EXIT); // the first failure: again at once
    let waiting = h.read(b"jpeg").unwrap_err();
    assert_eq!(
        (waiting.domain.as_str(), waiting.code),
        (domain::BACKOFF, RESTART_BASE.as_secs() as i64)
    );
    assert_eq!(lines(dir.path(), "starts"), 2);
    *clock.lock().unwrap() = RESTART_BASE;
    assert!(h.due());
    assert_eq!(h.read(b"jpeg").unwrap_err().domain, domain::EXIT);
    assert_eq!(h.backoff_state(), (3, RESTART_BASE + RESTART_BASE * 2));
    assert_eq!(lines(dir.path(), "starts"), 3);
}

/// HAND-627: an image withheld while the reader waits to start again says so, with the seconds left; any other
/// withheld image keeps Python's words.
// decision: HAND-627
#[test]
fn a_withheld_image_says_when_the_reader_starts_again() {
    let waiting = ferb_mac_hands::ocr::Withheld {
        faults: vec![Fault::new(domain::BACKOFF, 5)],
    };
    let text = waiting.to_string();
    assert!(text.starts_with(UNAVAILABLE), "{text}");
    assert!(text.contains("starts again in about 5 s"), "{text}");
    let other = ferb_mac_hands::ocr::Withheld {
        faults: vec![Fault::new(domain::EXIT, 1)],
    };
    assert_eq!(other.to_string(), UNAVAILABLE);
}

// decision: HAND-531
#[test]
fn a_helper_that_cant_start_is_a_fault() {
    let h = Helper::new("/nonexistent/ferb".into(), vec![], Duration::from_secs(10));
    let f = h.read(b"jpeg").unwrap_err();
    assert_eq!((f.domain.as_str(), f.code), (domain::START, 2));
}

/// A helper that never answers is killed and its image withheld. Waits on real time (TEST-508 keeps that out of the
/// gate): run by hand.
// decision: HAND-627
#[test]
#[ignore = "waits 300 ms of real time for the helper's limit: run by hand"]
fn a_helper_that_hangs_is_killed_at_its_limit() {
    let mut h = Helper::new(
        "/bin/sh".into(),
        vec!["-c".into(), "sleep 30".into()],
        Duration::from_millis(300),
    );
    h.timeout = Duration::from_millis(300);
    let started = std::time::Instant::now();
    assert_eq!(h.read(b"jpeg").unwrap_err().domain, domain::TIMEOUT);
    assert!(!h.running());
    assert!(started.elapsed() < Duration::from_secs(10));
}

/// The kept child's side: framed images in, framed answers out, until its input ends.
// decision: HAND-627
#[test]
fn the_serving_loop_answers_each_framed_image_until_its_input_ends() {
    let reads = Fake::reads(secret_region());
    let mut input = vec![];
    for image in [&b"one"[..], &b"second"[..]] {
        input.extend_from_slice(&(image.len() as u32).to_be_bytes());
        input.extend_from_slice(image);
    }
    let mut out = Vec::new();
    assert_eq!(
        vision_child::serve_loop(&mut &input[..], &mut out, &[&*reads]),
        0
    );
    assert_eq!(reads.calls(), 2);
    let mut rest = &out[..];
    for _ in 0..2 {
        let n = u32::from_be_bytes(rest[..4].try_into().unwrap()) as usize;
        assert_eq!(
            vision_child::parse_reply(&rest[4..4 + n]),
            Ok(secret_region())
        );
        rest = &rest[4 + n..];
    }
    assert!(rest.is_empty());
    // a frame longer than the limit is refused
    let mut out = Vec::new();
    let huge = ((vision_child::MAX_IMAGE + 1) as u32).to_be_bytes();
    assert_eq!(
        vision_child::serve_loop(&mut &huge[..], &mut out, &[&*reads]),
        65
    );
}

/// This process's Vision, switched broken or working by the test (macOS rebuilding its cache).
struct Toggle {
    broken: std::sync::atomic::AtomicBool,
    calls: AtomicUsize,
}

impl Toggle {
    fn new(broken: bool) -> Arc<Toggle> {
        Arc::new(Toggle {
            broken: std::sync::atomic::AtomicBool::new(broken),
            calls: AtomicUsize::new(0),
        })
    }
    fn set(&self, broken: bool) {
        self.broken.store(broken, Ordering::SeqCst);
    }
    fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

impl Recognize for Toggle {
    fn recognize(&self, _image: &[u8]) -> Result<Vec<TextRegion>, Fault> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if self.broken.load(Ordering::SeqCst) {
            let (d, c) = vision_fault();
            Err(Fault::new(d, c))
        } else {
            Ok(vec![])
        }
    }
}

fn readers_over(
    vision: Arc<Toggle>,
    helper: Arc<Helper>,
    clock: Arc<Mutex<Duration>>,
) -> vision_child::Readers {
    vision_child::Readers::new(
        vec![Attempt {
            step: "vision",
            recognize: vision,
        }],
        Some(helper),
        b"probe".to_vec(),
        Arc::new(move || *clock.lock().unwrap()),
    )
}

fn read_with(
    readers: &vision_child::Readers,
) -> Result<ferb_mac_hands::ocr::Redacted, ferb_mac_hands::ocr::Withheld> {
    let r = ImageRedactor {
        redactor: Arc::new(ferb_sanitize::Redactor::new(&[], &[])),
        recognize: readers.attempts(true),
        paint: painter(false),
    };
    let got = r.redact_noted(b"jpeg", &[]);
    readers.settle(&got);
    got
}

/// HAND-627: an image this process's Vision can't read goes to the helper, and the next ones go straight there; a
/// warm-up never uses the helper; while in-process reads, the helper is stopped.
// decision: HAND-627
#[test]
fn after_the_processs_own_vision_failed_images_go_to_the_helper() {
    let dir = tempfile::tempdir().unwrap();
    let clock = Arc::new(Mutex::new(Duration::ZERO));
    let helper = Arc::new(helper(dir.path(), "ok", &empty_reply(), clock.clone()));
    let vision = Toggle::new(true);
    let readers = readers_over(vision.clone(), helper.clone(), clock.clone());
    assert_eq!(read_with(&readers).unwrap().read_by, "child");
    assert_eq!(read_with(&readers).unwrap().read_by, "child");
    assert_eq!(
        vision.calls(),
        1,
        "the next image skips the broken in-process attempt"
    );
    assert!(readers.attempts(false).iter().all(|a| a.step != "child"));
    let healthy = readers_over(Toggle::new(false), helper.clone(), clock);
    assert!(helper.running());
    assert_eq!(read_with(&healthy).unwrap().read_by, "vision");
    assert!(!helper.running());
}

/// HAND-627: the self-test finds this process's Vision failing before any picture, and the helper is started and
/// warmed then: the first picture needs no start of its own.
// decision: HAND-627
#[test]
fn a_failing_self_test_warms_the_helper_before_a_picture_needs_it() {
    let dir = tempfile::tempdir().unwrap();
    let clock = Arc::new(Mutex::new(Duration::ZERO));
    let helper = Arc::new(helper(dir.path(), "ok", &empty_reply(), clock.clone()));
    let vision = Toggle::new(true);
    let readers = readers_over(vision.clone(), helper.clone(), clock);
    let t = readers.tick();
    assert_eq!(
        (t.tested, t.warmed, t.went_to_helper),
        (Some(false), Some(true), true)
    );
    assert!(helper.running());
    assert_eq!(
        (lines(dir.path(), "starts"), lines(dir.path(), "reads")),
        (1, 1)
    );
    assert_eq!(read_with(&readers).unwrap().read_by, "child");
    assert_eq!(vision.calls(), 1, "only the self-test tried in-process");
    assert_eq!(lines(dir.path(), "starts"), 1);
}

/// HAND-627: while on the helper, the self-test notices this process's Vision reading again and goes back to it, the
/// helper stopped; never a restart of the daemon.
// decision: HAND-627
#[test]
fn the_self_test_goes_back_to_the_processs_own_vision_when_it_reads_again() {
    let dir = tempfile::tempdir().unwrap();
    let clock = Arc::new(Mutex::new(Duration::ZERO));
    let helper = Arc::new(helper(dir.path(), "ok", &empty_reply(), clock.clone()));
    let vision = Toggle::new(true);
    let readers = readers_over(vision.clone(), helper.clone(), clock.clone());
    readers.tick();
    vision.set(false);
    assert_eq!(readers.tick().tested, None, "not due yet");
    *clock.lock().unwrap() = vision_child::SELF_TEST_EVERY;
    let t = readers.tick();
    assert_eq!(
        (t.tested, t.switched_back, t.warmed),
        (Some(true), true, None)
    );
    assert!(!helper.running() && !readers.in_process_failed());
    assert_eq!(read_with(&readers).unwrap().read_by, "vision");
}

/// HAND-627: a picture that comes while the helper is starting waits for it (one start), never withheld for it.
// decision: HAND-627
#[test]
fn a_picture_during_the_helpers_start_waits_and_is_read() {
    let dir = tempfile::tempdir().unwrap();
    let clock = Arc::new(Mutex::new(Duration::ZERO));
    let helper = Arc::new(helper(dir.path(), "ok", &empty_reply(), clock.clone()));
    let readers = Arc::new(readers_over(Toggle::new(true), helper, clock));
    readers.settle(&Err(ferb_mac_hands::ocr::Withheld {
        faults: vec![{
            let mut f = Fault::new("com.apple.Vision", 9);
            f.step = "vision".into();
            f
        }],
    }));
    let r2 = readers.clone();
    let warming = std::thread::spawn(move || r2.tick());
    let got = read_with(&readers);
    let t = warming.join().unwrap();
    assert_eq!(got.unwrap().read_by, "child");
    assert_ne!(t.warmed, Some(false));
    assert_eq!(lines(dir.path(), "starts"), 1);
}

/// HAND-627: a helper that keeps failing: the picture is withheld with the reason, and the ticks keep trying it in the
/// background on the backoff's schedule.
// decision: HAND-627
#[test]
fn a_helper_that_keeps_failing_withholds_with_the_reason_and_is_retried() {
    let dir = tempfile::tempdir().unwrap();
    let clock = Arc::new(Mutex::new(Duration::ZERO));
    let helper = Arc::new(helper(dir.path(), "dead", &empty_reply(), clock.clone()));
    let readers = readers_over(Toggle::new(true), helper.clone(), clock.clone());
    assert_eq!(readers.tick().warmed, Some(false));
    assert_eq!(
        readers.tick().warmed,
        Some(false),
        "the first failure: again at once"
    );
    let withheld = read_with(&readers).unwrap_err();
    assert!(
        withheld.to_string().contains("starts again in about 5 s"),
        "{withheld}"
    );
    assert_eq!(readers.tick().warmed, None, "within the wait");
    *clock.lock().unwrap() = vision_child::RESTART_BASE;
    assert_eq!(readers.tick().warmed, Some(false));
    assert_eq!(lines(dir.path(), "starts"), 3);
}
