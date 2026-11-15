//! On-device text recognition and opaque masking before an image is stored or sent (ferb/sanitize/images.py). No
//! hosted OCR, no raw text kept, no raw fallback: anything that goes wrong withholds the pixels. Vision itself is
//! `native::vision`; the decisions here (which regions are painted) are pure and checked against Python.

use std::sync::Arc;
use std::sync::OnceLock;

use fancy_regex::Regex;
use ferb_sanitize::{
    code_words, labels::code_alone, payment_card, secret_words, Redactor, CODE_REACH,
};

use crate::mask::ImgBox;

const H: &str = "\\t\\x0b\\x0c\\x1c-\\x1f \\x85\\xa0\\x{1680}\\x{2000}-\\x{200a}\\x{2028}\\x{2029}\\x{202f}\\x{205f}\\x{3000}";

fn rx(cell: &'static OnceLock<Regex>, src: impl FnOnce() -> String) -> &'static Regex {
    cell.get_or_init(|| Regex::new(&src()).expect("a fixed pattern"))
}

fn card_digits() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    rx(&R, || format!("\\d(?:[{H}-]?\\d){{12,18}}"))
}
fn frame_code() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    rx(&R, || format!("(?<!\\d)\\d(?:[{H}-]?\\d){{3,7}}(?!\\d)"))
}
fn password_dots() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    rx(&R, || "^[•●∙⋅·◦*]{4,}$".to_string())
}
fn otp_label() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    rx(&R, || {
        format!(
            "(?i)\\b(?:otp|totp|2fa|one[{H}_-]?time(?:[{H}_-]+(?:code|password|pin))?|\
(?:verification|authentication|security|login|sign[{H}_-]?in|access)[{H}_-]+(?:code|pin))\\b"
        )
    })
}
fn code_link() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    rx(&R, || {
        let tag = "</?[A-Za-z][A-Za-z0-9]*(?:[ \\t][^<>\\r\\n]*|/)?>";
        format!("(?i)^(?>(?:[{H}*_`'\"]|{tag})*)(?:(?:is|are|was|as)(?![A-Za-z0-9])|\\()")
    })
}
fn tail_marks() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    rx(&R, || format!("^[{H}:=]*$"))
}

fn strip(s: &str) -> &str {
    ferb_py::value::strip(s)
}

fn words(s: &str) -> usize {
    s.split(ferb_py::text::is_space)
        .filter(|w| !w.is_empty())
        .count()
}

/// images.py:42 (C1): the OCR lines that hold a labelled secret only with their neighbour.
pub fn neighbour_spans(lines: &[String]) -> Vec<usize> {
    let mut out: Vec<usize> = vec![];
    let add = |i: usize, out: &mut Vec<usize>| {
        if !out.contains(&i) {
            out.push(i);
        }
    };
    for i in 0..lines.len().saturating_sub(1) {
        let (a, b) = (strip(&lines[i]), strip(&lines[i + 1]));
        let code_b = code_alone(b);
        let near_end = code_words(a)
            .into_iter()
            .any(|m| words(&a[m.1..]) <= CODE_REACH);
        if code_b && near_end {
            add(i + 1, &mut out);
        }
        let secret = secret_words(a)
            .into_iter()
            .any(|m| tail_marks().is_match(&a[m.1..]).unwrap_or(false));
        if secret
            && !b.is_empty()
            && words(b) == 1
            && (a.ends_with(':') || a.ends_with('=') || b.chars().any(ferb_py::text::is_digit))
        {
            add(i + 1, &mut out);
        }
        if code_alone(a)
            && code_link().is_match(b).unwrap_or(false)
            && code_words(b)
                .into_iter()
                .any(|m| words(&b[..m.0]) <= CODE_REACH)
        {
            add(i, &mut out);
        }
    }
    out.sort_unstable();
    out
}

/// One region Vision read: its text and its box in image pixels.
#[derive(Debug, Clone, PartialEq)]
pub struct TextRegion {
    pub text: String,
    pub bbox: ImgBox,
}

