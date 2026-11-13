//! Vision's accurate on-device text recognizer (sanitize/images.py `recognize_text`): no language correction, so a
//! credential is never "corrected" into a word. Each region's box in image pixels, rounded outward.

use objc2::rc::Retained;
use objc2::AnyThread;
use objc2_core_graphics::CGImage;
use objc2_foundation::{NSArray, NSData, NSDictionary};
use objc2_vision::{VNImageRequestHandler, VNRecognizeTextRequest, VNRequest};

use crate::ocr::{vision_box, Recognize, TextRegion};

pub struct NativeVision;

impl Recognize for NativeVision {
    fn recognize(&self, image: &[u8]) -> Result<Vec<TextRegion>, String> {
        objc2::rc::autoreleasepool(|_| {
            let request = VNRecognizeTextRequest::new();
            request.setUsesLanguageCorrection(false);
            let data = NSData::with_bytes(image);
            let options = NSDictionary::new();
            let handler = VNImageRequestHandler::initWithData_options(
                VNImageRequestHandler::alloc(),
                &data,
                &options,
            );
            let as_request: Retained<VNRequest> =
                Retained::into_super(Retained::into_super(request.clone()));
            let requests = NSArray::from_retained_slice(&[as_request]);
            handler
                .performRequests_error(&requests)
                .map_err(|_| "on-device text recognition failed".to_string())?;
            let decoded = super::paint::decode(image).map_err(|e| e.to_string())?;
            let (width, height) = (
                CGImage::width(Some(&decoded)) as i64,
                CGImage::height(Some(&decoded)) as i64,
            );
            let mut regions = vec![];
            for obs in request.results().map(|r| r.to_vec()).unwrap_or_default() {
                let candidates = obs.topCandidates(1);
                let Some(best) = candidates.firstObject() else {
                    continue;
                };
                // SAFETY: a recognized-text observation's normalized bounding box.
                let b = unsafe { obs.boundingBox() };
                regions.push(TextRegion {
                    text: best.string().to_string(),
                    bbox: vision_box(
                        b.origin.x,
                        b.origin.y,
                        b.size.width,
                        b.size.height,
                        width,
                        height,
                    ),
                });
            }
            Ok(regions)
        })
    }
}

/// images.py:89: a plain white grayscale PNG, enough for Vision to load its recognizer (`warm_recognition`).
pub fn blank_png(width: u32, height: u32) -> Vec<u8> {
    fn crc32(data: &[u8]) -> u32 {
        let mut crc = 0xffff_ffffu32;
        for b in data {
            crc ^= *b as u32;
            for _ in 0..8 {
                crc = if crc & 1 != 0 {
                    (crc >> 1) ^ 0xedb8_8320
                } else {
                    crc >> 1
                };
            }
        }
        !crc
    }
    fn adler32(data: &[u8]) -> u32 {
        let (mut a, mut b) = (1u32, 0u32);
        for x in data {
            a = (a + *x as u32) % 65521;
            b = (b + a) % 65521;
        }
        (b << 16) | a
    }
    fn chunk(kind: &[u8], data: &[u8]) -> Vec<u8> {
        let mut out = (data.len() as u32).to_be_bytes().to_vec();
        let mut body = kind.to_vec();
        body.extend_from_slice(data);
        out.extend_from_slice(&body);
        out.extend_from_slice(&crc32(&body).to_be_bytes());
        out
    }
    let mut rows = vec![];
    for _ in 0..height {
        rows.push(0u8);
        rows.extend(std::iter::repeat_n(0xffu8, width as usize));
    }
    // zlib with stored (uncompressed) blocks: valid PNG data without a compressor
    let mut z = vec![0x78, 0x01];
    for (i, block) in rows.chunks(65535).enumerate() {
        let last = (i + 1) * 65535 >= rows.len();
        z.push(last as u8);
        z.extend_from_slice(&(block.len() as u16).to_le_bytes());
        z.extend_from_slice(&(!(block.len() as u16)).to_le_bytes());
        z.extend_from_slice(block);
    }
    z.extend_from_slice(&adler32(&rows).to_be_bytes());
    let mut ihdr = width.to_be_bytes().to_vec();
    ihdr.extend_from_slice(&height.to_be_bytes());
    ihdr.extend_from_slice(&[8, 0, 0, 0, 0]);
    let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
    png.extend(chunk(b"IHDR", &ihdr));
    png.extend(chunk(b"IDAT", &z));
    png.extend(chunk(b"IEND", &[]));
    png
}

/// DMN-519: load the recognizer once at start, off the event loop; failures are left to the real redaction.
pub fn warm_recognition() {
    let _ = NativeVision.recognize(&blank_png(160, 48));
}
