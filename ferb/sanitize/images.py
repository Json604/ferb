"""On-device OCR and opaque masking before an image is stored or sent. No hosted OCR, raw text, or raw fallback.

Vision's accurate recognizer is the platform default. OCR is class recognition, not a guarantee that every rendered
secret is readable; AX password boxes and the extension's secret-field rects remain independent defenses. Where a
capture's secret fields couldn't be located, this is the only layer, and the capture's note says so (L13).
https://developer.apple.com/documentation/vision/recognizing-text-in-images
"""
from dataclasses import dataclass
import math
import re
from typing import Callable
from ferb.act.evidence.mask import Box, mask_secret_fields
from ferb.sanitize.redact import (CODE_LINK, CODE_REACH, OTP_LABEL, Redactor, _H, _code_alone, code_words,
                                  payment_card, secret_words)

# Any whitespace but a line break reads as a space here too, as in the text redactor's classes (`_H`; re-review 4).
# OCR often merges a label and its number into one region ("Card4111…", "visa-4111…"); text redaction leaves a card
# glued to a word alone (L16). The mask decision checks digits and the card checksum alone: a false alarm only blurs.
CARD_DIGITS = re.compile(rf"\d(?:[{_H}-]?\d){{12,18}}")
# A number in a frame that shows an OTP label (`ImageRedactor`).
FRAME_CODE = re.compile(rf"(?<!\d)\d(?:[{_H}-]?\d){{3,7}}(?!\d)")
# A region that reads as nothing but four or more bullets or asterisks (spaces aside) is painted: a password field's
# dots. Measured on offline renders (fix wave 4 round 1, review M3): Vision reads a row of asterisks this way about
# half the time (10/18) but a row of bullets almost never (1/72: it returns '........•', '..•••••', '.......', or
# nothing), so this is no protection to count on. Runs with periods are not matched: allowing them caught 6/72 bullet
# rows and painted a table-of-contents leader Vision split off on its own.
PASSWORD_DOTS = re.compile(r"[•●∙⋅·◦*]{4,}")


def neighbour_spans(lines: list[str]) -> set[int]:
    """The OCR lines (by index) that hold a labelled secret only with their neighbour (C1): a code alone on its line
    under a line with a code word near its end ("Enter the code we sent you" over "48 29 13"), a value alone beside or
    under a line ending in a secret word ("Password:" by "sunshine2024"), or a code alone on its line over a line that
    starts with its link and code word ("482913" over "is your verification code"). A code alone is the text
    redactor's (`_code_alone`: brackets, punctuation or a list bullet around it). Single lines are the text
    redactor's."""
    out: set[int] = set()
    for i in range(len(lines) - 1):
        a, b = lines[i].strip(), lines[i + 1].strip()
        code_b = _code_alone(b)
        near_end = [m for m in code_words(a) if len(a[m.end():].split()) <= CODE_REACH]
        if code_b and near_end:
            out.add(i + 1)
        secret = [m for m in secret_words(a) if re.fullmatch(rf"[{_H}:=]*", a[m.end():])]
        if secret and b and len(b.split()) == 1 and (a.endswith((":", "=")) or any(c.isdigit() for c in b)):
            out.add(i + 1)
        if _code_alone(a) and CODE_LINK.match(b) and any(
                len(b[:m.start()].split()) <= CODE_REACH for m in code_words(b)):
            out.add(i)
    return out


@dataclass(frozen=True)
class TextRegion:
    text: str
    box: Box


def recognize_text(image: bytes) -> list[TextRegion]:
    import Vision
    import Quartz
    from Foundation import NSData, NSDictionary
    request = Vision.VNRecognizeTextRequest.alloc().init()
    request.setUsesLanguageCorrection_(False)  # credentials must not be corrected to dictionary words
    # An NSDictionary, never a Python {}: on macOS 27.2 Vision refuses PyObjC's dict proxy ("key does not exist"), so
    # every image failed recognition and was withheld (found in step 12d)
    handler = Vision.VNImageRequestHandler.alloc().initWithData_options_(
        NSData.dataWithBytes_length_(image, len(image)), NSDictionary.dictionary())
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
            # C1: a code word or secret word on one line reaches a code or value on the line after it, in Vision's
            # order (`neighbour_spans`). A region the text redactor finds any span in (the same rules as text) is
            # painted whole.
            labelled = neighbour_spans([r.text for r in regions])
            for i, region in enumerate(regions):
                text = region.text
                if (self.redactor.spans(text) or any(payment_card(m.group()) for m in CARD_DIGITS.finditer(text))
                        or (code_frame and FRAME_CODE.search(text))
                        or i in labelled or PASSWORD_DOTS.fullmatch(re.sub(r"\s", "", text))):
                    boxes.append(region.box)
            return self.mask(image, boxes) if boxes else image
        except Exception:
            raise RuntimeError('image redaction unavailable; pixels withheld') from None
