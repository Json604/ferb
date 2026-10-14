"""C1 (owner decision 2026-10-15, rule 14): a one-time code is a 4-8 digit run with a code word a few words away on
either side, and a value next to a secret word is masked whatever its shape. Labelled by words, never by digit shape
alone: an ordinary number with no code or secret word near it stays visible. Every capture path uses the one class."""
import pytest

from ferb.sanitize.images import ImageRedactor, TextRegion
from ferb.sanitize.redact import REDACTED, Redactor

# The whole-codebase review's provider corpus (codebase-review.md C1): every code must be masked.
PROVIDER_CORPUS = [
    ("123456 is your Instagram code. Don't share it.", "123456"),
    ("Your OTP for HDFC Bank txn of Rs 2,000 is 482913. Valid for 10 mins.", "482913"),
    ("G-482913 is your Google verification code.", "482913"),
    ("482913 is your verification code for WhatsApp", "482913"),
    ("Dear customer, 739201 is the One Time Password (OTP) for your transaction", "739201"),
    ("Your Uber code is 4829. Never share this code.", "4829"),
    ("Apple ID Code: 482913. Don't share it with anyone.", "482913"),
    ("Your Amazon code: 482-913", "482-913"),
    ("Enter code 48 29 13 to continue", "48 29 13"),
    ("Your Zomato login OTP is 4829", "4829"),
    ("OTP 482913", "482913"),
    ("Use 482913 to verify your Swiggy account", "482913"),
    ("Your passcode is 7392", "7392"),
    ("PIN 4821 for your card", "4821"),
    ("Your one-time password is 55 12 90", "55 12 90"),
]


@pytest.mark.parametrize("message,code", PROVIDER_CORPUS)
def test_provider_codes_are_masked_with_the_code_word_on_either_side(message, code) -> None:
    out = Redactor().text(message)
    assert code not in out and REDACTED in out, out


def test_masking_twice_changes_nothing() -> None:
    r = Redactor()
    for text in ("my token is ghp_abcdefghijklmnopqrstuvwxyz0123 keep it", "password: hunter2",
                 "123456 is your Instagram code", "my password is [redacted] ok"):
        once = r.text(text)
        assert r.text(once) == once, once


def test_a_code_in_parentheses_after_its_word_is_masked() -> None:
    assert Redactor().text("Your code (482913) expires in 10 minutes") == \
        "Your code ([redacted]) expires in 10 minutes"


def test_the_google_prefix_form_is_masked_whole() -> None:
    assert Redactor().text("G-482913 is your Google verification code.") == \
        "[redacted] is your Google verification code."


# "Words with secret masked too": a value next to a secret word, digits or not.
SECRET_WORD_CASES = [
    ("password: sunshine2024", "sunshine2024"),
    ("my api key is sk_live_51HxTestValue0000", "sk_live_51HxTestValue0000"),
    ("my password is Tr0ub4dor&3", "Tr0ub4dor&3"),
    ("password: hunter2", "hunter2"),
    ("my password is sunshine", "sunshine"),
    ("The wifi password is hunter2.", "hunter2"),
    ("my password for gmail is bluebird77", "bluebird77"),
    ("secret: correcthorse", "correcthorse"),
    ("API_KEY=abcd1234efgh", "abcd1234efgh"),
    ("export GITHUB_TOKEN=deadbeef1234", "deadbeef1234"),
    ("access token: tok.abc-def_1234", "tok.abc-def_1234"),
    ("private key = mykeymaterial", "mykeymaterial"),
    ('{"password": "correct horse battery"}', "correct horse battery"),
    ("my PIN is 4821", "4821"),
    ("passphrase: purple monkey", "purple"),
]


@pytest.mark.parametrize("message,value", SECRET_WORD_CASES)
def test_a_value_next_to_a_secret_word_is_masked(message, value) -> None:
    out = Redactor().text(message)
    assert value not in out and REDACTED in out, out


# Ordinary numbers and secret words without a value stay readable: a page's error, a price, a date, a phone number.
NEGATIVES = [
    "Your password is incorrect.",
    "Password is required",
    "I forgot my password",
    "Reset your password here",
    "The token is invalid or expired.",
    "API key missing",
    "Paid Rs 2,000 at 10:52 for order 48213",
    "Total: ₹1,499.00 (3 items)",
    "Meeting on 15 Oct 2026 at 4pm",
    "Deadline 2026-10-21",
    "Call +91 98765 43210 or 555-0143",
    "the 2024 code review",
    "2024 code review notes",
    "I wrote 5000 lines of code yesterday",
    "HTTP status code 404",
    "The year is 2026 and invoice 314159 is paid.",
    "Secret Santa 2024 list",
    "input tokens: 12345",
    "max_tokens=4096",
    "see https://github.com/acme/code/pull/4821 for it",
    "Pin tab 2 to the left",
    "Apps you use: Code (3733 recorded uses)",          # VS Code's name: nothing joins the number to it as its value
    "code review on 12 Oct 2024",
    "PIN for the card ending 1234 was reset",
]


@pytest.mark.parametrize("message", NEGATIVES)
def test_ordinary_text_without_a_labelled_value_stays_visible(message) -> None:
    assert Redactor().text(message) == message


