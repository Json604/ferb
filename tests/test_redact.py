"""Redaction pipeline (section 10.6)."""

import copy

import pytest

from ferb.cli.admin import render_trace
from ferb.sanitize.redact import EXCLUDED, REDACTED, Redactor, secret_key

r = Redactor()


@pytest.mark.parametrize('secret', [
    'known-secret-value', 'known\nmultiline\nsecret', 'Bearer abcdefghijklmnopqrstu',
    '-----BEGIN RSA PRIVATE KEY-----\nMIIabc\n-----END RSA PRIVATE KEY-----',
    'sk-ant-api03-abcdefghijklmnopqrst', 'https://user:password@example.com/?token=abc',
])
def test_stream_redaction_is_safe_at_every_byte_boundary(secret):
    redactor = Redactor()
    if secret.startswith('known'):
        redactor.add_secret_value(secret)
    raw = ('before π\n' + secret + '\nafter ✓\n').encode()
    expected = redactor.text(raw.decode()).encode()
    for split in range(len(raw) + 1):
        stream = redactor.stream()
        actual = stream.feed(raw[:split]) + stream.feed(raw[split:], final=True)
        assert actual == expected, split
    stream = redactor.stream()
    assert b''.join(stream.feed(bytes([x])) for x in raw) + stream.feed(b'', final=True) == expected


def test_stream_emits_complete_normal_lines_without_waiting_for_exit():
    stream = Redactor().stream()
    assert stream.feed('progress π\n'.encode()) == 'progress π\n'.encode()
    assert stream.feed(b'partial') == b''
    assert stream.feed(b'', final=True) == b'partial'



def test_stream_emits_an_interactive_prompt_before_process_exit():
    stream = Redactor().stream()
    assert stream.feed(b'Continue? ') == b'Continue? '
    assert stream.feed(b'next safe word ') == b'next safe word '
    assert stream.feed(b'', final=True) == b''


def test_stream_keeps_partial_private_key_headers_after_safe_prompt():
    raw = b'Continue? -----BEGIN RSA PRIVATE KEY-----\nMIIabc\n-----END RSA PRIVATE KEY-----\n'
    expected = b'Continue? [redacted]\n'
    for split in range(len(raw) + 1):
        stream = Redactor().stream()
        assert stream.feed(raw[:split]) + stream.feed(raw[split:], final=True) == expected, split
    stream = Redactor().stream()
    assert b''.join(stream.feed(bytes([c])) for c in raw) + stream.feed(b'', final=True) == expected

def test_overlapping_secrets_are_redacted_longest_first():
    redactor = Redactor()
    redactor.add_secret_value('abcd')
    redactor.add_secret_value('abcdef')
    assert redactor.text('abcdef') == REDACTED


def test_stream_suppresses_oversize_unfinished_private_key_without_raw_spill():
    stream = Redactor().stream()
    out = stream.feed(b'-----BEGIN RSA PRIVATE KEY-----\n' + b'A' * 200000)
    out += stream.feed(b'\n-----END RSA PRIVATE KEY-----\nnormal\n', final=True)
    assert b'AAAA' not in out and b'[redacted]' in out and out.endswith(b'normal\n')


def test_secret_key_names_by_word_not_substring() -> None:
    for k in ("password", "api_key", "apiKey", "Authorization", "x-api-key", "session", "refreshToken",
              "one_time_code", "otp", "set-cookie", "client_secret"):
        assert secret_key(k), k
    for k in ("input_tokens", "output_tokens", "author", "tokenizer_name", "description", "kind"):
        assert not secret_key(k), k


def test_internal_id_fields_are_exempt_but_still_pattern_scanned() -> None:
    out = r({"task_id": "t-1", "span_id": "s-2", "session_id": "sess-3", "run_token_id": "rt-4",
             "token": "abc123", "session_token": "xyz789"})
    assert out["task_id"] == "t-1" and out["span_id"] == "s-2"
    assert out["session_id"] == "sess-3" and out["run_token_id"] == "rt-4"
    assert out["token"] == REDACTED and out["session_token"] == REDACTED
    assert r({"task_id": "sk-ant-api03-AAAAAAAAAAAAAAAAAAAA"})["task_id"] == REDACTED   # values still scanned


