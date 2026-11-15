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
use ferb_mac_hands::vision_child::{self, ChildVision};

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

fn shell(script: &str, timeout: Duration) -> ChildVision {
    ChildVision {
        program: "/bin/sh".into(),
        args: vec!["-c".into(), script.into()],
        timeout,
        gate: Arc::new(Mutex::new(())),
        wait: true,
    }
}

// decision: HAND-531
#[test]
fn a_child_process_answers_over_its_pipes() {
    // the child reads the whole image from its input before answering
    let reply = String::from_utf8(vision_child::reply(&Ok(secret_region()))).unwrap();
    let c = shell(
        &format!("n=$(wc -c | tr -d ' '); [ \"$n\" = 4 ] && printf '%s' '{reply}'"),
        Duration::from_secs(20),
    );
    assert_eq!(c.recognize(b"jpeg"), Ok(secret_region()));
}

// decision: HAND-531
#[test]
fn a_child_that_fails_or_cant_start_is_a_fault() {
    let ten = Duration::from_secs(10);
    assert_eq!(
        shell("cat >/dev/null; exit 3", ten)
            .recognize(b"jpeg")
            .unwrap_err(),
        Fault::new(domain::EXIT, 3)
    );
    assert_eq!(
        shell("cat >/dev/null; printf 'garbage'", ten)
            .recognize(b"jpeg")
            .unwrap_err()
            .domain,
        domain::REPLY
    );
    let missing = ChildVision {
        program: "/nonexistent/ferb".into(),
        ..shell("", ten)
    };
    let f = missing.recognize(b"jpeg").unwrap_err();
    assert_eq!((f.domain.as_str(), f.code), (domain::START, 2));
}

/// A child that never answers is killed and its image withheld. Waits on real time (TEST-508 keeps that out of the gate): run by hand.
// decision: HAND-531
#[test]
#[ignore = "waits 300 ms of real time for a child's limit: run by hand"]
fn a_child_that_hangs_is_killed_at_its_limit() {
    let started = std::time::Instant::now();
    let hung = shell("sleep 30", Duration::from_millis(300)).recognize(b"jpeg");
    assert_eq!(hung.unwrap_err(), Fault::new(domain::TIMEOUT, 0));
    assert!(
        started.elapsed() < Duration::from_secs(10),
        "killed at its limit"
    );
}

// decision: HAND-531
#[test]
fn a_preview_frame_doesnt_queue_behind_a_running_child_and_other_images_wait() {
    let gate = Arc::new(Mutex::new(()));
    let reply = String::from_utf8(vision_child::reply(&Ok(vec![]))).unwrap();
    let mut c = shell(
        &format!("cat >/dev/null; printf '%s' '{reply}'"),
        Duration::from_secs(20),
    );
    c.gate = gate.clone();
    let held = gate.lock().unwrap();
    let preview = ChildVision {
        wait: false,
        ..c.clone()
    };
    assert_eq!(preview.recognize(b"jpeg").unwrap_err().domain, domain::BUSY);
    // an image that waits runs once the running child is done
    let waiting = std::thread::spawn(move || c.recognize(b"jpeg"));
    // it can't have run: the gate is held
    assert!(!waiting.is_finished());
    drop(held);
    assert_eq!(waiting.join().unwrap(), Ok(vec![]));
}