def test_existing_classes_stay_intact() -> None:
    r = Redactor()
    assert r.text("Your verification code is 314159.") == "Your verification code is [redacted]."
    assert r.text("Card 4111 1111 1111 1111") == "Card [redacted]"
    assert "sk-ant-api03-abcdefghijklmnopqrst" not in r.text("key sk-ant-api03-abcdefghijklmnopqrst")


def test_notification_records_are_masked_through_the_dict_walk() -> None:
    data = {"bundle_id": "com.apple.MobileSMS", "title": "AX-WHATSP",
            "body": "482913 is your verification code for WhatsApp"}
    assert "482913" not in str(Redactor()(data))


@pytest.mark.parametrize("label,value", [
    ("Enter code", "482913"), ("Code", "48 29 13"), ("PIN", "4821"), ("Passcode", "7392"),
    ("API key", "abc-def-ghi"), ("Access token", "anything here"), ("Secret", "sunshine"),
    ("Private key", "material"),
])
def test_typed_text_in_a_field_named_by_a_code_or_secret_word_is_masked(label, value) -> None:
    assert Redactor().field(value, role="AXTextField", label=label) == REDACTED


@pytest.mark.parametrize("label,value", [
    ("Promo code", "SAVE20"),            # a code word, but no code-shaped value
    ("Search", "482913"),                # no label: the owner rejected a digit-shape rule for typed text
    ("invoice number", "314159"),
    ("Message", "my number is 98765 43210"),
])
def test_typed_text_without_a_labelled_secret_stays(label, value) -> None:
    assert Redactor().field(value, role="AXTextField", label=label) == value


def test_a_secret_word_label_on_a_checkbox_never_hides_its_state() -> None:
    assert Redactor().field("1", role="AXCheckBox", label="Pin tab") == "1"


def test_answer_to_a_question_asking_for_a_code_is_hidden() -> None:
    assert Redactor().answer("4829", question="What's the code on the Uber screen?") == REDACTED
    assert Redactor().answer("yes", question="Should I reset your password?") == "yes"


def _masked(regions: list[TextRegion]) -> list[str]:
    seen: list = []

    def mask(image, boxes):
        seen.extend(boxes)
        return b"masked"
    ImageRedactor(Redactor(), recognize=lambda _: regions, mask=mask)(b"raw")
    return [r.text for r in regions if r.box in seen]


def test_ocr_masks_a_code_with_its_word_in_the_same_region() -> None:
    assert _masked([TextRegion("Your Uber code is 4829", (0, 0, 9, 9))]) == ["Your Uber code is 4829"]


def test_ocr_masks_a_code_on_the_line_under_its_code_word() -> None:
    regions = [TextRegion("Enter the code we sent you", (0, 0, 90, 9)), TextRegion("48 29 13", (0, 12, 40, 9)),
               TextRegion("Order 2024 shipped", (0, 400, 60, 9))]
    assert _masked(regions) == ["48 29 13"]


def test_ocr_masks_a_value_under_a_secret_word_label() -> None:
    regions = [TextRegion("Password:", (0, 0, 40, 9)), TextRegion("sunshine2024", (50, 0, 60, 9))]
    assert _masked(regions) == ["sunshine2024"]


def test_ocr_leaves_ordinary_numbers_near_no_code_word() -> None:
    assert _masked([TextRegion("the 2024 code review", (0, 0, 9, 9))]) == []
    regions = [TextRegion("Invoice 2026-10-15", (0, 0, 9, 9)), TextRegion("Total 4999", (0, 20, 9, 9)),
               TextRegion("Enter the code we sent you", (0, 40, 9, 9)), TextRegion("Resend", (0, 60, 9, 9)),
               TextRegion("Order 48213 shipped", (0, 80, 9, 9))]
    assert _masked(regions) == []          # the code word's neighbour holds no number; the line after it is too far


STREAMED = [
    "482913 is your verification code for WhatsApp\n",
    "Your OTP for HDFC Bank txn of Rs 2,000 is 482913. Valid for 10 mins.\n",
    "password: 48291234 and then some more ordinary words follow here\n",
    "my wifi password is sunshine and nothing else\n",
    "482913 is your login code w1 w2 w3 w4 w5 w6 w7 w8 w9 w10\n",
    "Enter code\n48 29 13\n",
]


@pytest.mark.parametrize("raw", STREAMED)
def test_streamed_codes_and_secret_values_are_safe_at_every_split(raw) -> None:
    redactor = Redactor()
    expected = redactor.text(raw).encode()
    data = raw.encode()
    for split in range(len(data) + 1):
        stream = redactor.stream()
        assert stream.feed(data[:split]) + stream.feed(data[split:], final=True) == expected, split
    stream = redactor.stream()
    assert b"".join(stream.feed(bytes([b])) for b in data) + stream.feed(b"", final=True) == expected


def test_a_stream_never_holds_a_line_with_a_number_and_no_value_link() -> None:
    """A dev server prints its port and waits: the line must reach the reader (the egress-proxy test's server)."""
    stream = Redactor().stream()
    assert stream.feed(b"port 54321\n") == b"port 54321\n"
    assert stream.feed(b"Code server ready (3733 files)\n") == b"Code server ready (3733 files)\n"
    assert stream.feed(b"Downloaded 12345 bytes\r") == b"Downloaded 12345 bytes\r"