/// The image-pixel box of a Vision observation's normalized bounding box (bottom-left origin), rounded outward
/// (images.py:107-111).
pub fn vision_box(x: f64, y: f64, w: f64, h: f64, width: i64, height: i64) -> ImgBox {
    let (wf, hf) = (width as f64, height as f64);
    let left = (x * wf).floor() as i64;
    let top = ((1.0 - y - h) * hf).floor() as i64;
    let right = ((x + w) * wf).ceil() as i64;
    let bottom = ((1.0 - y) * hf).ceil() as i64;
    (left, top, right - left, bottom - top)
}

/// The image redaction failed: the pixels are withheld (images.py:133).
pub const UNAVAILABLE: &str = "image redaction unavailable; pixels withheld";

/// Which boxes images.py:121-131 paints: the given ones, then every region the text rules mark. Err when the text
/// redactor couldn't read a region (Python raises there, and the image is withheld).
pub fn boxes_to_paint(
    redactor: &Redactor,
    regions: &[TextRegion],
    rects: &[ImgBox],
) -> Result<Vec<ImgBox>, ()> {
    let mut boxes: Vec<ImgBox> = rects.to_vec();
    let code_frame = regions
        .iter()
        .any(|r| otp_label().is_match(&r.text).unwrap_or(false));
    let texts: Vec<String> = regions.iter().map(|r| r.text.clone()).collect();
    let labelled = neighbour_spans(&texts);
    for (i, region) in regions.iter().enumerate() {
        let text = &region.text;
        let spans = redactor.spans(text, false).map_err(|_| ())?;
        let card = card_digits()
            .find_iter(text)
            .filter_map(Result::ok)
            .any(|m| payment_card(m.as_str()));
        let frame = code_frame && frame_code().is_match(text).unwrap_or(false);
        let squeezed: String = text
            .chars()
            .filter(|c| !ferb_py::text::is_space(*c))
            .collect();
        let dots = password_dots().is_match(&squeezed).unwrap_or(false);
        if !spans.is_empty() || card || frame || labelled.contains(&i) || dots {
            boxes.push(region.bbox);
        }
    }
    Ok(boxes)
}

/// Where reading or masking an image failed, for FERB's records (HAND-531): the step, and the framework's error
/// domain and code (an NSError's, with its underlying error's when it has one) or FERB's own (`ferb.*`). It never
/// holds the image, text read from it or an error's message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fault {
    /// The attempt that failed (`vision`, `vision_cpu`, `child`), or `rules` / `paint` after one read the image.
    pub step: String,
    pub domain: String,
    pub code: i64,
    /// The underlying error's domain and code (NSUnderlyingErrorKey), when there is one.
    pub under: Option<(String, i64)>,
}

impl Fault {
    /// A fault of `domain` and `code`; its step is set by the chain that tried it.
    pub fn new(domain: &str, code: i64) -> Fault {
        Fault {
            step: String::new(),
            domain: domain.to_string(),
            code,
            under: None,
        }
    }

    /// The fault as FERB's records keep it: step, domain, code (and the underlying error's).
    pub fn facts(&self) -> serde_json::Value {
        let mut out = serde_json::Map::new();
        out.insert("step".into(), self.step.clone().into());
        out.insert("domain".into(), self.domain.clone().into());
        out.insert("code".into(), self.code.into());
        if let Some((d, c)) = &self.under {
            out.insert("under_domain".into(), d.clone().into());
            out.insert("under_code".into(), (*c).into());
        }
        serde_json::Value::Object(out)
    }
}

