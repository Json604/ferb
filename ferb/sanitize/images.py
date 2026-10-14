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
from ferb.sanitize.redact import OTP_LABEL, Redactor, labelled_secret_spans, payment_card

# OCR often merges a label and its number into one region ("Card4111…", "visa-4111…"); text redaction leaves a card
# glued to a word alone (L16). The mask decision checks digits and the card checksum alone: a false alarm only blurs.
CARD_DIGITS = re.compile(r"\d(?:[ -]?\d){12,18}")


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
            # C1: a code word or secret word on one line reaches a code or value on the next ("Enter the code we
            # sent" over "48 29 13", "Password:" beside "sunshine2024"). Each line is read with the one after it (in
            # Vision's order) as one text, so the word reach is the text redactor's but never runs past a neighbour.
            labelled: set[int] = set()
            for i, region in enumerate(regions[:-1]):
                pair = region.text + "\n" + regions[i + 1].text
                for a, b in labelled_secret_spans(pair):
                    labelled.update(j for j, (lo, hi) in ((i, (0, len(region.text))),
                                                          (i + 1, (len(region.text) + 1, len(pair))))
                                    if a < hi and lo < b)
            for i, region in enumerate(regions):
                text = region.text
                if (self.redactor.text(text) != text or any(payment_card(m.group()) for m in CARD_DIGITS.finditer(text))
                        or (code_frame and re.search(r'(?<!\d)\d(?:[ -]?\d){3,7}(?!\d)', text))
                        or i in labelled):
                    boxes.append(region.box)
            return self.mask(image, boxes) if boxes else image
        except Exception:
            raise RuntimeError('image redaction unavailable; pixels withheld') from None
