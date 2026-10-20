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


def test_a_field_the_ocr_reads_as_password_dots_is_masked() -> None:
    """Fix wave 4: where a capture's secret fields couldn't be located, text recognition is the only layer left; a
    region it reads as nothing but a row of password dots is a password field, painted over like the others."""
    boxes = []
    def mask(image, rects):
        boxes.extend(rects)
        return b'safe'
    regions = [TextRegion('••••••••', (1, 1, 9, 9)), TextRegion('● ● ● ● ● ●', (2, 2, 9, 9)),
               TextRegion('************', (3, 3, 9, 9)), TextRegion('Password', (4, 4, 9, 9)),
               TextRegion('•••', (5, 5, 9, 9)), TextRegion('Rated •••••', (6, 6, 9, 9)),
               TextRegion('Visa •••• 4242', (7, 7, 9, 9))]
    assert ImageRedactor(Redactor(), recognize=lambda _: regions, mask=mask)(b'raw') == b'safe'
    assert boxes == [(1, 1, 9, 9), (2, 2, 9, 9), (3, 3, 9, 9)]  # three dots, a rating, a card's last four stay


def test_runs_with_periods_are_never_painted_so_ellipses_and_leaders_stay() -> None:
    """Review M3: Vision seldom reads password bullets cleanly (offline renders gave '........•', '..•••••', '.......',
    or nothing). Matching period runs too caught few more of them and painted a leader Vision split off a contents
    line, so periods never count: only a region that is all bullets or asterisks is painted."""
    boxes = []
    def mask(image, rects):
        boxes.extend(rects)
        return b'safe'
    painted = ['**********', '••••••••••', '* * * * * *']
    kept = ['........•', '..•••••', '.......', '........', 'Loading...', 'Chapter 1 ........ 5', '...', 'Wait…', '..5']
    regions = [TextRegion(t, (i, i, 9, 9)) for i, t in enumerate(painted + kept)]
    ImageRedactor(Redactor(), recognize=lambda _: regions, mask=mask)(b'raw')
    assert boxes == [(i, i, 9, 9) for i in range(len(painted))]
