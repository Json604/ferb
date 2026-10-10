"""On-device OCR and opaque masking before an image is stored or sent. No hosted OCR, raw text, or raw fallback.

Vision's accurate recognizer is the platform default. OCR is class recognition, not a guarantee that every rendered
secret is readable; AX password boxes and the existing incomplete-inventory refusal remain independent defenses.
https://developer.apple.com/documentation/vision/recognizing-text-in-images
"""
from dataclasses import dataclass
import math
import re
from typing import Callable
from ferb.act.evidence.mask import Box, mask_secret_fields
from ferb.sanitize.redact import OTP_LABEL, Redactor


@dataclass(frozen=True)
class TextRegion:
    text: str
    box: Box


def recognize_text(image: bytes) -> list[TextRegion]:
    import Vision
    import Quartz
    from Foundation import NSData
    request = Vision.VNRecognizeTextRequest.alloc().init()
    request.setUsesLanguageCorrection_(False)  # credentials must not be corrected to dictionary words
    handler = Vision.VNImageRequestHandler.alloc().initWithData_options_(
        NSData.dataWithBytes_length_(image, len(image)), {})
    ok, error = handler.performRequests_error_([request], None)
    if not ok or error is not None:
        raise RuntimeError('on-device text recognition failed')
    source = Quartz.CGImageSourceCreateWithData(image, None)
    decoded = Quartz.CGImageSourceCreateImageAtIndex(source, 0, None)
    width, height = Quartz.CGImageGetWidth(decoded), Quartz.CGImageGetHeight(decoded)
    regions = []
    for observation in request.results() or []:
        candidates = observation.topCandidates_(1)
        if not candidates:
            continue
        b = observation.boundingBox()
        x, y = math.floor(b.origin.x * width), math.floor((1 - b.origin.y - b.size.height) * height)
        right, bottom = math.ceil((b.origin.x + b.size.width) * width), math.ceil((1 - b.origin.y) * height)
        regions.append(TextRegion(str(candidates[0].string()), (x, y, right - x, bottom - y)))
    return regions


class ImageRedactor:
    def __init__(self, redactor: Redactor, *, recognize: Callable | None = None,
                 mask: Callable = mask_secret_fields) -> None:
        self.redactor, self.recognize, self.mask = redactor, recognize or recognize_text, mask

    def __call__(self, image: bytes, rects: list[Box] | None = None) -> bytes:
        try:
            regions = self.recognize(image)
            boxes = list(rects or [])
            # Codes are often on their own line underneath the label. A visible OTP label makes numeric OCR
            # regions secret fields for this frame; ordinary private sentences stay visible.
            code_frame = any(OTP_LABEL.search(r.text) for r in regions)
            for region in regions:
                text = region.text
                if self.redactor.text(text) != text or (code_frame and re.search(r'(?<!\d)\d(?:[ -]?\d){3,7}(?!\d)', text)):
                    boxes.append(region.box)
            return self.mask(image, boxes) if boxes else image
        except Exception:
            raise RuntimeError('image redaction unavailable; pixels withheld') from None
