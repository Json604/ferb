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
    s.split(ferb_sanitize::parity::is_space)
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
            && (a.ends_with(':')
                || a.ends_with('=')
                || b.chars().any(ferb_sanitize::parity::is_digit))
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
            .filter(|c| !ferb_sanitize::parity::is_space(*c))
            .collect();
        let dots = password_dots().is_match(&squeezed).unwrap_or(false);
        if !spans.is_empty() || card || frame || labelled.contains(&i) || dots {
            boxes.push(region.bbox);
        }
    }
    Ok(boxes)
}

/// Vision's text recognizer (`native::vision`).
pub trait Recognize: Send + Sync {
    fn recognize(&self, image: &[u8]) -> Result<Vec<TextRegion>, String>;
}

/// Opaque boxes painted on a JPEG and re-encoded (`native::paint`; mask.py:116).
pub trait Paint: Send + Sync {
    fn paint(&self, image: &[u8], boxes: &[ImgBox]) -> Result<Vec<u8>, String>;
}

/// images.py:113: every image FERB stores or sends passes here.
#[derive(Clone)]
pub struct ImageRedactor {
    pub redactor: Arc<Redactor>,
    pub recognize: Arc<dyn Recognize>,
    pub paint: Arc<dyn Paint>,
}

impl ImageRedactor {
    pub fn redact(&self, image: &[u8], rects: &[ImgBox]) -> Result<Vec<u8>, String> {
        let regions = self
            .recognize
            .recognize(image)
            .map_err(|_| UNAVAILABLE.to_string())?;
        let boxes =
            boxes_to_paint(&self.redactor, &regions, rects).map_err(|_| UNAVAILABLE.to_string())?;
        if boxes.is_empty() {
            return Ok(image.to_vec());
        }
        self.paint
            .paint(image, &boxes)
            .map_err(|_| UNAVAILABLE.to_string())
    }
}
