"""D13: only sanitized image bytes reach an image sink; OCR failures are not bypasses."""
import pytest
from ferb.sanitize.images import ImageRedactor, TextRegion
from ferb.sanitize.redact import Redactor


def test_ocr_secret_line_and_ax_boxes_masked_before_return() -> None:
    boxes = []
    def mask(image, rects):
        boxes.extend(rects)
        return b'safe'
    red = ImageRedactor(Redactor(), recognize=lambda _: [TextRegion('Code: 123456', (1, 2, 50, 10)),
        TextRegion('verification code', (1, 0, 50, 2)), TextRegion('4111 1111 1111 1111', (5, 40, 80, 10))], mask=mask)
    assert red(b'raw', [(0, 80, 20, 10)]) == b'safe'
    assert (0, 80, 20, 10) in boxes and (5, 40, 80, 10) in boxes and (1, 2, 50, 10) in boxes


def test_ocr_error_never_returns_raw_pixels_or_exception_content() -> None:
    def fail(_):
        raise RuntimeError('sensitive native diagnostic')
    with pytest.raises(RuntimeError, match='image redaction unavailable') as error:
        ImageRedactor(Redactor(), recognize=fail)(b'raw')
    assert 'sensitive' not in str(error.value)