def test_secret_formats_in_free_text() -> None:
    samples = ["sk-ant-api03-abcdefghijklmnopqrst", "sk-proj-abcdefghijklmnopqrstuvwx",
               "ghp_abcdefghijklmnopqrstuvwxyz0123", "xoxb-1234567890-abcdef", "AKIAABCDEFGHIJKLMNOP",
               "eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjM0NTYifQ.abcdefghijklmnop", "Bearer abcdefghijklmnopqrstu",
               "-----BEGIN RSA PRIVATE KEY-----\nMIIabc\n-----END RSA PRIVATE KEY-----"]
    for s in samples:
        out = r.text(f"before {s} after")
        assert s not in out and out.startswith("before ") and out.endswith(" after"), s


def test_urls_keep_shape_but_lose_credentials() -> None:
    out = r.text("go to https://u:hunter2@example.com/cb?code=abc&state=ok#access_token=zzz&x=1 now")
    assert "hunter2" not in out and "abc" not in out and "zzz" not in out
    assert "https://u:[redacted]@example.com/cb?code=[redacted]&state=ok#access_token=[redacted]&x=1" in out


def test_password_and_otp_fields_in_snapshots() -> None:
    snap = {"elements": [{"id": 1, "type": "password", "value": "hunter2"},
                         {"id": 2, "autocomplete": "one-time-code", "value": "123456"},
                         {"id": 3, "type": "text", "value": "hello"}]}
    els = r(snap)["elements"]
    assert [e["value"] for e in els] == [REDACTED, REDACTED, "hello"]


def test_debugger_input_events_and_known_secret_values() -> None:
    red = Redactor()
    red.add_secret_value("correct-horse-battery")
    ev = {"method": "Input.insertText", "params": {"text": "my typed words"}}
    assert red(ev)["params"]["text"] == REDACTED
    assert red.text("page says: correct-horse-battery is wrong") == f"page says: {REDACTED} is wrong"


def test_exclusion_list_domains_and_apps() -> None:
    red = Redactor(exclude_domains={"bank.example"}, exclude_apps={"1Password"})
    assert red({"url": "https://online.bank.example/accounts"})["url"] == EXCLUDED
    assert red({"url": "https://notbank.example/x"})["url"] == "https://notbank.example/x"
    assert red({"app": "1password"})["app"] == EXCLUDED


def test_input_is_never_mutated() -> None:
    original = {"password": "p", "nested": [{"token": "t"}], "text": "sk-ant-api03-abcdefghijklmnopqrst"}
    before = copy.deepcopy(original)
    r(original)
    assert original == before


def test_redacted_trace_stays_readable() -> None:
    """Owner requirement: after redaction a trace still shows structure, ids, kinds and non-secret arguments."""
    trace = {"task_id": "t-42", "spans": [{
        "id": "s-1", "parent_id": None, "task_id": "t-42", "kind": "task", "device_id": "mac-1",
        "started_at": "2026-09-28T10:00:00+05:30", "ended_at": "2026-09-28T10:00:02+05:30",
        "input": {"goal": "log in and check inbox"}, "output": {"status": "done"}, "cost_usd": 0.01, "error": None,
        "children": [{
            "id": "s-2", "parent_id": "s-1", "task_id": "t-42", "kind": "tool.call", "device_id": "mac-1",
            "started_at": "2026-09-28T10:00:00+05:30", "ended_at": "2026-09-28T10:00:01+05:30",
            "input": {"tool": "browser.fill", "field": "email", "session_id": "sess-7", "password": "hunter2",
                      "url": "https://mail.example/login?token=abc"},
            "output": None, "cost_usd": None, "error": "login failed for Bearer abcdefghijklmnopqrstu",
            "children": []}]}]}
    red = r(trace)
    text = "\n".join(render_trace(red))
    assert "hunter2" not in text and "abcdefghijklmnopqrstu" not in text and "token=abc" not in text
    assert "task t-42" in text and "task  log in and check inbox" in text and "browser.fill" in text  # structure
    assert "✗ login failed for [redacted]" in text                                         # error still useful
    assert red["spans"][0]["children"][0]["input"]["session_id"] == "sess-7"             # internal id kept
    assert red["spans"][0]["children"][0]["input"]["tool"] == "browser.fill"
    assert red["spans"][0]["input"]["goal"] == "log in and check inbox"
