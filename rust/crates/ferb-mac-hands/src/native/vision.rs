//! Vision's accurate on-device text recognizer (sanitize/images.py `recognize_text`): no language correction, so a
//! credential is never "corrected" into a word. Each region's box in image pixels, rounded outward.
//!
//! HAND-531: a failure carries Vision's error domain and code (never its message). Seen 2026-11-15 in the daemon
//! after hours of running: every request failed inside Vision's model runtime ("E5RT ... create_precompiled_compute_
//! operation ... (Code: 13)" in the unified log) while a fresh process read the same kind of image. [`CpuVision`]
//! asks for the CPU on every compute stage (a different compiled model than the one Vision chose before), and
//! [`child_main`] is the fresh process.

use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::AnyThread;
use objc2_core_graphics::CGImage;
use objc2_foundation::{NSArray, NSData, NSDictionary, NSError, NSUnderlyingErrorKey};
use objc2_vision::{VNImageRequestHandler, VNRecognizeTextRequest, VNRequest};

use crate::ocr::{domain, vision_box, Fault, Recognize, TextRegion};

/// Vision as it chooses its devices (the Neural Engine or the GPU when it can).
pub struct NativeVision;

/// Vision with every compute stage on the CPU.
pub struct CpuVision;

impl Recognize for NativeVision {
    fn recognize(&self, image: &[u8]) -> Result<Vec<TextRegion>, Fault> {
        read(image, false)
    }
}

impl Recognize for CpuVision {
    fn recognize(&self, image: &[u8]) -> Result<Vec<TextRegion>, Fault> {
        read(image, true)
    }
}

/// An NSError as a fault: its domain and code, and its underlying error's (never a message or user info text).
pub fn fault_of(e: &NSError) -> Fault {
    let mut f = Fault::new(&e.domain().to_string(), e.code() as i64);
    // SAFETY: the exported key is an NSString constant.
    let key = unsafe { NSUnderlyingErrorKey };
    f.under = e
        .userInfo()
        .objectForKey(key)
        .and_then(|u| u.downcast::<NSError>().ok())
        .map(|u| (u.domain().to_string(), u.code() as i64));
    f
}

/// Every compute stage of `request` on the CPU device Vision offers for it.
fn on_cpu(request: &VNRecognizeTextRequest) -> Result<(), Fault> {
    // SAFETY: a configured request; the dictionary maps each compute stage to the devices it supports.
    let stages = unsafe { request.supportedComputeStageDevicesAndReturnError() }
        .map_err(|e| fault_of(&e))?;
    let mut set = false;
    for stage in stages.allKeys().iter() {
        let Some(devices) = stages.objectForKey(&stage) else {
            continue;
        };
        let cpu = devices.iter().find(|d| {
            let any: &AnyObject = d.as_ref();
            any.class().name().to_bytes() == b"MLCPUComputeDevice"
        });
        let Some(cpu) = cpu else {
            return Err(Fault::new(domain::NO_CPU, 0));
        };
        // SAFETY: a device Vision listed as supported for this stage of this request.
        unsafe { request.setComputeDevice_forComputeStage(Some(&cpu), &stage) };
        set = true;
    }
    if set {
        Ok(())
    } else {
        Err(Fault::new(domain::NO_CPU, 0))
    }
}

fn read(image: &[u8], cpu: bool) -> Result<Vec<TextRegion>, Fault> {
    objc2::rc::autoreleasepool(|_| {
        let request = VNRecognizeTextRequest::new();
        request.setUsesLanguageCorrection(false);
        if cpu {
            on_cpu(&request)?;
        }
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
            .map_err(|e| fault_of(&e))?;
        let decoded = super::paint::decode(image).map_err(|_| Fault::new(domain::DECODE, 0))?;
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

/// The child's side of `vision_child` (`ferb read-image-text`): the image on standard input, the regions or the
/// fault on standard output; Vision as it chooses, then on the CPU.
pub fn child_main() -> i32 {
    let (mut input, mut output) = (std::io::stdin().lock(), std::io::stdout().lock());
    crate::vision_child::serve(&mut input, &mut output, &[&NativeVision, &CpuVision])
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
