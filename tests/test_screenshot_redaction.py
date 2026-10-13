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


def test_ocr_card_glued_to_its_label_is_still_masked() -> None:
    """OCR often merges a label and its number into one region ("Card4111…"). Text redaction leaves a card glued to
    a word alone (L16); pixel masking uses digits and the checksum alone, since a false alarm only blurs pixels."""
    boxes = []
    def mask(image, rects):
        boxes.extend(rects)
        return b'safe'
    regions = [TextRegion('Card4111111111111111', (1, 1, 9, 9)), TextRegion('visa-4111 1111 1111 1111', (2, 2, 9, 9)),
               TextRegion('Order4111111111111112 shipped', (3, 3, 9, 9))]
    assert ImageRedactor(Redactor(), recognize=lambda _: regions, mask=mask)(b'raw') == b'safe'
    assert boxes == [(1, 1, 9, 9), (2, 2, 9, 9)]                 # no checksum, no card: the order number stays
