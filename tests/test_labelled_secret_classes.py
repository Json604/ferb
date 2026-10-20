"""C1 (owner decision 2026-10-15, rule 14): a one-time code is a 4-8 digit run with a code word a few words away on
either side, and a value next to a secret word is masked whatever its shape. Labelled by words, never by digit shape
alone: an ordinary number with no code or secret word near it stays visible. Every capture path uses the one class."""
import re
from typing import Any

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
    ("Your passcode is 7392", "7392"),
    ("PIN 4821 for your card", "4821"),
    ("Your one-time password is 55 12 90", "55 12 90"),
    # re-review 2 (C2): an abbreviation's dot is no sentence end
    ("Your OTP for HDFC Bank txn of Rs. 2,000 is 482913. Valid for 10 mins.", "482913"),
    ("Your OTP for Rs. 500 txn at Swiggy is 482913", "482913"),
    ("OTP for a/c no. XX1234 is 482913", "482913"),
    ("Verification code for Ref. No. 4417 is 739201", "739201"),
]


@pytest.mark.parametrize("message,code", PROVIDER_CORPUS)
def test_provider_codes_are_masked_with_the_code_word_on_either_side(message, code) -> None:
    out = Redactor().text(message)
    assert code not in out and REDACTED in out, out


def test_masking_twice_changes_nothing() -> None:
    r = Redactor()
    for text in ("my token is ghp_abcdefghijklmnopqrstuvwxyz0123 keep it", "password: hunter2",
                 "We sent a verification code to +91 98765 43210",
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


@pytest.mark.parametrize("question,answer", [
    ("What's the verification code on the Uber screen?", "4829"), ("Please enter the OTP on the Zomato page.", "314159"),
    ("What's your UPI PIN?", "4821"), ("Which one-time password did the bank send?", "48 29 13"),
    ("What's the login code?", "482913"),
])
def test_an_answer_to_a_question_asking_for_a_one_time_code_or_sign_in_pin_is_hidden(question, answer) -> None:
    assert Redactor().answer(answer, question=question) == REDACTED


@pytest.mark.parametrize("question,answer", [
    ("What's the PIN code for the delivery address?", "560001"),      # review of fix wave 2 (probe p4)
    ("Which coupon code should I apply?", "SAVE2024"),
    ("How many tabs should I keep open?", "4000"),
    ("Which port should the code server use?", "the default one"),
])
def test_an_answer_with_no_code_to_a_code_or_address_question_stays(question, answer) -> None:
    assert Redactor().answer(answer, question=question) == answer


@pytest.mark.parametrize("question,answer", [
    ("What's your Gmail password?", "hunter2!"), ("Enter the passcode for the office door", "correct horse"),
    ("What's your UPI PIN?", "4821"), ("Should I reset your password?", "yes"),
    ("What's the password for Netflix?", "hunter2!"), ("Password to the office wifi?", "sunshine"),
    # re-review 3 (R1-a): any question with a password word, however it is phrased
    ("What password do you use for Netflix?", "hunter2!"), ("What's the password you use for Gmail?", "s3cret pass"),
    ("Can you tell me the Netflix password so I can sign in?", "hunter2!"), ("Type your password here", "hunter2!"),
    ("Password please", "hunter2!"), ("the password and username for Netflix", "kartikey hunter2!"),
    ("Do you have a password I can use for the router?", "sunshine"), ("What's the passphrase?", "correct horse"),
    ("Which password manager do you use?", "Bitwarden"), ("Should I open the password reset page?", "yes please"),
    # re-review 4 (M1): "pwd" and "passwd" are password words, as for the secret-word class
    ("What's the wifi pwd?", "hunter2!"), ("Enter the router passwd", "correct horse"), ("Wifi PWD?", "sunshine"),
    ("What are the passwds for the two routers?", "a1 b2"),
])
def test_an_answer_to_a_password_or_pin_question_is_hidden_whole_whatever_its_shape(question, answer) -> None:
    """Re-review 2/3 (R1, R1-a; rule 14 over convenience): a question with a password, passcode or passphrase word or a
    sign-in PIN hides the typed answer whole, whatever its shape or the question's phrasing. A question that mentions
    passwords without asking for one is asked with options; a picked option is never hidden."""
    assert Redactor().answer(answer, question=question) == REDACTED


@pytest.mark.parametrize("question,answer,expected", [
    ("What 6-digit code did Swiggy send you?", "482913", REDACTED),
    ("What's the code on the Uber screen?", "4829", REDACTED),
    ("What's the code on the Uber screen?", "it says 4829", f"it says {REDACTED}"),
    ("Which port should the code server use?", "8080", REDACTED),
    ("Which year's tax code should I check?", "2024", REDACTED),
])
def test_the_code_in_an_answer_to_a_code_question_is_masked(question, answer, expected) -> None:
    """Re-review 2 (R1): a question with the owner's code words (code, OTP, passcode, PIN, one-time password,
    verification; never an address or coupon code) masks the 4-8 digit code in the answer, as a code beside its word
    is masked everywhere. A port or year answered to such a question is masked too: the owner's rule is by words."""
    assert Redactor().answer(answer, question=question) == expected


@pytest.mark.parametrize("question,reply,expected", [
    ("What's your Gmail password?", "hunter2!", REDACTED),
    ("What's your Gmail password?", "wait, use the work account instead", "wait, use the work account instead"),
    ("What's the code on the Uber screen?", "4829 and then book it", f"{REDACTED} and then book it"),
])
def test_a_chat_reply_while_a_password_question_is_open_is_hidden_when_it_is_one_word(question, reply,
                                                                                      expected) -> None:
    """A chat message while a question is open may be a steer, not the answer (L3): a password question hides it whole
    when it has a password's shape (one word); its codes are masked either way."""
    assert Redactor().answer(reply, question=question, reply=True) == expected


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
    "Your OTP for Rs. 500 txn at Swiggy is 482913\n",                   # re-review 2 (C2)
    "OTP for a/c no. XX1234 is 482913\n",
    "Verification code for Ref. No. 4417 is 739201\n",
    "Use this code to sign in\n482913\n",                               # re-review 2 (R2)
    "[Swiggy] 482913 is your OTP\r\n",                                 # re-review 2 (I1, C1)
    "Your OTP for HDFC Bank txn of Rs 2,000 is 482913. Valid for 10 mins.\n",
    "password: 48291234 and then some more ordinary words follow here\n",
    "my wifi password is sunshine and nothing else\n",
    "482913 is your login code w1 w2 w3 w4 w5 w6 w7 w8 w9 w10\n",
    "Enter code\n48 29 13\n",
    # review of fix wave 2: a complete line ending in its code word kept the code before it visible
    "123456 is your Instagram code\n",
    "482913 is your Swiggy OTP\n",
    "G-482913 is your Google verification code\n",
    "Your OTP is 482913. Use 739201 as your PIN\n",
    "api key 482-913\n",
    "Your verification code is:\n\n482913\n\nIt expires in 10 minutes.\n",
    "482913\nis your verification code\n",
    "my private key = material9\n",
    "Recovery codes:\n4821-9921\n7731-1123\nKeep them safe.\n",
    "GitHub recovery codes:\na1b2c-3d4e5\nf6g7h-8i9j0\n",
    "my PIN is on the card and my password is sunshine\n",
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


def test_a_record_with_no_field_kind_is_never_masked_by_its_name() -> None:
    """Like the password rule: a dict naming no field kind (a chat channel, a file) keeps its text whatever its name."""
    r = Redactor()
    assert r({"name": "secret-santa", "text": "gifts are due friday"})["text"] == "gifts are due friday"
    assert r({"label": "Token budget", "value": "32768"})["value"] == "32768"
    assert r({"role": "AXTextField", "label": "API key", "value": "abc-def"})["value"] == REDACTED



@pytest.mark.parametrize("raw", STREAMED)
def test_a_whole_line_in_one_read_is_masked_as_the_text_redactor_masks_it(raw) -> None:
    redactor = Redactor()
    stream = redactor.stream()
    out = stream.feed(raw.encode()) + stream.feed(b"", final=True)
    assert out.decode() == redactor.text(raw)


def _streamed(redactor: Redactor, text: str, sizes: Any) -> str:
    data, stream, out, pos = text.encode(), redactor.stream(), b"", 0
    while pos < len(data):
        n = sizes()
        out += stream.feed(data[pos:pos + n])
        pos += n
    return (out + stream.feed(b"", final=True)).decode()


def test_the_stream_never_masks_less_than_the_text_redactor() -> None:
    """Review of fix wave 2 (probes p9, p11): random owner-like lines, read whole or in random pieces, never come out
    of the stream with fewer masks than `Redactor.text` gives the same text."""
    import random
    vocab = ["code", "OTP", "PIN", "password", "my", "token", "api key", "api", "key", "is", "to", ":", "-", "(", ")",
             "the", "your", "482913", "4829", "48 29 13", "482-913", "G-482913", "2024", "for", "verification",
             "one-time password", "secret", "=", "sunshine", "hunter2!", "was", "our", "wifi", "port", "54321",
             "words", "here", "Enter", "\n", "\n\n", "private", "as", ".", ",", "codes:", "a1b2c-3d4e5",
             "4821-9921", "and", "on", "\r\n", "\r", "sign in", "sign", "in", "Rs.", "No.", "a/c no.", "[Swiggy]",
             "[x]", "1000", "1001", "[y/N]", "Port (default 8080):", "coupon code", "PIN code",
             # re-review 4 (NEW-C): lines indented, or blank, by Unicode whitespace
             "\xa0", "\u2003", "\x0c", "\u3000", "\n\xa0", "\n\xa0482913", "\r\n\u2002482-913", "\n\x0b(G-482913)",
             "\n\u3000\n", "\n\xa0a1b2c-3d4e5",
             # re-review 4 (T1): blank lines under a label, item numbers, next-line values with spaces
             "Backup codes:\n\n", "Password:\n \n", "\n1. a1b2c-3d4e5", "\n2) 4821 9921", "\n\u2022 q2w3e4r5", "1.",
             "\n  s3cret pass 9", "pwd:", "\xa0codes:"]
    rng = random.Random(11)
    redactor = Redactor()
    for _ in range(6000):
        text = " ".join(rng.choice(vocab) for _ in range(rng.randint(2, 14))) + rng.choice(["", "\n"])
        whole = redactor.text(text).count(REDACTED)
        assert _streamed(redactor, text, lambda: 10 ** 9).count(REDACTED) >= whole, text
        assert _streamed(redactor, text, lambda: rng.randint(1, 8)).count(REDACTED) >= whole, text



# Review of fix wave 2 (probe p5): owners' own requests keep their numbers. "verify" is not one of the owner's code
# words (code, OTP, passcode, PIN, one-time password, verification), and a code never reaches its word across a
# sentence or clause break.
OWNER_REQUESTS = [
    ("check if my Amazon order 4821 is delivered and verify the address", ["4821"]),
    ("find invoice 48213 - verify the GST amount", ["48213"]),
    ("flat 1204 - verify the address on Swiggy", ["1204"]),
    ("the meeting is at 1530 - verify with Rahul", ["1530"]),
    ("track order 405-1234567 to verify delivery", ["405-1234567"]),
    ("PR 4821 is ready, please review the code", ["4821"]),
    ("issue 4821 - code freeze", ["4821"]),
    ("Build 20241015 is out; verify on staging", ["20241015"]),
    ("Pay 2500 to Rahul and verify on WhatsApp", ["2500"]),
    ("send 5000 to mom, verify the UPI PIN prompt yourself", ["5000"]),
    ("remind me at 1730 to verify the code", ["1730"]),
    ("set a timer for 1500 to check the OTP", ["1500"]),
    ("Rs 4999 is the price. Promo code SAVE10 applies.", ["4999"]),
    ("Server started on port 3000: verify at http://localhost:3000/verify", ["3000"]),
    ("tests/test_otp.py:1234: AssertionError", ["1234"]),
    ("Sept 2024: code red", ["2024"]),
    ("Line 1234: code smell", ["1234"]),
    ("We sent a verification code to +91 98765 43210", ["91 98765 43210"]),
]


@pytest.mark.parametrize("message,kept", OWNER_REQUESTS)
def test_owners_requests_keep_their_numbers(message, kept) -> None:
    out = Redactor().text(message)
    for number in kept:
        assert number in out, out


def test_a_code_after_a_sentence_break_is_still_masked_by_its_own_word() -> None:
    out = Redactor().text("Order 4821 is out for delivery. Share OTP 7731 with the agent")
    assert "4821" in out and "7731" not in out


# Owner decision 2026-10-15: address and coupon codes are ordinary data. "PIN code", "pincode", "ZIP code", "postal
# code", "coupon code", "promo code" (and close variants) are no code words; a login PIN or an OTP stays masked.
@pytest.mark.parametrize("message", [
    "Bangalore PIN code: 560001", "PIN code 560001", "Pincode 560001", "pin-code 560001", "pin_code: 560001",
    "ship to HSR Layout, Bangalore - 560102. PIN code 560102", "ZIP code 94107", "zip code 94103",
    "postal code 94103", "postal_code=560001", "Use coupon code 4455 at checkout", "promo code 2024",
    "Promo code SAVE10", "voucher code 7788",
])
def test_address_and_coupon_codes_stay_visible(message) -> None:
    assert Redactor().text(message) == message


@pytest.mark.parametrize("message,code", [
    ("your PIN is 4821", "4821"), ("Enter PIN 1234 to unlock", "1234"), ("UPI PIN: 4821", "4821"),
    ("Your OTP is 482913, delivery to PIN code 560001", "482913"),
])
def test_a_login_pin_or_otp_stays_masked(message, code) -> None:
    out = Redactor().text(message)
    assert code not in out
    if "560001" in message:
        assert "560001" in out


@pytest.mark.parametrize("label,value,kept", [
    ("PIN code", "560001", True), ("Pincode", "560001", True), ("Postal Code", "560102", True),
    ("ZIP code", "94107", True), ("Coupon code", "SAVE10", True), ("Promo code", "2024", True),
    ("UPI PIN", "4821", False), ("Enter OTP", "482913", False), ("Your PIN", "1234", False),
])
def test_typed_address_and_coupon_codes_stay_and_a_pin_hides(label, value, kept) -> None:
    out = Redactor().field(value, role="AXTextField", label=label)
    assert (out == value) is kept, (label, out)


def test_form_snapshot_nodes_keep_address_codes() -> None:
    r = Redactor()
    for node in ({"role": "textbox", "name": "PIN code", "value": "560001"},
                 {"role": "textbox", "name": "ZIP code", "value": "94103"},
                 {"tag": "input", "type": "text", "name": "pincode", "value": "560001"},
                 {"tag": "input", "type": "text", "name": "postal_code", "value": "560001"},
                 {"role": "textbox", "name": "Coupon code", "value": "2024"}):
        assert r(node)["value"] == node["value"], node
    assert r({"role": "textbox", "name": "UPI PIN", "value": "4821"})["value"] == REDACTED


def test_ocr_keeps_an_address_pin_code() -> None:
    assert _masked([TextRegion("PIN code", (0, 0, 40, 9)), TextRegion("560001", (0, 12, 40, 9))]) == []
    assert _masked([TextRegion("Bangalore PIN code: 560001", (0, 0, 90, 9))]) == []



@pytest.mark.parametrize("message,code", [
    ("Your verification code is:\n\n482913\n\nIt expires in 10 minutes.", "482913"),
    ("Use this code to sign in:\n482913", "482913"), ("Your OTP is\n482913", "482913"),
    ("Sign-in code\n\n482913", "482913"), ("482913\nis your verification code", "482913"),
])
def test_an_emails_code_alone_on_its_line_next_to_its_label_is_masked(message, code) -> None:
    assert code not in Redactor().text(message)


def test_a_line_that_only_holds_a_number_under_an_ordinary_line_stays() -> None:
    assert Redactor().text("Your order number is:\n482913") == "Your order number is:\n482913"


def test_snapshot_nodes_split_into_a_label_and_its_code_are_related() -> None:
    """Review of fix wave 2 (probe p6): a page shows "Your verification code" in one node and the code in the next."""
    r = Redactor()
    page = r({"elements": [{"role": "heading", "name": "Your verification code"},
                           {"role": "statictext", "value": "482913"},
                           {"role": "statictext", "value": "Order 778812"}]})["elements"]
    assert page[1]["value"] == REDACTED and page[2]["value"] == "Order 778812"
    below = r({"nodes": [{"role": "statictext", "text": "482913"},
                         {"role": "statictext", "text": "is your verification code"}]})["nodes"]
    assert below[0]["text"] == REDACTED
    plain = r({"elements": [{"role": "heading", "name": "Order number"}, {"role": "statictext", "value": "482913"}]})
    assert plain["elements"][1]["value"] == "482913"


@pytest.mark.parametrize("key", ["code", "access", "otp"])
def test_a_long_line_with_a_class_word_and_no_secret_is_never_replaced_by_the_oversized_marker(key) -> None:
    """Review of fix wave 2 (probe p8): a 130 KB JSON line holding "access" came out as the oversized marker."""
    import json
    items = [{"id": i, "name": f"pkg{i}", "description": "a library for parsing things quickly"} for i in range(1500)]
    line = json.dumps({key: 200, "items": items}) + "\n"
    stream, out, data = Redactor().stream(), b"", line.encode()
    for i in range(0, len(data), 4096):
        out += stream.feed(data[i:i + 4096])
    early = len(out)
    out += stream.feed(b"", final=True)
    assert out.decode() == line and early > len(data) // 2          # streamed as it came, not held to the end


def test_a_long_line_still_masks_a_code_near_its_end() -> None:
    text = "word " * 20000 + "your OTP is 482913\n"
    stream, out, data = Redactor().stream(), b"", text.encode()
    for i in range(0, len(data), 4096):
        out += stream.feed(data[i:i + 4096])
    out += stream.feed(b"", final=True)
    assert b"482913" not in out and b"oversized" not in out


@pytest.mark.parametrize("message,kept", [
    ("my employee id is 48213 and my PIN is on the card", ["on the card", "48213"]),
    ("my password is the one we agreed", ["the one we agreed"]),
])
def test_my_secret_word_hides_only_a_value_that_looks_like_one(message, kept) -> None:
    out = Redactor().text(message)
    for part in kept:
        assert part in out, out


@pytest.mark.parametrize("message,value", [
    ("my password is sunshine", "sunshine"), ("my password is fluffy, don't tell anyone", "fluffy"),
    ("our wifi password is bluebird and the router is upstairs", "bluebird"),
])
def test_my_secret_word_still_hides_a_plain_word_password(message, value) -> None:
    assert value not in Redactor().text(message)


@pytest.mark.parametrize("message,codes", [
    ("Backup codes: 1234 5678 9012 3456", ["1234", "5678", "9012", "3456"]),
    ("Your 2FA backup codes: 1234 5678, 9012 3456", ["1234", "5678", "9012", "3456"]),
    ("Recovery codes:\n4821-9921\n7731-1123\nKeep them safe.", ["4821-9921", "7731-1123"]),
    ("GitHub recovery codes:\na1b2c-3d4e5\nf6g7h-8i9j0", ["a1b2c-3d4e5", "f6g7h-8i9j0"]),
])
def test_every_code_in_a_backup_or_recovery_list_is_masked(message, codes) -> None:
    out = Redactor().text(message)
    for code in codes:
        assert code not in out, out
    if "Keep them safe" in message:
        assert "Keep them safe." in out


def test_many_short_lines_with_digits_stream_through_whole() -> None:
    """Full suite on the review round: 40000 lines "line0".."line39999" each looked like a list code, so the stream held
    them all and dropped them as oversized (test_mac_shell's paging tests). A list is held only under its label."""
    import time
    text = "".join(f"line{i}\n" for i in range(40000))
    stream, out, data = Redactor().stream(), b"", text.encode()
    started = time.monotonic()
    for i in range(0, len(data), 1024):
        out += stream.feed(data[i:i + 1024])
    out += stream.feed(b"", final=True)
    assert out.decode() == text and time.monotonic() - started < 10


# Re-review 2 (C2): an abbreviation's dot ("Rs.", "no.", "Ref.") is no sentence break, so a bank's OTP stays masked.
ABBREVIATED = [
    ("Your OTP for HDFC Bank txn of Rs. 2,000 is 482913. Valid for 10 mins.", "482913"),
    ("Your OTP for Rs. 500 txn at Swiggy is 482913", "482913"),
    ("OTP for a/c no. XX1234 is 482913", "482913"),
    ("Verification code for Ref. No. 4417 is 739201", "739201"),
]


@pytest.mark.parametrize("message,code", ABBREVIATED)
def test_an_abbreviation_dot_never_ends_the_clause(message, code) -> None:
    assert code not in Redactor().text(message)
    redactor = Redactor()
    data = (message + "\n").encode()
    for split in range(len(data) + 1):
        stream = redactor.stream()
        assert code.encode() not in stream.feed(data[:split]) + stream.feed(data[split:], final=True), split


def test_a_sentence_end_still_breaks_the_clause() -> None:
    out = Redactor().text("Order 4821 is out for delivery. Share OTP 7731 with the agent")
    assert "4821" in out and "7731" not in out
    assert "4999" in Redactor().text("Rs 4999 is the price. Promo code SAVE10 applies.")


@pytest.mark.parametrize("message", ["[Swiggy] 482913 is your OTP", "[10:42] 482913 is your Instagram code",
                                     "[#] 482913 is your verification code", "[Bank] Your OTP is 482913"])
def test_a_code_after_a_bracketed_prefix_is_masked(message) -> None:
    """Re-review 2 (I1): the idempotence guard took any "] " for a mask's end."""
    assert "482913" not in Redactor().text(message)


CRLF = [
    "Dear customer, 739201 is the One Time Password (OTP) for your transaction\r\n",
    "482913 is your Swiggy OTP, valid for 10 minutes.\r\n",
    "G-482913 is your Google verification code, do not share\r\n",
    "Your sign in code for Swiggy account login 482913\n",
    "Use this coupon code 4455 at checkout\n",
    "482913.\r\n(OTP)\r\nThanks\r\n",
    "sunshine\r\n482913.\r\nexit code 0\r\n482913.\r\nas your PIN\r\n",
]


@pytest.mark.parametrize("raw", CRLF)
def test_crlf_lines_and_split_words_stream_as_the_text_redactor_masks_them(raw) -> None:
    """Re-review 2 (C1): a read ending between "\\r" and "\\n" (PTY output is CRLF), or right after "sign ", leaked the
    code; a read ending after "coupon " over-masked an address-like coupon code."""
    redactor = Redactor()
    expected = redactor.text(raw).encode()
    data = raw.encode()
    for split in range(len(data) + 1):
        stream = redactor.stream()
        assert stream.feed(data[:split]) + stream.feed(data[split:], final=True) == expected, split
    if raw.endswith("\r\n"):
        stream = redactor.stream()
        out = stream.feed(data[:-1]) + stream.feed(b"\n") + stream.feed(b"", final=True)
        assert out == expected


@pytest.mark.parametrize("prompt", ["Delete 1234 files? [y/N] ", "Port (default 8080): ",
                                    "Proceed with 2048 MB download? (y/n) ",
                                    "Listening on http://127.0.0.1:8000 (Press CTRL+C to quit) "])
def test_an_interactive_prompt_with_a_number_appears_before_its_answer(prompt) -> None:
    """Re-review 2 (I3): a number on the unfinished line held the whole prompt until a newline that never comes."""
    stream = Redactor().stream()
    assert stream.feed(prompt.encode()) == prompt.encode()


@pytest.mark.parametrize("start", ["482913 is your ", "482913", "482913 i", "Your code is "])
def test_a_code_that_can_still_join_its_word_is_held(start) -> None:
    stream = Redactor().stream()
    assert b"482913" not in stream.feed(start.encode())


@pytest.mark.parametrize("text", ["".join(f"{n}\n" for n in range(1000, 30000)),
                                  "".join(f"{n * 7919 % 99999999:08d}\r\n" for n in range(6000))])
def test_number_only_output_streams_through_whole_and_holds_at_most_one_line(text) -> None:
    """Re-review 2 (I2): `seq 1000 29999` came out as the oversized marker and an id dump was held to the end, with
    seconds of work per read. A code-only line is held only for the line after it (its label or naming line can be
    there); neighbouring code-only lines never hold each other."""
    stream, out, data = Redactor().stream(), b"", text.encode()
    for i in range(0, len(data), 1024):
        out += stream.feed(data[i:i + 1024])
        assert len(stream.pending) <= 32, len(stream.pending)          # the work per read stays the read's own
    out += stream.feed(b"", final=True)
    assert out.decode() == text


@pytest.mark.parametrize("message", ["HTTP status codes: 200, 301, 404, 500", "Course codes: CS101, MA102",
                                     "allowed_codes: 200, 201, 204", "Response codes: 2001 2002 2003"])
def test_an_ordinary_codes_label_lists_ordinary_data(message) -> None:
    """Re-review 2 (I5): list masking is only for backup, recovery, 2FA and one-time code lists."""
    assert Redactor().text(message) == message


@pytest.mark.parametrize("raw", ["Use this code to sign in\n482913\n", "Your Swiggy verification code\n\n739201\n",
                                 "Here is your one-time password\r\n4829\r\n"])
def test_a_label_line_without_a_colon_labels_the_code_under_it(raw) -> None:
    """Re-review 2 (R2): the owner's rule is a code word within a few words on either side; a line break is
    whitespace. Text, every stream split and sibling nodes agree."""
    code = re.search(r"\d{4,8}", raw).group(0)
    redactor = Redactor()
    assert code not in redactor.text(raw)
    data = raw.encode()
    for split in range(len(data) + 1):
        stream = redactor.stream()
        assert code.encode() not in stream.feed(data[:split]) + stream.feed(data[split:], final=True), split
    lines = [line for line in raw.splitlines() if line.strip()]
    assert code not in str(redactor([{"text": line} for line in lines]))


def test_a_sentence_ended_before_the_line_end_labels_nothing() -> None:
    assert "2024" in Redactor().text("Your code expired. Request a new one\n2024\n")


@pytest.mark.parametrize("raw", ["Pin/Zip code: 560001\n", "PIN / ZIP code 560001\n", "Enter your ZIP code 94107\n",
                                 "Enter your postal code 560001\n"])
def test_an_address_code_label_stays_visible_whole_and_streamed(raw) -> None:
    """Re-review 2 (R3): "Pin/Zip code" is an address label; a read ending in "ZIP " or "postal " doesn't over-mask."""
    redactor = Redactor()
    assert redactor.text(raw) == raw
    data = raw.encode()
    for split in range(len(data) + 1):
        stream = redactor.stream()
        assert stream.feed(data[:split]) + stream.feed(data[split:], final=True) == data, split


@pytest.mark.parametrize("message", [m for m, _ in PROVIDER_CORPUS if "Rs." in m or "no." in m or "Ref." in m])
def test_ocr_masks_a_bank_otp_after_an_abbreviation(message) -> None:
    """Re-review 2 (C2): a screenshot of the SMS masks it as the text path does."""
    assert _masked([TextRegion(message, (0, 0, 300, 9))]) == [message]


@pytest.mark.parametrize("raw", ["Your OTP (y/n) wifi 48 29 13 my 1234 5678 token \n1002\n1003\n",
                                 "Backup codes: , 482-913 \n1002\n1003\nKeep them safe.\n",
                                 "Recovery codes: , \n1002\n1003\n"])
def test_the_stream_relates_lines_as_the_text_redactor_does(raw) -> None:
    """Fuzz finds (re-review 2): the text redactor reads a label line after its earlier masks (a masked OTP shortens
    the line into a label's reach), and a list's first code may follow a comma; the stream judged the raw line."""
    redactor = Redactor()
    expected = redactor.text(raw).encode()
    data = raw.encode()
    for split in range(len(data) + 1):
        stream = redactor.stream()
        assert stream.feed(data[:split]) + stream.feed(data[split:], final=True) == expected, split


@pytest.mark.parametrize("raw", ["Backup codes:\na1b2c-3d4e5 f6g7h-8i9j0\nk1l2m-3n4o5 p6q7r-8s9t0\n",
                                 'Password:\n"correct horse 22"\n',
                                 "Here are your backup codes:\r\nq2w3e4r5  t6y7u8i9\r\nz1x2c3v4  b5n6m7k8\r\n",
                                 "Recovery codes:\n7f3a9-2bc41 a8d02-91fe3\n",
                                 "Your verification code is:\n  482913  \n"])
def test_a_read_ending_inside_the_line_under_a_label_keeps_it_masked(raw) -> None:
    """Re-review 3 (NEW-C1): a released label, "Password:" or list line was kept as context only while the next line
    was still empty, so a read ending inside that line ("a1b2c-3d4e5 f") showed every later code."""
    redactor = Redactor()
    expected = redactor.text(raw).encode()
    data = raw.encode()
    for split in range(len(data) + 1):
        stream = redactor.stream()
        assert stream.feed(data[:split]) + stream.feed(data[split:], final=True) == expected, split


@pytest.mark.parametrize("raw", ["Two factor codes:\n4821-9921\n7731-1123\n", "Two-factor codes:\nCS101 MA102\n",
                                 "Your 2FA backup codes: a1b2c-3d4e5 f6g7h-8i9j0\n",
                                 "[Swiggy] Share 1000 : , Two-factor codes:\n q2w3e4r5  t6y7u8i9\r\n",
                                 "Security backup codes:\n7f3a9-2bc41\n"])
def test_a_list_label_split_anywhere_still_masks_its_codes(raw) -> None:
    """Re-review 3 (NEW-I1): "two factor" is a list label's first words; a read ending after "Two " or "Two factor "
    released the line before the label was whole."""
    redactor = Redactor()
    expected = redactor.text(raw).encode()
    assert expected != raw.encode()
    data = raw.encode()
    for split in range(len(data) + 1):
        stream = redactor.stream()
        assert stream.feed(data[:split]) + stream.feed(data[split:], final=True) == expected, split


@pytest.mark.parametrize("raw", ["1000 482913 482913. 1234 for is 10 Your to Order",
                                 "Card 4111 1111 1111 1111. Thanks\n",
                                 "4829 4821-9921 482913 2,000 Pin/Zip one-time 2002 my passcode: Instagram\r\n"])
def test_a_card_number_ending_in_a_dot_is_never_split(raw) -> None:
    """Fuzz find (re-review 3 probes; since a76b076): the numeric tail guard skipped digits followed by a dot, so a
    read ending at "…482913." released the card's first groups."""
    redactor = Redactor()
    expected = redactor.text(raw).encode()
    data = raw.encode()
    for split in range(len(data) + 1):
        stream = redactor.stream()
        assert stream.feed(data[:split]) + stream.feed(data[split:], final=True) == expected, split


@pytest.mark.parametrize("raw", ["  \r\n\r\nOrder 778812\r\n\r\n(OTP)\r\n", "Use this code to sign in\nvs. 4821\nas your PIN\n",
                                 "Swiggy codes: \n\n 482913. is Shah,\r\n",
                                 "Backup codes:\na1b2c-3d4e5 Keep them safe\n"])
def test_a_line_released_in_parts_is_masked_as_the_whole_line(raw) -> None:
    """Fuzz find (re-review 3 probes; since 3cbb440): after a read released "Order ", the rest of the line was masked
    as a line of its own ("778812" over "(OTP)"), masking what the text redactor keeps."""
    redactor = Redactor()
    expected = redactor.text(raw).encode()
    data = raw.encode()
    for split in range(len(data) + 1):
        stream = redactor.stream()
        assert stream.feed(data[:split]) + stream.feed(data[split:], final=True) == expected, split


def _differing_splits(redactor: Redactor, raw: str, *, pairs: bool = False) -> list[tuple[int, ...]]:
    """The read splits (every single one, and every pair when asked) where the stream differs from `Redactor.text`."""
    expected, data = redactor.text(raw).encode(), raw.encode()
    cutsets: list[tuple[int, ...]] = [(c,) for c in range(len(data) + 1)]
    if pairs:
        cutsets += [(a, b) for a in range(1, len(data)) for b in range(a + 1, len(data))]
    bad = []
    for cuts in cutsets:
        stream, out, prev = redactor.stream(), b"", 0
        for cut in (*cuts, len(data)):
            out += stream.feed(data[prev:cut])
            prev = cut
        if out + stream.feed(b"", final=True) != expected:
            bad.append(cuts)
    return bad


UNICODE_INDENTED = [
    "Your verification code is:\n\xa0482913\n",
    "Your verification code is:\n 482913\n",
    "Your verification code is:\n 482913\n",
    "Your verification code is:\n　482913\n",
    "Your verification code is:\n\x0b482913\n",
    "Your verification code is:\n\x0c482913\n",
    "Use this code to sign in\n\xa0 482913\n",
    "Sign-in code\r\n\xa0482913\r\n",
    "[Swiggy] 482913 is your OTP\n\n\xa0482-913\n",
    "Security backup codes:\r\n\xa02024\r\n",
    "Your code is:\n\xa0\n482913\n",
    "Your verification code is:\n　(G-482913)\n",
]


@pytest.mark.parametrize("raw", UNICODE_INDENTED)
def test_a_code_under_its_label_indented_by_unicode_whitespace_stays_masked(raw) -> None:
    """Re-review 4 (NEW-C): the text redactor strips any whitespace from a code alone on its line, but the stream kept
    a label line as context only while the next line held spaces or tabs, so a no-break space, an em space, an
    ideographic space, \\x0b or \\x0c before the code showed it at a split."""
    redactor = Redactor()
    assert redactor.text(raw) != raw
    assert _differing_splits(redactor, raw, pairs=True) == []


ADDRESS_MESSAGES = ["Bangalore PIN code: 560001", "PIN code 560001", "ZIP code 94107", "postal code 94103",
                    "Use coupon code 4455 at checkout", "Pin/Zip code: 560001", "PIN / ZIP code 560001"]
IN_LINE_SPACES = ["\xa0", " ", " ", " ", "　", "\x0b", "\x0c"]


@pytest.mark.parametrize("space", IN_LINE_SPACES)
def test_unicode_whitespace_inside_a_line_reads_as_a_space(space) -> None:
    """Re-review 4 (NEW-C, the text side): a page's "Code:&nbsp;482913" or "Use 482913 as your PIN" with no-break
    spaces showed the code, since the class patterns took only spaces and tabs between their words. Any whitespace but
    a line break now reads as a space: each corpus line masks the same with its spaces swapped, and ordinary lines,
    address codes and the owner's requests stay."""
    r = Redactor()
    corpus = ([m for m, _ in PROVIDER_CORPUS + SECRET_WORD_CASES + OWNER_REQUESTS] + NEGATIVES + ADDRESS_MESSAGES
              + ["Card 4111 1111 1111 1111", "Backup codes: 1234 5678, 9012 3456", "Two factor codes: a1b2c-3d4e5",
                 "my wifi password is sunshine", "Your one-time password is 482913", "api key 482-913"])
    for message in corpus:
        assert r.text(message.replace(" ", space)) == r.text(message).replace(" ", space), message


@pytest.mark.parametrize("raw", ["Code:\xa0482913\n", "Use\xa0482913\xa0as\xa0your\xa0PIN\n",
                                 "Card\xa04111\xa01111\xa01111\xa01111\n", "password: hunter2!\n",
                                 "my\xa0wifi\xa0password\xa0is\xa0sunshine\n", "Backup codes:\xa01234\xa05678\n",
                                 "Two\xa0factor\xa0codes:\n4821-9921\n", "api\xa0key\xa0482-913\n",
                                 "482913　is your code\n", "one\xa0time\xa0password\xa0482913\n"])
def test_a_line_with_unicode_spaces_streams_as_the_text_redactor_masks_it(raw) -> None:
    redactor = Redactor()
    assert REDACTED in redactor.text(raw)
    assert _differing_splits(redactor, raw) == []


def test_in_line_whitespace_is_every_whitespace_but_a_line_break() -> None:
    from ferb.sanitize import redact
    every = "".join(map(chr, range(0x110000)))
    assert set(re.findall(rf"[{redact._H}]", every)) == {c for c in every if c.isspace()} - {"\r", "\n"}


# Re-review 4 (T1, rule 14): layouts the text redactor and the stream both missed.
T1_LAYOUTS = [
    # a blank line, whitespace-only or no-break-space-only too, between a list's label and its codes (or between codes)
    ("Backup codes:\n\na1b2c-3d4e5 f6g7h-8i9j0\n", ["a1b2c-3d4e5", "f6g7h-8i9j0"]),
    ("Recovery codes:\n \n4821-9921\n7731-1123\n", ["4821-9921", "7731-1123"]),
    ("Your 2FA backup codes:\r\n\xa0\r\nq2w3e4r5  t6y7u8i9\r\n", ["q2w3e4r5", "t6y7u8i9"]),
    ("Two-factor codes:\n\n\n7f3a9-2bc41\n\n8i9j0-k1l2m\n", ["7f3a9-2bc41", "8i9j0-k1l2m"]),
    ("Backup codes:\n\xa0a1b2c-3d4e5\n　f6g7h-8i9j0\n", ["a1b2c-3d4e5", "f6g7h-8i9j0"]),
    # "Password:", a blank line, then its value
    ("Password:\n\nhunter2!\n", ["hunter2"]),
    ('Password:\r\n\xa0\r\n"correct horse 22"\r\n', ["correct horse 22"]),
    ("token =\n\n  abc123def\n", ["abc123def"]),
    # numbered or bulleted items under a backup, recovery or 2FA list's label
    ("Backup codes:\n1. a1b2c-3d4e5\n2. f6g7h-8i9j0\n", ["a1b2c-3d4e5", "f6g7h-8i9j0"]),
    ("Recovery codes:\n1) 4821 9921\n2) 7731 1123\n10) 5522 8833\n", ["4821", "9921", "7731", "1123", "5522", "8833"]),
    ("Your backup codes:\n• q2w3e4r5\n• t6y7u8i9\n", ["q2w3e4r5", "t6y7u8i9"]),
    ("2FA recovery codes:\n\n  1. a1b2c-3d4e5\n\n  2. f6g7h-8i9j0\n", ["a1b2c-3d4e5", "f6g7h-8i9j0"]),
    ("Security backup codes:\n- 7f3a9-2bc41\n* a8d02-91fe3\n", ["7f3a9-2bc41", "a8d02-91fe3"]),
    # an unquoted value with spaces on the line after a password label: the whole value line
    ("password for gmail:\n  s3cret pass 9\n", ["s3cret", "pass 9"]),
    ("Password:\ncorrect horse battery staple\n", ["correct", "horse", "battery", "staple"]),
    ("Your temporary password:\r\n\r\nTr0ub4dor &3\r\n", ["Tr0ub4dor", "&3"]),
]


@pytest.mark.parametrize("raw,secrets", T1_LAYOUTS)
def test_list_and_password_layouts_with_blank_lines_markers_and_spaces_are_masked(raw, secrets) -> None:
    """Re-review 4 (T1): a blank line under "Backup codes:" or "Password:", numbered list items, and a next-line
    password with spaces showed the secret in text and in the stream; both now mask it at every split."""
    redactor = Redactor()
    out = redactor.text(raw)
    for secret in secrets:
        assert secret not in out, out
    assert _differing_splits(redactor, raw) == []


@pytest.mark.parametrize("raw,expected", [
    ("password for gmail:\n  s3cret pass 9\n", "password for gmail:\n  [redacted]\n"),
    ("Password:\nhunter2!\nUsername: kartikey\n", "Password:\n[redacted]\nUsername: kartikey\n"),
    ("Backup codes:\n1. a1b2c-3d4e5\n2. f6g7h-8i9j0\n\nKeep them safe.\n",
     "Backup codes:\n1. [redacted]\n2. [redacted]\n\nKeep them safe.\n"),
    ("HTTP status codes:\n\n200, 301\n", "HTTP status codes:\n\n200, 301\n"),
    ("Backup codes:\n1. Open Settings\n2. Tap Security\n", "Backup codes:\n1. Open Settings\n2. Tap Security\n"),
    ("1000\n\n1001\n\n1002\n", "1000\n\n1001\n\n1002\n"),
])
def test_the_layouts_mask_only_the_value_lines(raw, expected) -> None:
    redactor = Redactor()
    assert redactor.text(raw) == expected
    assert _differing_splits(redactor, raw) == []


@pytest.mark.parametrize("label", ["", "Backup codes:\n", "Your verification code is:\n"])
def test_a_read_relates_its_new_lines_once_and_unmasked_lines_never(label, monkeypatch) -> None:
    """Re-review 4 (P1): each read worked out the relations of every line it held two or three times, each after the
    masks before the labelled classes (`Redactor.before_labels`), so number-only output cost 8-15 times a76b076. A
    line's relations are now worked out once per read, and the masks only for a line holding a class word's stem."""
    seen: list[str] = []
    real = Redactor.before_labels
    monkeypatch.setattr(Redactor, "before_labels", lambda self, s: seen.append(s) or real(self, s))
    redactor = Redactor()
    text = label + "".join(f"{n}\n" for n in range(1000, 30000))
    data, stream, out, reads = text.encode(), redactor.stream(), b"", 0
    for i in range(0, len(data), 1024):
        out += stream.feed(data[i:i + 1024])
        reads += 1
    out += stream.feed(b"", final=True)
    assert out.decode() == redactor.text(text)
    assert len(seen) <= reads + 1, len(seen)
    assert all(label.strip() in line for line in seen)