/// FERB's own fault domains (an Apple framework's error keeps its own domain).
pub mod domain {
    /// The image couldn't be decoded for its size.
    pub const DECODE: &str = "ferb.decode";
    /// No CPU device was offered for a compute stage of the request.
    pub const NO_CPU: &str = "ferb.no_cpu_device";
    /// The child process couldn't be started (code: errno, or -1).
    pub const START: &str = "ferb.start";
    /// The child gave no answer in time (code: the limit in seconds); it was killed.
    pub const TIMEOUT: &str = "ferb.timeout";
    /// The child ended without an answer (code: its exit status, -1 for a signal).
    pub const EXIT: &str = "ferb.exit";
    /// The child's answer couldn't be read.
    pub const REPLY: &str = "ferb.reply";
    /// Another image's child was running and this one doesn't queue (a live preview's frame).
    pub const BUSY: &str = "ferb.busy";
    /// The text rules couldn't read a region.
    pub const RULES: &str = "ferb.rules";
    /// The boxes couldn't be painted.
    pub const PAINT: &str = "ferb.paint";
}

/// Vision's text recognizer (`native::vision`).
pub trait Recognize: Send + Sync {
    fn recognize(&self, image: &[u8]) -> Result<Vec<TextRegion>, Fault>;
}

/// Opaque boxes painted on a JPEG and re-encoded (`native::paint`; mask.py:116).
pub trait Paint: Send + Sync {
    fn paint(&self, image: &[u8], boxes: &[ImgBox]) -> Result<Vec<u8>, String>;
}

/// One way of reading an image's text, named for the record.
#[derive(Clone)]
pub struct Attempt {
    pub step: &'static str,
    pub recognize: Arc<dyn Recognize>,
}

/// An image read and masked: the image, the attempt that read it, and the attempts that failed before it (empty when
/// the first read it).
#[derive(Debug)]
pub struct Redacted {
    pub image: Vec<u8>,
    pub read_by: &'static str,
    pub faults: Vec<Fault>,
}

/// An image withheld: every fault on the way. Shown only as [`UNAVAILABLE`].
#[derive(Debug, Default)]
pub struct Withheld {
    pub faults: Vec<Fault>,
}

impl std::fmt::Display for Withheld {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(UNAVAILABLE)
    }
}

/// images.py:113: every image FERB stores or sends passes here. HAND-531: the recognizers are tried in order until
/// one reads the image (Vision in this process, again on the CPU, then in a fresh child process); with none, the image
/// is withheld as before. A painted box is never skipped: an image is returned only after its text was read.
#[derive(Clone)]
pub struct ImageRedactor {
    pub redactor: Arc<Redactor>,
    pub recognize: Vec<Attempt>,
    pub paint: Arc<dyn Paint>,
}

impl ImageRedactor {
    /// The masked image, or [`UNAVAILABLE`] (Python's RuntimeError text: a contract for the models and clients).
    pub fn redact(&self, image: &[u8], rects: &[ImgBox]) -> Result<Vec<u8>, String> {
        self.redact_noted(image, rects)
            .map(|r| r.image)
            .map_err(|w| w.to_string())
    }

    /// [`redact`](Self::redact) with the faults on the way, for FERB's records.
    pub fn redact_noted(&self, image: &[u8], rects: &[ImgBox]) -> Result<Redacted, Withheld> {
        let mut faults = Vec::new();
        let mut read = None;
        for attempt in &self.recognize {
            match attempt.recognize.recognize(image) {
                Ok(regions) => {
                    read = Some((attempt.step, regions));
                    break;
                }
                Err(mut f) => {
                    f.step = attempt.step.to_string();
                    faults.push(f);
                }
            }
        }
        let Some((read_by, regions)) = read else {
            return Err(Withheld { faults });
        };
        let after = |faults: &mut Vec<Fault>, step: &str, domain: &str| {
            let mut f = Fault::new(domain, 0);
            f.step = step.to_string();
            faults.push(f);
        };
        let Ok(boxes) = boxes_to_paint(&self.redactor, &regions, rects) else {
            after(&mut faults, "rules", domain::RULES);
            return Err(Withheld { faults });
        };
        if boxes.is_empty() {
            return Ok(Redacted {
                image: image.to_vec(),
                read_by,
                faults,
            });
        }
        match self.paint.paint(image, &boxes) {
            Ok(image) => Ok(Redacted {
                image,
                read_by,
                faults,
            }),
            Err(_) => {
                after(&mut faults, "paint", domain::PAINT);
                Err(Withheld { faults })
            }
        }
    }
}
