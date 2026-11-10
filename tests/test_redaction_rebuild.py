"""Step 6a: the rebuilt redactor (ferb/sanitize). Every vendor format in one table with a fake value; idempotence and
JSON shape proven by seeded property tests over the redaction corpus; dict keys and non-text values covered; gates
ask for a specific hit; a value redacted once is never redacted again downstream.

Every value here is made up, built by concatenation so no literal in this file looks like a live credential."""
import json
import random

import pytest

from ferb.memory.learning import redacted_json
from ferb.sanitize.formats import first_format
from ferb.sanitize.redact import EXCLUDED, REDACTED, Redactor
from tests.support.redaction_corpus import ORDINARY, _PAGES, ordinary_card


def fake(n: int, chars: str = "Ab3dEf6hIj9kLm2nOp5qRs8tUv1wXy4zQ7") -> str:
    return (chars * (n // len(chars) + 1))[:n]


HEX32 = "9f8e7d6c5b4a39281706f5e4d3c2b1a0"
B64_40 = fake(36) + "/+Zx"
JWT = "eyJ" + "hbGciOiJIUzI1NiJ9" + "." + "eyJ" + "zdWIiOiIxMjM0NTYifQ" + "." + fake(30)
PEM_BODY = "MIIE" + fake(60)

# (format, text with a fake secret, the secret part that must be gone, a part that must stay)
VENDOR_TABLE = [
    ("AWS access key id", "key AKIA" + "Q2W3E4R5T6Y7U8I9" + " in", "Q2W3E4R5T6Y7U8I9", "key "),
    ("AWS secret access key", "aws_secret_access_key = " + B64_40, B64_40, "aws_secret_access_key = "),
    ("AWS secret access key", "export AWS_SECRET_ACCESS_KEY=" + B64_40, B64_40, "AWS_SECRET_ACCESS_KEY="),
    ("Stripe key", "sk_" + "live_" + fake(24), fake(24), ""),
    ("Stripe key", "sk_" + "test_" + fake(24), fake(24), ""),
    ("Stripe key", "rk_" + "live_" + fake(24), fake(24), ""),
    ("Stripe key", "rk_" + "test_" + fake(24), fake(24), ""),
    ("GitHub token", "ghp_" + fake(36), fake(36), ""),
    ("GitHub token", "gho_" + fake(36), fake(36), ""),
    ("GitHub token", "ghu_" + fake(36), fake(36), ""),
    ("GitHub token", "ghs_" + fake(36), fake(36), ""),
    ("GitHub token", "ghr_" + fake(36), fake(36), ""),
    ("GitHub fine-grained token", "github_pat_" + fake(22) + "_" + fake(30), fake(22), ""),
    ("GitLab token", "glpat-" + fake(20), fake(20), ""),
    *[("Slack token", f"xox{c}-1234567890-" + fake(24), fake(24), "") for c in "abprs"],
    ("OpenAI key", "sk-" + fake(48), fake(48), ""),
    ("OpenAI key", "sk-proj-" + fake(48), fake(48), ""),
    ("Anthropic key", "sk-ant-api03-" + fake(40), fake(40), ""),
    ("Google API key", "AIza" + fake(35), fake(35), ""),
    ("npm token", "npm_" + fake(36), fake(36), ""),
    ("Hugging Face token", "hf_" + fake(34), fake(34), ""),
    ("SendGrid key", "SG." + fake(22) + "." + fake(43), fake(22), ""),
    ("Twilio key", "sid SK" + HEX32, HEX32, "sid "),
    ("Twilio key", "curl -u AC" + HEX32 + ":" + HEX32[::-1], HEX32[::-1], "curl -u "),
    ("Discord bot token", "token M" + fake(23) + "." + fake(6) + "." + fake(27), fake(27), ""),
    ("JSON web token", "jwt " + JWT + " end", JWT, " end"),
    ("private key", "-----BEGIN RSA PRIVATE KEY-----\n" + PEM_BODY + "\n-----END RSA PRIVATE KEY-----\nok",
     PEM_BODY, "\nok"),
    ("private key", "pasted:\n-----BEGIN PRIVATE KEY-----\n" + PEM_BODY + "\n" + PEM_BODY[::-1], PEM_BODY[::-1],
     "pasted:"),
    ("private key", "-----BEGIN OPENSSH PRIVATE KEY-----\n" + PEM_BODY + "\n", PEM_BODY, ""),
    ("private key", "-----BEGIN PGP PRIVATE KEY BLOCK-----\nVersion: x\n\n" + PEM_BODY
     + "\n-----END PGP PRIVATE KEY BLOCK-----", PEM_BODY, ""),
    ("private key", '{"k": "-----BEGIN EC PRIVATE KEY-----\\n' + PEM_BODY + '\\n"}', PEM_BODY, '{"k": "'),
    ("Authorization header", "Authorization: Basic " + "dXNlcjpw" + fake(12) + "==", "dXNlcjpw",
     "Authorization: Basic "),
    ("bearer token", "Authorization: Bearer " + fake(30), fake(30), "Authorization: Bearer "),
    ("bearer token", "curl -H 'Bearer " + fake(30) + "'", fake(30), "Bearer "),
    ("cookie", "Cookie: sid=" + fake(20) + "; theme=dark", "theme=dark", "Cookie: "),
    ("cookie", "Set-Cookie: session=" + fake(20) + "; Path=/; HttpOnly", fake(20), "Set-Cookie: "),
    ("URL password", "https://deploy:" + fake(14) + "@git.example.com/repo.git", fake(14), "https://deploy:"),
    ("URL token", "https://" + fake(24) + "@git.example.com/repo.git", fake(24), "@git.example.com"),
    ("URL credential", "https://b.s3.amazonaws.com/x?X-Amz-Credential=AK%2F" + fake(10) + "&X-Amz-Signature="
     + HEX32 + "&v=1", HEX32, "&v=1"),
    ("URL credential", "https://acct.blob.core.windows.net/c/f?sv=2022&sig=" + fake(30), fake(30), "sv=2022"),
    ("database password", "mysql -u root -p" + fake(12) + " app", fake(12), "mysql -u root "),
    ("database password", "mysqldump --password=" + fake(12) + " db", fake(12), "--password="),
    ("Azure account key", "AccountName=a;AccountKey=" + B64_40 + ";EndpointSuffix=core", B64_40, "AccountKey="),
    ("Sentry DSN", "dsn https://" + HEX32 + "@o12345.ingest.sentry.io/678", HEX32, "o12345.ingest.sentry.io"),
    ("payment card", "Card 4111 1111 1111 1111", "4111", "Card "),
    ("payment card", "qty 2 4111 1111 1111 1111", "4111", "qty 2 "),
    ("payment card", "4111-1111-1111-1111", "4111", ""),
    ("CVV", "CVV: 123 on the back", "123", "CVV: "),
    ("CVV", "cvc 4321", "4321", "cvc "),
    ("card expiry", "card 4111 1111 1111 1111 12/27 737", "12/27", "card "),
    ("card expiry", "Expiry date: 09/2028", "09/2028", "Expiry date: "),
    ("card expiry", "exp 11/29", "11/29", "exp "),
]


# decision: GATE-212
@pytest.mark.parametrize("kind,text,secret,kept", VENDOR_TABLE, ids=[f"{k}:{i}" for i, (k, *_) in
                                                                      enumerate(VENDOR_TABLE)])
def test_every_vendor_format_is_masked_and_named(kind, text, secret, kept) -> None:
    r = Redactor()
    out = r.text(text)
    assert secret not in out and REDACTED in out and kept in out, out
    assert first_format(text) is not None and r.hit(text) is not None
    assert r.text(out) == out


# decision: GATE-206, GATE-212
def test_the_cvv_after_a_card_and_its_expiry_is_masked() -> None:
    assert Redactor().text("4111 1111 1111 1111 12/27 737 thanks") == "[redacted] [redacted] [redacted] thanks"


# decision: GATE-212
@pytest.mark.parametrize("text", ["the basic plan costs 499", "Order #405-1234567 shipped", "valid until Monday",
                                  "exp(2) = 7.389", "a public key -----BEGIN PUBLIC KEY-----", "mkdir -p logs",
                                  "ssh git@github.com", "https://git@github.com/org/repo", "sk-short", "AIza-short",
                                  "4111 1111 1111 1112", "port 54321", "ghp_short"])
def test_ordinary_text_shaped_near_a_format_stays(text) -> None:
    r = Redactor()
    assert r.text(text) == text and r.hit(text) is None


# ---------- one label vocabulary ----------

@pytest.mark.parametrize("key,value,hidden", [
    ("pin", "4821", True), ("password", "hunter2", True), ("session_token", "abc", True), ("DB_PASSWORD", "x1", True),
    ("client_secret", "abc", True), ("apiKey", "abc", True), ("x-api-key", "abc", True), ("cvv", "123", True),
    ("code", "482913", True), ("code", "200", False), ("verification_code", "4821", True),
    ("cache_key", "Zx9" + fake(20), True), ("cache_key", "user-profile", False), ("sort_key", "name", False),
    ("signing_key", "rotate-weekly", True),
    ("token_limit", "4096", False), ("max_tokens", "4096", False), ("pin_code", "560001", False),
    ("zip_code", "94107", False), ("pin_message", "hello", False), ("password_file", "~/pw.txt", False),
])
def test_dict_keys_and_key_value_text_share_one_vocabulary(key, value, hidden) -> None:
    """{"pin": …} and "pin": … behave the same, as do KEY=value and `key: value` for identifier-shaped keys."""
    r = Redactor()
    as_dict = r({key: value})[key] == REDACTED
    as_json = value not in r.text(json.dumps({key: value}))
    as_assignment = value not in r.text(f"{key}={value}")
    assert (as_dict, as_json, as_assignment) == (hidden, hidden, hidden)


def test_a_url_parameter_uses_the_vocabulary() -> None:
    assert Redactor().text("https://x.example/p?pin=4821&page=2") == "https://x.example/p?pin=[redacted]&page=2"


def test_a_field_named_by_the_vocabulary_hides_its_value() -> None:
    r = Redactor()
    assert r.field("abc-def", role="AXTextField", label="client_secret") == REDACTED
    assert r({"tag": "input", "type": "text", "autocomplete": "cc-number", "value": "4111111111111111"})["value"] \
        == REDACTED
    assert r({"tag": "input", "type": "tel", "autocomplete": "cc-csc", "value": "123"})["value"] == REDACTED


# ---------- structures: keys, numbers, bytes ----------

def test_a_secret_keys_structure_keeps_its_shape_with_every_value_masked() -> None:
    assert Redactor()({"credentials": {"user": "a", "password": "b", "otp": [1, None], "ok": True}}) == \
        {"credentials": {"user": REDACTED, "password": REDACTED, "otp": [REDACTED, None], "ok": True}}


def test_dict_keys_are_redacted_and_two_that_redact_alike_both_stay() -> None:
    r = Redactor()
    a, b = "sk-ant-api03-" + fake(30), "sk-ant-api03-" + fake(31)
    out = r({a: 1, b: 2, "plain": 3})
    assert a not in json.dumps(out) and b not in json.dumps(out)
    assert sorted(out.values()) == [1, 2, 3] and len(out) == 3


def test_numbers_and_bytes_are_covered() -> None:
    r = Redactor()
    r.add_secret_value("839201")
    assert r({"n": 4111111111111111, "otp": 839201, "count": 12, "big": 14839201, "f": 1.5, "b": True}) == \
        {"n": REDACTED, "otp": REDACTED, "count": 12, "big": 14839201, "f": 1.5, "b": True}
    assert r([839201, 7]) == [REDACTED, 7]
    raw = b"token sk-ant-api03-" + fake(30).encode() + b" \xff end"
    out = r(raw)
    assert isinstance(out, bytes) and b"sk-ant" not in out and out.endswith(b" \xff end")
    assert r(b"plain bytes") == b"plain bytes"


def test_a_known_value_is_found_in_its_escaped_forms() -> None:
    r = Redactor()
    r.add_secret_value('p@ss "w0rd"&')
    for text in ('{"v": "p@ss \\"w0rd\\"&"}', "?q=p%40ss%20%22w0rd%22%26", "p@ss &quot;w0rd&quot;&amp;"):
        assert "w0rd" not in r.text(text), text


# ---------- idempotence and JSON shape, by seeded property tests ----------

def _corpus() -> list[str]:
    from tests import test_labelled_secret_classes as c
    pieces = [m for m, _ in c.PROVIDER_CORPUS + c.SECRET_WORD_CASES + c.OWNER_REQUESTS] + c.NEGATIVES + \
        c.ADDRESS_MESSAGES + c.STREAMED + c.CRLF + [m for m, _ in c.T1_LAYOUTS] + [m for m, _ in c.MARKUP_CODES] + \
        c.SPACED_TAGS + ORDINARY + _PAGES + [t for _, t, _, _ in VENDOR_TABLE]
    return pieces + ["Password: '", "typed key '", "PIN", "4821", "[redacted]", "my", "is", ":", "=", '"', "'",
                     "code", "\n", "\r\n", "Backup codes:", "1234 5678", "https://u:p@h.example/?code=1", "2024",
                     "PIN code", "560001", "-----BEGIN RSA PRIVATE KEY-----", "MIIabc", "eyJ", "Bearer", "x" * 20]


SEPARATORS = [" ", "\n", ", ", ": ", "=", '"', "'", "", " is ", "\n\n", "\t", "; ", " - ", "(", ")"]


def test_redacting_twice_changes_nothing() -> None:
    rng = random.Random(6)
    corpus = _corpus()
    r = Redactor()
    r.add_secret_value("839201")
    r.add_secret_value("Zq9-hunter-77")
    bad = []
    for _ in range(4000):
        parts = [rng.choice(corpus) for _ in range(rng.randint(1, 6))]
        text = "".join(p + rng.choice(SEPARATORS) for p in parts)
        once = r.text(text)
        if r.text(str(once)) != once:
            bad.append(text)
    assert bad == [], bad[:5]


def _random_value(rng: random.Random, corpus: list[str], depth: int = 0) -> object:
    pick = rng.random()
    if depth < 3 and pick < 0.25:
        return {rng.choice(corpus + ["pin", "password", "code", "token", "a", "b", "id", "task_id"]):
                _random_value(rng, corpus, depth + 1) for _ in range(rng.randint(1, 4))}
    if depth < 3 and pick < 0.4:
        return [_random_value(rng, corpus, depth + 1) for _ in range(rng.randint(1, 4))]
    if pick < 0.5:
        return rng.choice([4111111111111111, 4821, 839201, 12, 0, True, None, 1.5])
    return rng.choice(corpus)


def _keys(value: object) -> object:
    if isinstance(value, dict):
        return [len(value), [_keys(v) for v in value.values()]]
    if isinstance(value, list):
        return [_keys(v) for v in value]
    return None


def test_structures_stay_valid_json_with_every_key_and_redact_once() -> None:
    rng = random.Random(9)
    corpus = _corpus()
    r = Redactor()
    r.add_secret_value("839201")
    for _ in range(1500):
        value = _random_value(rng, corpus)
        once = r(value)
        assert json.loads(json.dumps(once)) == once                 # valid JSON
        assert _keys(once) == _keys(value)                          # every key and item kept
        assert r(json.loads(json.dumps(once))) == once              # redacting again changes nothing


def test_redacted_json_keeps_every_key() -> None:
    r = Redactor()
    assert redacted_json(r.text, {"a": "Password: '", "b": "typed key '"}) == {"a": "Password: '", "b": "typed key '"}
    card = ordinary_card()
    assert redacted_json(r.text, card) == card


# ---------- gates ask for a specific hit ----------

@pytest.mark.parametrize("text,hit", [
    ("echo 'PIN 1234'", None), ("password: hunter2", None), ("Your code is 482913", None),
    ("cat ~/.ssh/config", None), ("export KEY=sk-ant-api03-" + fake(30), "Anthropic key"),
    ("pay with 4111 1111 1111 1111", "payment card"), ("Zq9-hunter-77 typed", "a saved secret"),
])
def test_hit_names_a_known_value_or_a_format_only(text, hit) -> None:
    r = Redactor()
    r.add_secret_value("Zq9-hunter-77")
    assert r.hit(text) == hit


# ---------- one pass per value ----------

def test_a_redacted_value_is_not_redacted_again_until_a_known_value_changes(monkeypatch) -> None:
    r = Redactor()
    passes: list[str] = []
    real = Redactor.spans
    monkeypatch.setattr(Redactor, "spans", lambda self, s, **k: passes.append(s) or real(self, s, **k))
    once = r({"page": "Your code is 482913", "items": ["a", "b"], "n": 3})
    count = len(passes)
    assert r(once) == once and r.text(once["page"]) is once["page"] and len(passes) == count
    r.add_secret_value("Zq9-hunter-77")                     # a new known value: earlier marks are not trusted
    assert r(once) == once and len(passes) > count
    edited = once["page"] + " and Zq9-hunter-77"            # any edit drops the mark
    assert r.text(edited).endswith("and [redacted]")


def test_the_marker_survives_json_and_sqlite_as_plain_text(tmp_path) -> None:
    import pickle
    import sqlite3
    r = Redactor()
    value = r.text("hello")
    db = sqlite3.connect(tmp_path / "x.db")
    db.execute("create table t(x text)")
    db.execute("insert into t values (?)", (value,))
    assert db.execute("select x from t").fetchone()[0] == "hello"
    assert json.dumps({value: value}) == '{"hello": "hello"}' and type(pickle.loads(pickle.dumps(value))) is str


def test_excluded_hosts_and_apps_still_read_excluded() -> None:
    r = Redactor(exclude_domains={"bank.example"}, exclude_apps={"1Password"})
    assert r.text("see https://online.bank.example/a?code=1 now") == f"see {EXCLUDED} now"
    assert r({"app": "1password"})["app"] == EXCLUDED
    assert r.text(r.text("see https://online.bank.example/a now")) == f"see {EXCLUDED} now"


# ---------- the registry preloaded at start; the process exception hooks ----------

# decision: GATE-573
def test_preloaded_keys_are_known_from_the_start() -> None:
    r = Redactor()
    key = "Fake-Provider-Key-0001"                          # no vendor format: found by its value alone
    assert r.preload([key, key]) == 1
    assert r.text(f"echo {key}x") == "echo [redacted]x" and r.hit(f"x {key}") is not None


# decision: GATE-572, GATE-573
def test_the_daemon_preloads_only_the_api_keys_in_use(daemon, monkeypatch) -> None:
    """Fake stores only: the daemon reads the API keys of the runtimes in use in a thread, registers them and journals
    counts, never values. No saved secret is read: neither FERB's [secrets] nor the owner's (owner, 2026-11-10)."""
    import ferb.runtimes.keys as keys
    d = daemon.daemon
    asked: list[set[str]] = []
    key = "sk-ant-api03-" + fake(30)
    monkeypatch.setattr(keys, "stored_api_keys", lambda providers, problem=None: asked.append(set(providers)) or
                        [key])
    daemon.run(d._preload_secrets())
    assert d.redact.text(f"saw {key}") == "saw [redacted]"
    assert d.redact.secret_values() == frozenset({key})
    [entry] = daemon.run(_entries(d))
    assert entry == {"values": 1, "unreadable": 0}
    assert asked == [{d._runtime_for(role) for role in ("orchestrator", "worker", "checker")}]   # only those used


# decision: GATE-573
def test_api_keys_are_read_only_for_providers_the_runtime_table_uses(monkeypatch) -> None:
    import subprocess
    import ferb.runtimes.keys as keys
    ran: list[list[str]] = []
    monkeypatch.setattr(subprocess, "run", lambda cmd, **k: ran.append(cmd) or subprocess.CompletedProcess(cmd, 44))
    assert keys.stored_api_keys({"codex", "claude"}) == [] and ran == []        # no API runtime: nothing read


async def _entries(d):
    return [e.data for e in d._journal().of_kind("secret.registry_preloaded")]


def test_unhandled_exceptions_reach_stderr_redacted(monkeypatch) -> None:
    import io
    import sys
    import threading
    from ferb.daemon.main import redacting_hooks
    monkeypatch.setattr(sys, "excepthook", sys.excepthook)
    monkeypatch.setattr(threading, "excepthook", threading.excepthook)
    monkeypatch.setattr(sys, "unraisablehook", sys.unraisablehook)
    r = Redactor()
    r.add_secret_value("Fake-Typed-Value-9")
    err = io.StringIO()
    redacting_hooks(r.text, err)
    try:
        raise RuntimeError("login failed for Fake-Typed-Value-9 with token ghp_" + fake(36))
    except RuntimeError as e:
        sys.excepthook(type(e), e, e.__traceback__)
    worker = threading.Thread(target=lambda: (_ for _ in ()).throw(ValueError("card 4111 1111 1111 1111")))
    worker.start()
    worker.join()
    out = err.getvalue()
    assert "RuntimeError" in out and "ValueError" in out and "in thread" in out
    assert "Fake-Typed-Value-9" not in out and "ghp_" + fake(36) not in out and "4111" not in out


def test_an_event_built_from_redacted_values_is_not_redacted_again(monkeypatch) -> None:
    """One pass per value: a tool result redacted at capture reaches the events object (which redacts every event once)
    without its text being read again."""
    from ferb.daemon.events import Events
    r = Redactor()
    data = r({"summary": "Your code is 482913", "args": {"text": "password: hunter2"}, "n": 3})    # at capture
    passes: list[str] = []
    real = Redactor.spans
    monkeypatch.setattr(Redactor, "spans", lambda self, s, **k: passes.append(s) or real(self, s, **k))
    got: list[dict] = []
    events = Events(lambda: r)
    events.on(None, lambda topic, d: got.append(d))
    events.publish("tool.finished", data)
    assert passes == [] and got == [data] and "482913" not in json.dumps(got) and "hunter2" not in json.dumps(got)


def test_the_stream_masks_exactly_as_the_text_redactor_on_random_corpus_text() -> None:
    """The rebuilt stream (line-local rules, held lines, a few lines of context) releases text masked exactly as
    `Redactor.text` masks the whole, whatever the read sizes: random corpus combinations, keys and codes included."""
    rng = random.Random(21)
    corpus = _corpus()
    r = Redactor()
    r.add_secret_value("Zq9-hunter-77")
    bad = []
    for _ in range(1500):
        text = "".join(rng.choice(corpus) + rng.choice(SEPARATORS + ["\n"]) for _ in range(rng.randint(1, 6)))
        data, stream, out, pos = text.encode(), r.stream(), b"", 0
        while pos < len(data):
            n = rng.randint(1, 12)
            out += stream.feed(data[pos:pos + n])
            pos += n
        if (out + stream.feed(b"", final=True)).decode() != r.text(text):
            bad.append(text)
    assert bad == [], bad[:3]


def test_a_vendor_secret_anywhere_in_ordinary_text_is_always_masked() -> None:
    """Fuzz (tests target design 1): every vendor-table secret, placed at a random spot among random ordinary text and
    separators, is gone from the output, whatever surrounds it; the text around it stays readable."""
    rng = random.Random(12)
    separators = [" ", "\n", ", ", "; ", "\t", " - ", "\n\n", ". "]
    leaks = []
    r = Redactor()
    for _ in range(3000):
        _, text, secret, _ = rng.choice(VENDOR_TABLE)
        before = [rng.choice(ORDINARY) for _ in range(rng.randint(0, 3))]
        after = [rng.choice(ORDINARY) for _ in range(rng.randint(0, 3))]
        whole = "".join(p + rng.choice(separators) for p in before) + text + \
            "".join(rng.choice(separators) + p for p in after)
        if secret in r.text(whole):
            leaks.append(whole)
    assert leaks == [], leaks[:3]
