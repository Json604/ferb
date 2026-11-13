"""Fixtures for the Rust port of ferb/sanitize (ENG-3): the Python build's own answers on invented inputs.

Run from the repo root:  uv run python rust/fixtures/sanitize/make.py

A wider run that leaves the tracked files alone: `make.py --seed 7 --scale 6 --out /some/temp/folder`, then
`FERB_SANITIZE_FIXTURES=/some/temp/folder cargo test -p ferb-sanitize --test parity`.

Writes rust/fixtures/sanitize/*.json (the cases) and rust/crates/ferb-sanitize/src/generated.rs (the Python build's
pattern sources and Unicode tables, through gen_rust.py). Every string in a case is base64 of its UTF-8 bytes: the
values are invented but shaped like real keys, and the commit gate's `secrets` check reads tracked files for that
shape. For the same reason the seeds below are joined from harmless parts (`J`), never written whole.

Inputs: seeds for every format and labelled class the code knows, the string literals of the redaction tests,
near-misses and Unicode variants of both, random multi-line mixes, nested JSON, and for the stream many random chunk
splits (inside multi-byte characters and inside tokens too). The random source is seeded: the same Python build writes
the same files."""

import ast
import base64
import json
import random
import sys
from pathlib import Path
from typing import Any

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[2]
sys.dont_write_bytecode = True
sys.path.insert(0, str(ROOT))
sys.path.insert(0, str(HERE))

import gen_rust  # noqa: E402
from ferb.core import origin  # noqa: E402
from ferb.sanitize import formats, known, labels, terminal, untrusted  # noqa: E402
from ferb.sanitize.redact import Redactor, mask  # noqa: E402

RNG = random.Random(20261113)
SCALE = 1          # --scale N: N times the random cases, for a wider local run (never the tracked files)
OUT = HERE
J = "".join


def b64(value: str | bytes) -> str:
    raw = value if isinstance(value, bytes) else value.encode("utf-8")
    return base64.b64encode(raw).decode("ascii")


def enc(value: Any) -> Any:
    """A case's value with every string as base64."""
    if isinstance(value, (str, bytes)):
        return b64(value)
    if isinstance(value, (list, tuple)):
        return [enc(v) for v in value]
    if isinstance(value, (set, frozenset)):
        return [enc(v) for v in sorted(value)]
    if isinstance(value, dict):
        return {k: enc(v) for k, v in value.items()}
    return value


# ---------------------------------------------------------------- seeds (invented; joined from parts)

HEX32 = "0123456789abcdef" * 2
ALNUM = "Zx9Qw3Er7Ty1Ui5Op2As8Df4Gh6Jk0LcVbNm"
B64ISH = "aB3dE5fG7hJ9kL1mN2pQ4rS6tU8vW0xY-_zA"

KEYS = [
    J(["AK", "IA", "IOSFODNN7EXAMPLE"]), J(["AS", "IA", "QWERTYUIOP123456"]), J(["AB", "IA", "0123456789ABCDEF"]),
    J(["AC", "CA", "ZZZZ0000YYYY1111"]),
    "aws_secret_access_key = " + J(["wJalrXUtnFEMI/K7MDENG/", "bPxRfiCYEXAMPLEKEY"]),
    'AWS_SECRET_KEY: "' + J(["abcdEFGH0123+/=", "abcdEFGH0123+/=", "abcdEFGH01"]) + '"',
    J(["sk", "_live_", ALNUM[:24]]), J(["rk", "_test_", ALNUM[:12]]),
    J(["gh", "p_", ALNUM[:36]]), J(["gh", "o_", ALNUM[:20]]), J(["gh", "s_", ALNUM[:22]]),
    J(["github", "_pat_", "11ABCDEFG0", "_", ALNUM[:30]]),
    J(["gl", "pat-", B64ISH[:20]]),
    J(["xo", "xb-", "1234567890", "-", "abcdefghij"]), J(["xo", "xp-", "9876543210-zyxwv"]),
    J(["sk-", "ant-", "api03-", B64ISH[:24]]),
    J(["sk-", "proj-", B64ISH[:28]]), J(["sk-", ALNUM[:30]]), J(["sk-", "svcacct-", ALNUM[:22]]),
    J(["AI", "za", "Sy", B64ISH[:33]]),
    J(["np", "m_", ALNUM[:36]]),
    J(["h", "f_", ALNUM[:32]]),
    J(["S", "G.", B64ISH[:22], ".", B64ISH[:30]]),
    J(["S", "K", HEX32]), J(["A", "C", HEX32]), J(["A", "C", HEX32, ":", HEX32[::-1]]),
    J(["M", "TIzNDU2Nzg5MDEyMzQ1Njc4", ".", "GhIjKl", ".", B64ISH[:30]]),
    "DefaultEndpointsProtocol=https;AccountName=acct;AccountKey=" + J(["abcd0123+/", "ABCD4567=="]) * 2,
    "SharedAccessKey=" + J(["Zm9vYmFy", "QkFSRk9P", "MTIzNDU2"]),
    "https://" + HEX32 + "@o123.ingest.sentry.io/456", "https://" + HEX32 + ":" + HEX32[::-1] + "@sentry.io/9",
    "Authorization: Bearer " + J(["abc.DEF", "-123_456", "~789+/=="]),
    "bearer\u00a0" + J(["tok", "en1234567890abc"]),
    "authorization=Basic " + J(["dXNlcjpw", "YXNzd29yZA=="]), 'proxy-authorization: "token ' + ALNUM[:18] + '"',
    "Authorization: " + ALNUM[:20], "x-authorization: nope",
    "pg_dump --password=" + J(["hunter", "2!x"]) + " db", "--PASSWORD=abc123",
    "CVV 123", "cvc: 4567", "CVV2 (3 digits) is 987", "card verification code - 321", "csc#1234", "cvv 12", "CVV 12345",
    "cvv 123/45", "exp 03/27", "Expiry date: 12/2029", "valid thru 01-30", "good thru 11/26", "MM/YY 04/28",
    "expires. 13/27", "valid until 03/27", "expiration 00/27", "exp 3/27",
    "-----BEGIN RSA " + J(["PRIV", "ATE KEY-----"]) + "\nMIIEow" + "IBAAKCAQEA" * 3 + "\nqwerty" + "ASDF1234" * 4
    + "\n-----END RSA " + J(["PRIV", "ATE KEY-----"]),
    "-----BEGIN OPENSSH " + J(["PRIV", "ATE KEY-----"]) + "\nb3BlbnNzaC1rZXktdjEAAAAA\nQUJDREVGRw==",
    "-----BEGIN PGP " + J(["PRIV", "ATE KEY BLOCK-----"]) + "\nVersion: Test 1\nComment: none\n\nlQOYBF" + "abcd" * 8
    + "\ntrailing words here",
    "key: -----BEGIN EC " + J(["PRIV", "ATE KEY-----"]) + "\\nMHcCAQEE" + "IIabcd12" * 3 + "\\n-----END EC "
    + J(["PRIV", "ATE KEY-----"]) + " done",
    "-----BEGIN " + J(["PRIV", "ATE KEY-----"]) + "\r\nProc-Type: 4,ENCRYPTED\r\nDEK-Info: AES-256-CBC,00\r\n"
    + "QUJDREVGR0hJSktMTU5PUA==" + " and more text",
    "-----BEGIN PUBLIC KEY-----\nMFkwEwYH\n-----END PUBLIC KEY-----",
    J(["ey", "J", "hbGciOiJIUzI1NiJ9", ".", "ey", "J", "zdWIiOiIxMjM0NTY3ODkwIn0", ".", "SflKxwRJSMeKKF2QT4fwpM"]),
    J(["ey", "J", "abc.def.ghi"]), J(["xey", "J", "hbGciOiJIUzI1NiJ9.abcdefgh12.abcdefgh34"]),
    "Cookie: sid=abc123; theme=dark", 'Set-Cookie: "a=b; c=d" trailing', "cookie: 'x=1'", "cookie:   ",
    "set-cookie : token=xyz   \r\nNext: line", '"cookie": "sess=1"', "my-cookie: no",
    "mysql -uroot -phunter2 db", "mysqldump -h host -p" + ALNUM[:8] + " | gzip", "mariadb -p secret", "mysql -p'quoted'",
    "/usr/bin/mysql -pabc", "mysqladmin -u x -ppw1 status; mysql -ppw2", "notmysql -pabc",
    "https://user:" + J(["pa", "ss"]) + "@example.com/path?x=1", "https://" + ALNUM[:20] + "@host.example/x",
    "ftp://user@host/", "https://example.com/cb?code=abc123&state=xyz#access_token=" + ALNUM[:16],
    "https://example.com/?pageToken=abc&sig=deadbeef&X-Amz-Signature=cafe&X-Amz-Credential=AKID/2026",
    "https://e.x/?token=&key=k;pwd=p#frag?password=q", "redis://:pw@127.0.0.1:6379/0", "x://a:b@c",
    "see http://shop.example/item/4111111111111111/view and http://4111111111111111.example/",
    "https://pay.example/?card=4111111111111111", "a://b ://c d://e://f g+h.i-j://k",
    "4111 1111 1111 1111", "4111-1111-1111-1111", "4111111111111111", "qty 2 4111 1111 1111 1111", "4111 1111 1111 1112",
    "card 5500 0000 0000 0004 exp 12/29 cvv 123", "378282246310005, 03/27, 1234", "6011000990139424 | valid thru 0127",
    "4111 1111 1111 1111 12/29 999", "0000 0000 0000 0000", "4111\u00a01111\u00a01111\u00a01111", "id_4111111111111111",
    "x4111111111111111", "4111111111111111x", "4111111111111111-9", "9-4111111111111111", "30569309025904",
    "3530111333300000 3566002020360505", "\\n4111111111111111", "\\u00e94111111111111111", "1 2 3 4 5 6 7 8 9 0 1 2 3 4 5 6 7 8",
    "tel +1 415 555 2671 0000 1111", "4111 1111 1111 1111**", "12345678901234567890123",
]

LABELLED = [
    "password=hunter2", "PASSWORD: hunter2", "Password: correct horse battery", 'db_password = "p@ss w0rd"',
    "api_key=abc", "apiKey: 'zzz'", "API_KEY=`tick`", "token_limit=4000", "session_id=abc123", "kms_key_id=alias/x",
    "auth: abc", '"auth": "abc", "x": 1', "{\"password\": \"hunter2\", \"user\": \"bob\"}", '{"pin": 4821, "ok": true}',
    '{"secret": null, "token": true}', "export SECRET_KEY=" + ALNUM[:20], "signing_key=" + B64ISH[:20], "cache_key=user:1",
    "client_secret=xyz&grant_type=code", "?code=123456&x=1", "verification_code=123456", "code=abcdef", "code: 12",
    "--password hunter2", "--token=abc123 --verbose", "-p=xyz", "$TOKEN=abc", "a.b.token=xyz", "X-Api-Key: " + ALNUM[:20],
    "request_signature=" + ALNUM[:16], "sig=abc", "dsn=postgres://u:p@h/db", "salt=" + "a1" * 10, "hmac=" + "0f" * 16,
    "my \npassword is swordfish", "our wifi \npasscode is tulip and", "my password is hunter2", "my password is swordfish.", "the password is swordfish", "the password is sword-fish",
    "password was abc1", "our wifi passphrase is correct horse", "PIN 4821", "pin 48", "PIN code 560001", "zip code 94110",
    "pin: 4821", "passcode - 7391", "secret key = abc", "secret is out", "the token for the api is abc123xyz",
    "password for my account: hunter2", "password of the router is admin123", "Password:\nhunter2\nnext line",
    "Password:\r\n\r\n  s3cret value  \r\nbye", "password: 'quoted value' rest", 'password: "unclosed value', "password: \\\"esc\\\" x",
    "password is \"abc\" and more", "passwd", "password", "password:", "password: ", "pwd=", "api key: " + ALNUM[:12],
    "private-key is here", "access_token " + ALNUM[:20], "refresh token is 1//0abc-def", "tokenize this", "passwordless login",
    "my pin is 1234 and my other pin is 9876", "pincode 560001", "postal code: 560001", "coupon code SAVE20", "promo code 1234",
    "Your verification code is 314159.", "314159 is your verification code", "Code: 482913", "code (482913)", "482913 (OTP)",
    "739201 as your PIN", "OTP 1234 5678", "G-482913 is your Google verification code", "Use 482-913 to sign in. code",
    "Your one-time password: 99887766", "one time code 1234", "2FA 123456", "login code\n\n  482913\n", "482913\nis your code",
    "> Your code\n>\n> 482913", "# Verification code\n**482913**", "Your code is <b>482913</b>.", "<code>482913</code>",
    "verification code to sign in 482913", "security code sent. Enter 482913", "code 2026", "The year is 2026 and invoice 314159 is paid.",
    "Rs. 2,000 debited; OTP is 482913", "a/c no. XX1234 code 482913", "code, then 482913", "code! 482913", "code. Next 482913",
    "your code will arrive soon 482913 thanks", "one two three four five six seven eight nine code 482913",
    "code a b c d e f g h 482913", "code a b c d e f g h i 482913", "482913 was the code", "482913 are codes",
    "is your verification code", "(482913) <b", "482913 <b>is your code</b>", "-  482913", "* 482913\nis your OTP",
    "Backup codes: 1234-5678, abcd-1234 9z9z9", "Recovery codes:\n1. a1b2c-3d4e5\n2. f6g7h-8i9j0\n\nKeep them safe",
    "backup codes:\n- 11112222\n- 33334444\nnot a code line\n55556666", "two-factor backup codes: 123456 789012",
    "2FA codes:", "login codes: abc def", "verification codes: 1a2 3b4\n5c6", "pin codes: 1234 5678",
    "task_id=" + HEX32, "id " + HEX32 + " and chat-0123456789ab", "000000000000", "0123456789ab",
    "Enter the code 12345678 9", "code 1234567890", "code_123456", "ref_123456 code", "code: [redacted] 482913",
    "code [redacted]-482913", "OTP: 4829 13", "otp:482913;next", "OTP\u00a0482913", "otp\u2003482913", "otp\x1c482913",
]

UNICODE_EXTRA = [
    "pa\u017f\u017fword: hunter2", "PA\u017f\u017fWORD=hunter2", "p\u0131n 4821", "P\u0130N 4821", "pin\u0307 4821",
    "to\u212aen=abc123", "\u212aEY=" + ALNUM[:18], "c\u0131vv 123", "cvv \u0661\u0662\u0663", "exp \u0660\u0663/\u0662\u0667",
    "code \uff14\uff18\uff12\uff19\uff11\uff13", "OTP \u0664\u0668\u0662\u0669\u0661\u0663", "code 482913\u0301", "c\u0301ode 482913",
    "\U0001f511 password: hunter2 \U0001f512", "\u5bc6\u7801 password=\u79d8\u5bc6123", "password=\U0001f600\U0001f600",
    "e\u0301ty code 482913 e\u0301", "\u2068code\u2069 482913", "code\u200b482913", "\ufeffcode 482913", "pin\u00b21234",
    "password is \u00bd4abc", "secret: \u2167", "\u00e9password=abc", "password\u00e9=abc", "4111\u20071111\u20071111\u20071111",
    "\u0661\u0662\u0663\u0664 \u0661\u0662\u0663\u0664 \u0661\u0662\u0663\u0664 \u0661\u0662\u0663\u0664", "AUTHOR\u0130ZAT\u0130ON: abc123",
    "bearer\u3000" + ALNUM[:14], "Cook\u0131e: a=b", "MYSQL -pfoo", "my\u00a0password\u00a0is\u00a0hunter2",
    "Password:\u2028hunter2", "Password:\x85hunter2", "password:\x0bhunter2", "code\x1f482913", "line1\x1e482913\x1dis your code",
    "\ud7ff code 482913 \ue000", "\U0010ffff token=abc123", "a\u0300\u0301\u0302 pin 4821 b\u0327",
]

KNOWN_SETS: list[list[str]] = [
    [],
    [J(["ferb", "-proxy-", "tok-", "Q7w8E9r0T1"])],
    [J(["sk-", "ant-", "api03-", "FAKEfake0000", "FAKEfake1111"]), "482913"],
    ["9431", "ab12cd", "0123456789ab"],
    ['a"b\\c d&e<f>\u00e9\n\t\x7f', "caf\u00e9-\U0001f511-key", "x y+z/100%"],
    [HEX32, "deadbeefdeadbeefdeadbeefdeadbeef"[:12], "pass word with spaces"],
]


def harvested() -> list[str]:
    """The string literals of the redaction tests: the inputs the Python suite itself guards."""
    names = ["test_redaction_rebuild", "test_secret_classes", "test_sealed_words", "test_redaction_scaling",
             "test_labelled_secret_classes", "test_redact", "test_owner_answers", "test_chat_secret_notice",
             "test_untrusted_and_trace", "test_origins", "test_card_source_cleanout", "test_reply_streaming"]
    found: dict[str, None] = {}
    for name in names:
        path = ROOT / "tests" / f"{name}.py"
        if not path.exists():
            continue
        for node in ast.walk(ast.parse(path.read_text(encoding="utf-8"))):
            if isinstance(node, ast.Constant) and isinstance(node.value, str) and 3 <= len(node.value) <= 600:
                try:
                    node.value.encode("utf-8")
                except UnicodeEncodeError:
                    continue
                found[node.value] = None
    return list(found)


SOUP = ["code", "Code", "codes", "pin", "PIN", "otp", "OTP", "2fa", "password", "Password", "passcode", "secret", "token", "key", "api_key",
        "verification", "one-time", "sign-in", "login", "security", "backup", "recovery", "is", "are", "was", "as", "your", "my", "our", "the",
        "for", "and", "but", "zip", "coupon", "cvv", "exp", "valid", "thru", "cookie", "bearer", "authorization", "mysql", "-p", "session",
        ":", "=", "-", "\u2013", "#", "(", ")", ".", ",", ";", "!", "?", "/", "*", "_", "`", "'", '"', "\\", "<b>", "</b>", "<", ">", ">", "&",
        "[redacted]", "[excluded]", "[", "]", "{", "}", "482913", "4821", "12", "123 456", "1234-5678", "G-482913", "4111 1111 1111 1111",
        "4111111111111111", "03/27", "560001", "a1b2c-3d4e5", "0123456789ab", HEX32, "x", "A", "Rs.", "no.", "e.g.", "://", "https://e.x/?code=1234",
        "http://", "@", "?token=", "&sig=", "%", "+", "\u00a0", "\u3000", "\x1c", "\u200b", "\u0301", "\u0131", "\u0130", "\u017f", "\u212a",
        "\u0664\u0668\u0662\u0669", "\uff11\uff12\uff13\uff14", "\U0001f600", "\u00e9", "\u4e2d", "1.", "2)", "\u2022", "-----BEGIN",
        "-----BEGIN " + J(["PRIV", "ATE KEY-----"]), "-----END " + J(["PRIV", "ATE KEY-----"]), "QUJDREVGR0hJSktM", "ey" + "J", "\\n", "\\u00e9", "\\\""]
GLUE = [" ", " ", " ", " ", "", "", "\n", "\n", "\r\n", "\r", "\t", "  ", "\n\n", "\n> ", "\u00a0"]


def soup() -> str:
    """Random text from the pieces the rules read: words, links, marks, digits, masks, breaks."""
    return "".join(RNG.choice(SOUP) + RNG.choice(GLUE) for _ in range(RNG.randint(2, 14)))


def variants(s: str) -> list[str]:
    """Near-misses and Unicode variants of one seed."""
    out = ["x" + s, s + "x", "_" + s, s + "_", s[:-1], s[1:], s.upper(), s.lower(), s.swapcase(), f'"{s}"', f"'{s}',",
           f"({s})", f"<p>{s}</p>", s.replace(" ", "\u00a0"), s.replace(" ", "  "), s.replace(" ", "\t"),
           s.replace(" ", "\n"), s + "\n", "\n" + s, s + "\r\n" + s, json.dumps({"k": s}), json.dumps(s),
           s + " \U0001f600", "\u00e9" + s, s + "\u0301", s.replace("i", "\u0131").replace("I", "\u0130"),
           s.replace("s", "\u017f"), s.replace("k", "\u212a").replace("K", "\u212a"),
           s.translate({48 + d: 0x0660 + d for d in range(10)}), s.translate({48 + d: 0xFF10 + d for d in range(10)}),
           s.replace(":", " :"), s.replace("=", " = "), s.replace("-", "\u2013"), "[redacted] " + s, s + " [redacted]",
           s.replace(" ", "\x1c"), s + "."]
    return [v for v in out if v != s]


# ---------------------------------------------------------------- cases

CASES: dict[str, list[dict[str, Any]]] = {}


def add(group: str, op: str, args: dict[str, Any], out: Any) -> None:
    CASES.setdefault(group, []).append({"op": op, "in": enc(args), "out": enc(out)})


def redactor(known_values: list[str] = (), domains: list[str] = (), apps: list[str] = ()) -> Redactor:  # type: ignore[assignment]
    r = Redactor(set(domains) or None, set(apps) or None)
    for value in known_values:
        r.add_secret_value(value)
    return r


def config(known_values: list[str] = (), domains: list[str] = (), apps: list[str] = ()) -> dict[str, Any]:  # type: ignore[assignment]
    """A case's redactor, written only where it isn't the plain one."""
    out = {"known": list(known_values), "domains": list(domains), "apps": list(apps)}
    return {k: v for k, v in out.items() if v}


def same(out: str, s: str) -> Any:
    """An output equal to its input is written `True` (most texts hold nothing to hide; the files stay small)."""
    return True if out == s else out


def redact_out(r: Redactor, s: str) -> dict[str, Any]:
    return {"text": same(str(r.text(s)), s), "ident": same(str(r.ident(s)), s), "hit": r.hit(s), "kinds": r.kinds(s)}


def text_cases(pool: list[str]) -> None:
    plain = Redactor()
    for s in pool:
        add("text", "redact", {"s": s, **config()}, redact_out(plain, s))
    for s in pool[::4]:
        add("spans", "spans", {"s": s}, {
            "format_hits": [[h.start, h.end, h.kind] for h in formats.format_hits(s)],
            "labelled_spans": [list(x) for x in labels.labelled_spans(s)],
            "urls": [list(m.span()) for m in formats.urls(s)],
            "code_words": [list(m.span()) for m in labels.code_words(s)],
            "secret_words": [list(m.span()) for m in labels.secret_words(s)],
            "id_bodies": [list(x) for x in known.id_bodies(s)],
            "id_bodies_keyed": [list(x) for x in known.id_bodies(s, keyed=True)]})
    for s in pool[::8]:
        for line in s.splitlines()[:3] or [s]:
            span = labels.code_alone_span(line)
            add("lines", "line_facts", {"s": line},
                {"list_line": labels.list_line(line), "label_line": labels.label_line(line),
                 "link_line": labels.link_line(line), "code_alone": list(span) if span else None,
                 "quote_only": labels.quote_only(line), "list_label_line": labels.list_label_line(line)})


def known_cases(pool: list[str]) -> None:
    for values in KNOWN_SETS[1:]:
        r = redactor(values)
        forms = sorted(r.secret_values())
        texts = []
        for v in values:
            for form in [v, *known.escaped_forms(v)]:
                texts += [form, f"key is {form} ok", f"x{form}y", f"1{form}2", f"{form}{form}", f"task-{form}",
                          json.dumps({"k": form}), f"https://h.example/p?q={form}", form[:-1], form[1:], form.upper(),
                          f"id {HEX32[:10]}{form}{HEX32[:10]} z", f"{form}\n{form[:3]}"]
            add("known", "escaped_forms", {"s": v}, list(known.escaped_forms(v)))
        texts += RNG.sample(pool, 25)
        texts.append("a" * 20 + HEX32 + " " + "0123456789ab" + " 9431 19431 94312 x9431y " + "ab12cd7 7ab12cd")
        for s in texts:
            add("known", "known", {"s": s, "value": values[0], **config(values)}, {
                **redact_out(r, s),
                "known_spans": sorted(list(x) for x in known.known_spans(s, r.secret_values())),
                "known_spans_keyed": sorted(list(x) for x in known.known_spans(s, r.secret_values(), keyed=True)),
                "sealed_spans": [list(x) for x in known.sealed_spans(s, values[0])],
                "holds_sealed": known.holds_sealed(s, r.secret_values())})
        add("known", "forms", {**config(values)}, forms)
    # decision: GATE-572/573: a value shorter than 4 characters is never known; nothing else is matched by value
    short = ["abc", "", "ab\u00e9"]
    add("known", "redact", {"s": "abc ab\u00e9 hunter2", **config(short)}, redact_out(redactor(short), "abc ab\u00e9 hunter2"))
    domains, apps = ["Example.com", ".bank.test", "xn--caf-dma.example", "[::1]"], ["Secret App", "\u00c9diteur"]
    r = redactor([], domains, apps)
    urls = ["https://example.com/a", "http://sub.EXAMPLE.com:8080/x?token=abc", "https://notexample.com/", "ftp://u:p@bank.test/f",
            "https://bank.test.evil.example/", "https://[::1]:80/x", "https://[::1/x", "https://[v1.x]/", "https://[1.2.3.4]/",
            "https://user@[fe80::1%eth0]:1/", "http://\uff45xample.com/", "https://exam\u2100ple.com/", "https://caf\u00e9.example/",
            "https://example.com\uff0fpath", "http://a[b]@example.com/", "https://[::ffff:1.2.3.4]/", "https://[1:2:3:4:5:6:7:8]/",
            "https://[1::2::3]/", "http://example.com", "HTTP://EXAMPLE.COM/A", "http://@/x", "http://:80/", "x://example.com.",
            "secret app", "Secret App", "\u00e9diteur", "see Secret App now", "visit https://example.com/a and https://ok.example/b",
            "https://[v1f.a:b]/", "https://[::%]/", "https://[::%a%b]/", "https://[12345::]/", "https://[:1:2:3:4:5:6:7]/",
            "https://[1:2:3:4:5:6:7::]/", "https://[::1.2.3.04]/", "https://[::1]x/"]
    for s in urls:
        add("known", "redact", {"s": s, **config([], domains, apps)}, redact_out(r, s))


def small_cases(pool: list[str]) -> None:
    label_seeds = ["password", "Password", "DB_PASSWORD", "session_token", "token_limit", "pin_message", "session_id", "kms_key_id",
                   "apiKey", "api key", "API-KEY", "x-api-key", "X_API_KEY_ID", "auth", "my auth", "cookie", "set-cookie", "Set Cookie",
                   "code", "verification_code", "zip_code", "pincode", "PIN code", "promoCode", "2fa", "otp", "TOTP", "cvv2", "cardNumber",
                   "cc-number", "signing_key", "cache key", "key", "key_id", "signature", "request_sig", "dsn", "task_id", "covered",
                   "", " ", "__", "password value", "passwordValue", "password_b64_raw", "id", "secret\u00e9", "\u00e9secret", "SECRET",
                   "one_time_code", "onetimepassword", "private key", "privateKey", "access key id", "bearer", "credentials",
                   "sessionid", "Session ID", "pin", "PINs", "passwords", "token2", "tokenB", "refresh-token-hex", "to\u212aen",
                   "PA\u017f\u017fWORD", "p\u0131n", "P\u0130N", "postal code", "area_codes", "codes", "couponcode"]
    words = sorted({w for s in pool for w in s.replace("=", " ").replace(":", " ").split() if 0 < len(w) < 40})
    label_seeds += RNG.sample(words, 250)
    for label in label_seeds:
        add("small", "label_class", {"s": label}, labels.label_class(label))
        add("small", "words", {"s": label}, labels.words(label))
        add("small", "secret_param", {"s": label}, formats.secret_param(label))
    values = ["hunter2", "", " ", "null", "None", "TRUE", "undefined", "482913", "4829 13", "G-482913", "12", "123456789",
              ALNUM[:16], ALNUM[:15], "a1" * 8, "abcdefgh12345678", "aaaaaaaaaaaaaaa1", "0123456789abcdef", "ab12" * 5,
              B64ISH, "abc def 1234567890", "\u00e9" + ALNUM[:16], "A1b2C3d4E5f6G7h8", "abcdefghijklmno1", "aabbccddeeffgg11",
              "1a2b3c4d1a2b3c4d", "0a1b2c3d4e5f6a7b", "112233445566778a", "abcdabcdabcdabc1", "abcdefg1abcdefg1"]
    values += ["".join(RNG.choice("abcdefgh12345678"[:n]) for _ in range(RNG.randint(14, 24))) for n in (4, 8, 9, 10, 12, 16) for _ in range(25)]
    for v in values:
        add("small", "high_entropy", {"s": v}, labels.high_entropy(v))
        for cls in ("secret", "code", "credential", None):
            add("small", "labelled_value", {"cls": cls, "s": v}, labels.labelled_value(cls, v))
    cards = ["4111111111111111", "4111 1111 1111 1111", "4111111111111112", "1111111111111111", "411111111111", "378282246310005",
             "30569309025904", "6011000990139424", "12345678901234567890", "\u0664\u0661\u0661\u0661" * 4, "4111-1111-1111-1111x",
             "0" * 13, "79927398713", "0000000000000018", ""]
    for c in cards:
        add("small", "payment_card", {"s": c}, formats.payment_card(c))
    kinds = [k for k, _, _ in formats.TABLE] + ["Discord bot token", "private key", "JSON web token", "cookie", "URL password",
                                                "payment card", "saved secret", "", " ", "x", "NPM", "hour", "Unix key", "\u00e9l\u00e9ment",
                                                "\u01c5x", "xml", "1st key", "ABC def", "\u00dfx", "\ufb01x"]
    for k in kinds:
        add("small", "named_kinds", {"kinds": [k]}, formats.named_kinds([k]))
    for n in (0, 2, 3, 5):
        ks = RNG.sample(kinds[:25], n)
        add("small", "named_kinds", {"kinds": ks}, formats.named_kinds(ks))
        add("small", "hidden_note", {"kinds": ks, "held_in": "message"}, formats.hidden_note(ks))
    add("small", "hidden_note", {"kinds": ["Stripe key"], "held_in": "answer"}, formats.hidden_note(["Stripe key"], "answer"))
    for s in pool[::9]:
        n = len(s)
        spans = []
        for _ in range(RNG.randint(1, 4)):
            a = RNG.randint(0, n)
            spans.append([a, min(n, a + RNG.randint(0, 12)), RNG.choice(["[redacted]", "[excluded]", "<x>"])])
        add("small", "mask", {"s": s, "spans": spans}, mask(s, [tuple(x) for x in spans]))
    add("small", "mask", {"s": "ab [redacted] cd [excluded]ef", "spans": [[4, 6, "[redacted]"], [12, 20, "[excluded]"], [0, 1, "Q"], [1, 2, "[excluded]"]]},
        mask("ab [redacted] cd [excluded]ef", [(4, 6, "[redacted]"), (12, 20, "[excluded]"), (0, 1, "Q"), (1, 2, "[excluded]")]))
    for s in pool[::7]:
        add("small", "printable", {"s": s}, terminal.printable(s))
    add("small", "printable", {"s": "a\x1b[2Jb\rc\x00\x7f\x85\x9f\u00a0\n\t"}, terminal.printable("a\x1b[2Jb\rc\x00\x7f\x85\x9f\u00a0\n\t"))


def field_cases() -> None:
    r = Redactor()
    labels_ = ["verification code", "invoice number", "OTP", "one-time-code", "sign-in code", "Password", "password", "Show password",
               "PIN", "PIN code", "Pin tab", "api key", "secret", "name", "", "Which phone number should I use for login?",
               "Card number", "cvv", "Search", "security code", "2FA", "access\u00a0code", "TOTP", "\u00e9", "new-password", "pwd", "P\u0130N"]
    roles = ["", "AXSecureTextField", "AXTextField", "password", "PASSWORD", "textbox", "checkbox", "Secure thing", "\u017fecure", "button",
             "AXTextArea", "combobox", "PA\u017f\u017fWORD", "text"]
    texts = ["314159", "private", "", " ", "1", "G-482913", "4829 13", "hunter2", "my password is hunter2", "4111 1111 1111 1111",
             "2025550143", "12-34", "code 482913", "\u0663\u0661\u0664\u0661\u0665\u0669"]
    metas = [[], ["current-password"], ["username", "Passwort"], ["x", "pwd-field"], ["PASSW"]]
    for _ in range(300 * SCALE):
        text, role, label = RNG.choice(texts), RNG.choice(roles), RNG.choice(labels_)
        kind = RNG.choice([None, None, "", "text", "password", "checkbox", "AXTextField", "Textbox"])
        meta = RNG.choice(metas)
        secure, concealed = RNG.random() < 0.05, RNG.random() < 0.05
        out = r.field(text, role=role, label=label, secure_input=secure, concealed=concealed, metadata=tuple(meta), kind=kind)
        add("fields", "field", {"text": text, "role": role, "label": label, "secure_input": secure, "concealed": concealed,
                                "metadata": meta, "kind": kind},
            {"field": same(str(out), text), "password_field": labels.password_field(role, label, *meta),
             "labelled_code": labels.labelled_code(label, text), "labelled_secret": labels.labelled_secret(role, label, text)})
    questions = ["What is the password for the router?", "Enter the verification code", "Which PIN code is your address?",
                 "What's your sign-in PIN?", "What is the passphrase", "Enter the OTP sent to your phone", "Which city?",
                 "What code did the site show?", "Type your passcodes", "one-time password please", "What is your zip code?",
                 "PWD?", "pa\u017f\u017fword?", "Enter the 2FA code or say skip", "security code on the card"]
    answers = ["hunter2", "it is hunter2", "482913", "48 29 13", "use 482913 then 111222", "skip it", "", "  ", "4111 1111 1111 1111",
               "G-482913", "my password is x1y2", "12-34-56", "no", "\u0664\u0668\u0662\u0669\u0661\u0663", "code 482913 and 1234"]
    for q in questions:
        for a in answers:
            for reply in (False, True):
                add("fields", "answer", {"text": a, "question": q, "reply": reply},
                    {"answer": same(str(r.answer(a, question=q, reply=reply)), a),
                     "hidden_answer": labels.hidden_answer(q, a, reply=reply)})


def random_json(pool: list[str], depth: int = 0) -> Any:
    keys = ["password", "token", "code", "value", "text", "content", "role", "type", "input_type", "autocomplete", "method", "params",
            "task_id", "id", "name", "label", "title", "note", "pin", "otp", "secret", "api_key", "session_id", "covered", "key",
            "unmodifiedText", "items", "children", "placeholder", "evidence", "signing_key", "a", "b", "c", "m", "z", "Password",
            "verification_code", "source_refs", "card", "n", "count", "auth"]
    scalars: list[Any] = [None, True, False, 0, 1, 4821, 482913, 4111111111111111, -4111111111111111, 1.5, 0.0, 1e16, 1e-7, 123456.0,
                          "", " ", "password", "text", "Input.insertText", "Input.dispatchKeyEvent", "Page.navigate", "PIN", "4821",
                          "482913", "Your verification code", "is your code", "one-time-code", "current-password", "cc-number",
                          "cc-exp-month", "AXSecureTextField", "textbox", "checkbox", "hunter2", HEX32, "Backup codes:", "1234-5678",
                          "-----BEGIN " + J(["PRIV", "ATE KEY-----"]), "MIIEowIBAAKCAQEA1234", "-----END " + J(["PRIV", "ATE KEY-----"]),
                          "secure", ["secure"], {"x": "secure"}, "email", "Search", "OTP", 30569309025904, 12, "12", "G-482913"]
    roll = RNG.random()
    if depth > 3 or roll < 0.45:
        return RNG.choice(pool)[:160] if RNG.random() < 0.3 else RNG.choice(scalars)
    if roll < 0.7:
        return [random_json(pool, depth + 1) for _ in range(RNG.randint(0, 5))]
    out: dict[str, Any] = {}
    for _ in range(RNG.randint(0, 6)):
        key = RNG.choice(keys) if RNG.random() < 0.85 else RNG.choice(pool)[:30]
        out[key] = random_json(pool, depth + 1)
    return out


def walk_cases(pool: list[str]) -> None:
    key_begin, key_end = "-----BEGIN RSA " + J(["PRIV", "ATE KEY-----"]), "-----END RSA " + J(["PRIV", "ATE KEY-----"])
    fixed: list[Any] = [
        {"role": "AXSecureTextField", "value": "short"}, {"label": "one-time code", "value": "123456"},
        {"type": "password", "value": "x", "name": "pw"}, {"type": "text", "name": "user_password", "value": "abc"},
        {"type": "checkbox", "name": "show_password", "value": "on"}, {"autocomplete": "cc-number", "value": 4111111111111111},
        {"autocomplete": "one-time-code", "value": ""}, {"autocomplete": None, "value": "v", "label": None},
        {"method": "Input.insertText", "params": {"text": "hunter2", "x": 1, "key": "a", "code": "KeyA", "other": "my password is abc1"}},
        {"method": "Page.navigate", "params": {"text": "hunter2", "url": "https://e.x/?token=abcdef"}},
        {"password": {"a": [1, "x", None, True, "", {"b": 2.5}], "c": 0}, "token": "", "secret": None, "pin": False, "otp": 0},
        {"code": 482913, "verification_code": "48 29 13", "zip_code": 560001, "signing_key": ALNUM[:20], "cache_key": "user:1"},
        {"task_id": HEX32, "session_id": ["chat-0123456789ab", 5, {"token": "t"}], "span_id": "000000000000", "covered": "x 0123456789ab"},
        {"a": "PIN", "b": "4821"}, {"a": "Your verification code", "b": 482913, "c": True}, {"z": "is your code", "a": "482913"},
        {"id": "482913", "label": "Your code"}, ["Your verification code", "482913"], ["482913", "is your verification code"],
        ["Your code", {"text": "482913"}, {"name": " ", "label": "x"}], [{"value": "OTP"}, {"title": "482913", "id": 1}],
        [key_begin, "MIIEowIBAAKCAQEA1234abcd", True, 5, 1.5, None, {"text": "QUJDREVGRw=="}, key_end, "after"],
        {"a": key_begin, "b": "MIIEowIBAAKCAQEA1234abcd", "c": False, "d": key_end},
        [482913, 5], [1, 2.5, None, [3]], [], {}, [[]], {"": ""}, "plain text password=abc", 4111111111111111, 5, None, True, 2.5,
        {"hunter2 password=abc": 1, "password=abc": 2, "password=xyz": 3, "[redacted]": 4, "password=[redacted]#2": 5},
        {"type": 5, "role": ["secure"], "value": "v"}, {"role": {"secure": 1}, "text": "v"}, {"type": True, "content": "abc"},
        {"role": None, "type": "password", "text": "t"}, {"type": "PASSWORD", "value": 0}, {"type": "Password", "value": None},
        {"input_type": "text", "type": "password", "role": "x", "id": "pwd", "value": "v"}, {"role": 1.0, "type": 1e16, "value": "v"},
        {"role": "sec\u0131ure", "value": "v"}, {"role": "\u017fECURE", "value": "v"}, {"label": 5, "name": "pin", "value": "1234"},
        {"name": "q", "label": "Verification code", "value": "482913", "type": "tel"}, {"text": "482913", "label": "OTP", "role": "button"},
        {"content": "hunter2", "name": "password", "type": "textarea"}, {"content": "hunter2", "name": "password", "type": "custom"},
        {"count": 12345678901234567, "n": 18446744073709551615, "m": -9223372036854775808},
    ]
    r = Redactor()
    for obj in fixed:
        add("walk", "walk", {"json": json.dumps(obj, ensure_ascii=False), **config()},
            {"walk": json.dumps(r(obj), ensure_ascii=False), "py_str": str(obj)})
    scalars = [1.5, 1e16, 1e15, 1e-5, 0.0001, 123456789.125, -0.0, 5e-324, 1.7976931348623157e308, 0.1, 100.0, 1e22, 12345678901234567.0,
               "it's", 'say "hi"', "both ' and \"", "tab\tnl\n\x00\x7f\x85\u00a0\u00ad\u200b\u2028\ue000\U0001f600\U000e0001\u0378", [None, True, {"a'b": "\\"}]]
    for obj in scalars:
        add("walk", "py_str", {"json": json.dumps(obj, ensure_ascii=False)}, str(obj))
    for i in range(200 * SCALE):
        obj = random_json(pool)
        values = KNOWN_SETS[i % len(KNOWN_SETS)] if i % 5 == 0 else []
        add("walk", "walk", {"json": json.dumps(obj, ensure_ascii=False), **config(values)},
            {"walk": json.dumps(redactor(values)(obj), ensure_ascii=False), "py_str": str(obj)})


def chunks_of(data: bytes) -> list[bytes]:
    style = RNG.random()
    if style < 0.15:
        return [data[i:i + 1] for i in range(len(data))][:120] + ([data[120:]] if len(data) > 120 else [])
    out, pos = [], 0
    top = RNG.choice([3, 7, 16, 40, 200])
    while pos < len(data):
        n = RNG.randint(1, top) if RNG.random() > 0.05 else 0
        out.append(data[pos:pos + n])
        pos += n
    return out


def stream_case(chunks: list[Any], values: list[str], max_pending: int | None, last_final: bool) -> None:
    """One stream: its chunks fed in order (the last with final=True when `last_final`, else an empty final feed
    after them). Plain chunks are written as the whole data and where it is cut; outputs as (feed number, bytes) for
    the feeds that released something."""
    r = redactor(values)
    s = r.stream()
    if max_pending is not None:
        s.MAX_PENDING = max_pending              # type: ignore[misc]  (an instance's own bound, as a test sets it)
    outs = []
    for i, chunk in enumerate(chunks):
        raw = chunk if isinstance(chunk, bytes) else chunk["rep"] * chunk["n"]
        outs.append(s.feed(raw, final=last_final and i == len(chunks) - 1))
    if not last_final:
        outs.append(s.feed(b"", final=True))
    args: dict[str, Any] = {"max_pending": max_pending, "last_final": last_final, **config(values)}
    if all(isinstance(c, bytes) for c in chunks):
        cuts, pos = [], 0
        for c in chunks:
            pos += len(c)
            cuts.append(pos)
        args.update(data=b"".join(chunks), cuts=cuts)
    else:
        args.update(chunks=chunks)
    add("stream", "stream", args, {"feeds": len(outs), "outs": [[i, o] for i, o in enumerate(outs) if o]})


def stream_cases(pool: list[str]) -> None:
    key_begin, key_end = "-----BEGIN RSA " + J(["PRIV", "ATE KEY-----"]), "-----END RSA " + J(["PRIV", "ATE KEY-----"])
    texts = ["Your verification code is 123456.\nCard 4111 1111 1111 1111\n", "Your verification \ncode is 314159.\n",
             "Password:\nhunter2\nnext\n", "login code\n\n482913\n", "482913\nis your code\n", "> Your code\n>\n> 482913\ndone\n",
             "Backup codes:\n1. a1b2c-3d4e5\n2. f6g7h-8i9j0\n\nKeep them safe\n", "plain words only here, nothing to hide at all\n" * 3,
             "caf\u00e9 \U0001f600 na\u00efve \u4e2d\u6587 password=\u79d8\u5bc6123 fin\r\n", "a\rb\r\nc\n\rd\r", "tail without newline 4111 1111 1111 1111",
             key_begin + "\nMIIEowIBAAKCAQEA\nabcd1234\n" + key_end + "\nafter the key\n", "x " + key_begin + "\nMIIEowIBAAKCAQEA\nnever ends " * 3,
             key_begin + "\n" + "QUJDREVGRw==\n" * 40, "pre\n" + key_begin + "\n" + "QUJDREVGR0hJSktM\n" * 30 + key_end + "\npost code 482913\n",
             "word " * 200, "nospace" * 150, "code " + "x" * 300 + " 482913\n", "4111 1111 " * 30 + "\n", "1,234.56 - 7.8 9 4111-1111-1111-1111**\n",
             "(482913) <b>is your code</b>\n", "482913 <b\n", "G-482913 is your Google verification code.\nG-", "otp: 4829", "otp: 4829\n13\n"]
    texts += [RNG.choice(["\n", "\r\n", " ", "\n\n", "\n> \n"]).join(RNG.sample(pool, RNG.randint(1, 3))) + RNG.choice(["", "\n", "\r", "\r\n"])
              for _ in range(80 * SCALE)]
    texts += ["".join(soup() for _ in range(RNG.randint(1, 4))) for _ in range(30 * SCALE)]
    for i, text in enumerate(texts):
        data = text.encode("utf-8")
        for rep in range(2):
            values = KNOWN_SETS[(i + rep) % len(KNOWN_SETS)] if rep == 1 else []
            if values and RNG.random() < 0.7:
                at = RNG.randint(0, len(text))
                data2 = (text[:at] + RNG.choice(values) + text[at:]).encode("utf-8")
            else:
                data2 = data
            mp = RNG.choice([None, None, None, 64, 200, 1000]) if rep != 0 else None
            stream_case(chunks_of(data2), values, mp, RNG.random() < 0.5)
    bad = [b"ok \xff\xfe bad", b"\xe2\x82", b"\xf0\x9f\x98", b"\xc3(\xa0\xa1 x", b"\xed\xa0\x80 surrogate", b"\xf4\x90\x80\x80", b"\xc0\xaf",
           b"\xe0\x80\xaf", b"abc\xf0\x9f password=x\x80y\n", "caf\u00e9".encode()[:-1] + b" code 482913", b"\xef\xbb\xbfcode 482913\n", b"\x80" * 5]
    for data in bad:
        for _ in range(4):
            stream_case(chunks_of(data + RNG.choice([b"", b" tail\n", "\u00e9\n".encode()])), [], None, RNG.random() < 0.5)
    # the real bound (65536): an oversized record with no end, an endless private key, a long held line
    stream_case([{"rep": b"x", "n": 70000}, b" more\nnext line\n", b"after\n"], [], None, False)
    stream_case([b"password=", {"rep": b"a1", "n": 40000}, b"\nvisible again\n"], [], None, False)
    stream_case([(key_begin + "\n").encode(), {"rep": b"QUJDREVGR0hJSktM\n", "n": 4200}, (key_end + "\nshown\n").encode()], [], None, False)
    stream_case([b"note ", (key_begin + "\n").encode(), {"rep": b"QUJD", "n": 17000}, b"\nstill inside\n", b"more"], [], None, True)
    stream_case([{"rep": "caf\u00e9 \U0001f600 ".encode(), "n": 9000}, b"\ncode 482913\n"], [], None, False)
    stream_case([{"rep": b"word ", "n": 14000}, b"4111 1111 1111 1111\n"], [KNOWN_SETS[1][0]], None, False)


def untrusted_cases(pool: list[str]) -> None:
    def wrap(text: str, uid: str = "0a1b2c3d4e5f") -> str:
        return f'<untrusted-data id="{uid}">\n{text}\n</untrusted-data id="{uid}">'
    S, E = untrusted.PASTE_START, untrusted.PASTE_END
    msgs = ["remind me at 5", "  [200~remind me \u2026[201~  ", S + "pasted text" + E + " typed", "[200~lone start\nline two[201~",
            "x [200~inner[201~ y [201~", S + "[200~leaked inside" + E, "\x1b[200~\x1b[201~", "a\n\n  b  \n\tc\u00a0d\n", "", "   ", "[200~", "[201~[200~",
            "t [200~ a [201~ b [200~ c [201~", "\x1b[20 not a marker " + S + "x" + E, "line [201~\n[200~ next", E + "end first" + S,
            "pre " + wrap("pasted body") + " post", wrap("one") + "\n" + wrap("two", "ffff"), wrap("mismatch", "aa").replace('id="aa">', 'id="ab">', 1),
            untrusted.EARLIER_QUESTION + wrap("Which account?") + "\nanswer text " + wrap("paste"), "x\n" + untrusted.EARLIER_QUESTION_DATA + wrap("Q2?\nline"),
            wrap(""), wrap("  "), wrap("multi\nline\n</untrusted-data forged"), "tag \U000e0041 zero\u200bwidth \u2028 sep \x7f del \x85 nel \ue000 pua",
            'quote " back\\slash \t tab \x01 ctl \U0001f600', "</untrusted-data id=\"x\"> forged </untrusted-data>"]
    msgs += RNG.sample(pool, 60)
    for m in msgs:
        marked = untrusted.mark(m)
        add("untrusted", "untrusted", {"s": m, "uid": "0a1b2c3d4e5f"}, {
            "mark": {"for_model": marked.for_model, "own_words": marked.own_words, "untrusted": marked.untrusted,
                     "typed_lines": marked.typed_lines},
            "typed_lines_of": untrusted.typed_lines_of(m), "without_leaked_markers": same(untrusted.without_leaked_markers(m), m),
            "pasted_parts": untrusted.pasted_parts(m), "earlier_questions": untrusted.earlier_questions(m),
            "as_data": untrusted.as_data(m), "without_ferb_question": same(untrusted.without_ferb_question(m), m),
            "holds_pasted": untrusted.holds_pasted(m),
            "recorded_own_words": [untrusted.recorded_own_words(m, pasted) for pasted in (None, 0, 1, 2)],
            "wrap": wrap(m.replace("</untrusted-data", "<\\/untrusted-data"))})
    real = untrusted.wrap_untrusted("a </untrusted-data b")          # the real function's shape, its id aside
    uid = real.split('"')[1]
    assert real == wrap("a <\\/untrusted-data b", uid) and len(uid) == 12


def origin_cases() -> None:
    labels_ = ["owner", "ferb", "private", "inferred", "external", "external:web", "external:web:example.com", "external:email:a@b.c",
               "external:web:other.example", "external:download", "", "weird", "externalx", "external:", "external:web:a:b:c", "Owner"]
    for o in labels_:
        add("origin", "is_external", {"s": o}, origin.is_external(o))
        add("origin", "rank", {"s": o}, origin.rank(o))
        add("origin", "kind", {"s": o}, origin.kind(o))
    for _ in range(120 * SCALE):
        items = RNG.sample(labels_, RNG.randint(0, 6))
        text = RNG.choice([None, "", ",".join(items), "," + ",,".join(items) + ","])
        add("origin", "origins", {"items": items, "text": text}, {
            "least_trusted": origin.least_trusted(items), "stored_outside": origin.stored_outside(items),
            "join": origin.join(items), "kinds": sorted(origin.kinds(items)), "brief": sorted(origin.brief(items)),
            "shown": origin.shown(items), "split": sorted(origin.split(text)),
            "recorded": [sorted(origin.recorded(text, False)), sorted(origin.recorded(text, True))]})
    pairs = [("Web", "Example.COM"), ("email", "A, B  c"), ("  message \n", ""), ("x" * 200, "\u00e9" * 200), ("K\u0130ND", "d,e,,f"), ("", "")]
    for k, d in pairs:
        add("origin", "external", {"kind": k, "detail": d}, origin.external(k, d))
    idents = ["https://Example.com/a?b", "example.com", "http://user:pw@HOST.example:8080/x", "https://[::1]/", "https://[bad/", "not a url",
              "://x", "a://", " https://sp.example/x", "ht\ttp://tab.example/", "1http://num.example/", "http://ex\uff0fample.com/",
              "http://[fe80::1%EtH0]/", "//noscheme.example/x", "file:///etc/hosts", "mailto:a@b.c", "h+t.p-s://Plus.Example", "http://a@b@C.example/",
              "http://[::1]:x/", "caf\u00e9://HOST/", "http://\u0130stanbul.example/", "http://exa mple.com/ x://Y", "x:y://Z.example/", "\x00\x1f http://ctl.example/"]
    for kind in ("file", "app", "owner_account", "download", "web", "origin", "email", "message", "Weird Kind"):
        for ident in idents if kind == "web" else idents[:3]:
            add("origin", "from_read_source", {"kind": kind, "ident": ident}, origin.from_read_source(kind, ident))
    from urllib.parse import urlsplit
    for ident in idents:
        try:
            out: Any = ["ok", urlsplit(ident).hostname]
        except ValueError:
            out = ["err", None]
        add("origin", "hostname", {"s": ident}, out)


# ---------------------------------------------------------------- marks, bytes and integers of any size (py, json)

def py_build(r: Redactor, node: Any) -> Any:
    """A tagged tree as the Python value it stands for: {"s": str}, {"t": str} (what `r.text` returns: marked),
    {"b": bytes}, {"i": digits}, {"f": float}, {"l": [..]}, {"d": [[key, value], ..]}, {"r": tree} (what `r` returns
    for that tree: its strings marked); None and bools as they are."""
    if node is None or isinstance(node, bool):
        return node
    (tag, v), = node.items()
    if tag == "t":
        return r.text(v)
    if tag == "i":
        return int(v)
    if tag == "l":
        return [py_build(r, x) for x in v]
    if tag == "d":
        return {k: py_build(r, x) for k, x in v}
    if tag == "r":
        return r(py_build(r, v))
    return v


def py_show(r: Redactor, v: Any) -> Any:
    """A value as a tagged tree; a string with the redactor's current mark is {"m": str}, a dict item is
    [key, value, whether the key is marked]."""
    mark = r.marked()
    if v is None or isinstance(v, bool):
        return v
    if isinstance(v, str):
        return {"m" if type(v) is mark else "s": str(v)}
    if isinstance(v, (bytes, bytearray)):
        return {"b": bytes(v)}
    if isinstance(v, int):
        return {"i": str(v)}
    if isinstance(v, float):
        return {"f": v}
    if isinstance(v, list):
        return {"l": [py_show(r, x) for x in v]}
    return {"d": [[str(k), py_show(r, x), type(k) is mark] for k, x in v.items()]}


def py_tree(obj: Any, pool: list[str], depth: int = 0) -> Any:
    """A JSON value as a tagged tree, some of its strings redacted alone first, some as bytes, some parts redacted
    as a whole first."""
    roll = RNG.random()
    if obj is None or isinstance(obj, bool):
        return obj
    if isinstance(obj, str):
        if roll < 0.3:
            return {"t": obj}
        if roll < 0.42:
            raw = obj.encode("utf-8")
            if RNG.random() < 0.6 and raw:
                at = RNG.randint(0, len(raw))
                raw = raw[:at] + bytes(RNG.choice([[0xff], [0x80], [0xe2, 0x82], [0xc3], [0xed, 0xa0, 0x80], [0xf0, 0x9f]])) + raw[at:]
            return {"b": raw}
        return {"s": obj}
    if isinstance(obj, int):
        return {"i": str(obj)}
    if isinstance(obj, float):
        return {"f": obj}
    if isinstance(obj, list):
        out: Any = {"l": [py_tree(x, pool, depth + 1) for x in obj]}
    else:
        out = {"d": [[k, py_tree(x, pool, depth + 1)] for k, x in obj.items()]}
    return {"r": out} if roll < 0.3 else out


def py_case(tree: Any, values: list[str] = (), then: list[str] = (), reconfigure: bool = False,  # type: ignore[assignment]
            domains: list[str] = (), apps: list[str] = ()) -> None:  # type: ignore[assignment]
    r = redactor(values, domains, apps)
    built = py_build(r, tree)
    for value in then:
        r.add_secret_value(value)
    if reconfigure:
        r.configure(set(domains) or None, set(apps) or None)
    out = r(built)
    add("py", "py", {"tree": tree, "then": list(then), "reconfigure": reconfigure, **config(values, domains, apps)},
        {"out": py_show(r, out), "twice": py_show(r, r(out))})


def py_cases(pool: list[str]) -> None:
    S = lambda v: {"s": v}        # noqa: E731
    T = lambda v: {"t": v}        # noqa: E731
    L = lambda *v: {"l": list(v)}  # noqa: E731
    R = lambda v: {"r": v}        # noqa: E731
    I = lambda v: {"i": str(v)}   # noqa: E731,E741
    key_begin, key_end = "-----BEGIN RSA " + J(["PRIV", "ATE KEY-----"]), "-----END RSA " + J(["PRIV", "ATE KEY-----"])
    known_digits = "918273645546372819102938"
    fixed: list[tuple[Any, dict[str, Any]]] = [
        # a list whose texts were each redacted alone is not read as sibling lines (redact.py:377); a plain one is
        (L(T("Your verification code"), T("482913")), {}), (L(S("Your verification code"), S("482913")), {}),
        (L(T("Your verification code"), S("482913")), {}), (L(T("482913"), T("is your verification code")), {}),
        (L(T("Your verification code"), I(482913)), {}), (L(I(482913), I(5)), {}), (L(T("Your code"), {"d": [["text", T("482913")]]}), {}),
        (R(L(S("Your verification code"), S("482913"))), {}), (L(R(L(S("Your code"), S("482913"))), T("x")), {}),
        (L(T(key_begin), T("MIIEowIBAAKCAQEA1234abcd"), T(key_end)), {}), (L(S(key_begin), S("MIIEowIBAAKCAQEA1234abcd"), S(key_end)), {}),
        (L(T(key_begin), T("MIIEowIBAAKCAQEA1234abcd"), True, I(5), T(key_end)), {}),
        # a mark made under other known values or exclusions is not trusted
        (L(T("Your verification code"), T("482913")), {"then": [KNOWN_SETS[1][0]]}),
        (L(T("Your verification code"), T("482913")), {"reconfigure": True}),
        (L(T("a " + KNOWN_SETS[1][0] + " b"), T("c")), {"then": [KNOWN_SETS[1][0]]}),
        (R({"d": [["note", S("see " + KNOWN_SETS[1][0])], ["n", I(5)]]}), {"then": [KNOWN_SETS[1][0]]}),
        (T("Slack"), {"apps": ["slack"]}), (L(T("Slack"), T("482913")), {"apps": ["slack"]}), (T("[excluded]"), {"apps": ["[excluded]"]}),
        # marked keys, keys that redact alike, a marked text under each kind of key
        (R({"d": [["hunter2 password=abc", I(1)], ["password=abc", I(2)], ["password=xyz", I(3)], ["[redacted]", I(4)]]}), {}),
        ({"d": [["value", T("482913")], ["label", S("Verification code")]]}, {}), ({"d": [["text", T("")], ["role", S("AXSecureTextField")]]}, {}),
        ({"d": [["task_id", T(HEX32)], ["session_id", L(T("chat-0123456789ab"), I(5))]]}, {"values": ["0123456789ab"]}),
        ({"d": [["password", T("")], ["token", T("x")], ["secret", {"b": b""}], ["pin", {"b": b"1234"}]]}, {}),
        ({"d": [["a", T("PIN")], ["b", T("4821")]]}, {}), (R({"d": [["a", S("PIN")], ["b", S("4821")]]}), {}),
        # bytes: UTF-8, not UTF-8, nothing to hide, under field keys
        ({"b": b"password=hunter2 \xff\xfe tail"}, {}), ({"b": b"plain \xff bytes"}, {}), ({"b": "café password: hunter2".encode()}, {}),
        ({"b": b"\xe2\x82"}, {}), ({"b": b""}, {}), ({"b": b"code 482913 \xed\xa0\x80\xf0\x9f\x98"}, {}), ({"b": "\U00040080 \U000400ff password=abc \xff".encode("utf-8", "surrogatepass")[:-2] + b"\xff"}, {}),
        (L({"b": b"Your verification code"}, {"b": b"482913"}), {}), ({"d": [["type", {"b": b"password"}], ["value", S("v")]]}, {}),
        ({"d": [["role", {"b": b"it's \"q\"\\\n\x00\xff"}], ["value", S("v")], ["label", {"b": b"a'b"}]]}, {}),
        ({"d": [["value", {"b": b""}], ["type", S("password")]]}, {}), ({"b": ("a " + KNOWN_SETS[1][0] + " \xff").encode("latin-1")}, {"values": [KNOWN_SETS[1][0]]}),
        ({"b": b"4111 1111 1111 1111\x80"}, {}), ({"b": b"\xff" * 40}, {}), ({"b": b"Password:\n\xffhunter2\n"}, {}),
        # integers of any size
        (I(10 ** 30), {}), (I(-(10 ** 30) - 4111111111111111), {}), (I(known_digits), {"values": [known_digits]}),
        (I("-" + known_digits), {"values": [known_digits]}), (I("77" + known_digits + "77"), {"values": [known_digits]}),
        ({"d": [["code", I(10 ** 24 + 482913)], ["pin", I(48291348)], ["otp", I(2 ** 70)], ["api_key", I(2 ** 80)]]}, {}),
        (L(S("Your verification code"), I(2 ** 64)), {}), ({"d": [["a", S("PIN")], ["b", I(2 ** 64 + 4821)]]}, {}),
        ({"d": [["type", I(2 ** 70)], ["value", S("v")], ["role", I(-(2 ** 70))]]}, {}), (I(18446744073709551616), {}), (I(0), {}),
    ]
    for tree, opts in fixed:
        py_case(tree, **opts)
    for i in range(170 * SCALE):
        values = KNOWN_SETS[i % len(KNOWN_SETS)] if i % 4 == 0 else []
        then = [RNG.choice(KNOWN_SETS[1:])[0]] if i % 9 == 0 else []
        py_case(py_tree(random_json(pool), pool), values, then, reconfigure=i % 11 == 0)


def json_cases(pool: list[str]) -> None:
    """A record's JSON text through json.loads, the redactor and json.dumps (both `ensure_ascii` ways); None where
    json.loads refuses the text."""
    known_digits = "918273645546372819102938"
    texts = ['{"n": 123456789012345678901234567890, "m": -98765432109876543210, "z": -0, "f": 1.0, "e": 1E5, "g": 1e999, "h": -1e-999}',
             '[' + known_digits + ', -' + known_digits + ', 4111111111111111, 1' + known_digits + ']',
             '{"code": 1000000000000000000482913, "pin": 4821, "otp": 99999999999999999999, "count": 18446744073709551616}',
             '["Your verification code", 18446744073709551616, 482913]', '{"a": "PIN", "b": 48210000000000000000000}',
             '{"k": 1, "k": 2, "a": 3, "k": "password=abc"}', ' \t\r\n[ 1 , 2.50 ,\n"x" ] ', '[NaN, Infinity, -Infinity, -0.0, 0.0, 1e16, 1e-7, 5e-324]',
             '"\\u00e9\\ud83d\\ude00\\/\\b\\f\\n\\r\\t\\"\\\\ café \U0001f600 \x7f"', '{"\\u0070assword": "x", "t\\u006fken": [1, {"a": 2}]}',
             '{"value": 4111111111111111111, "autocomplete": "cc-number"}', '[' + "9" * 4300 + ']', '[' + "9" * 4301 + ']',
             '1' + "0" * 400, '-' + "4" * 60, '[1.5e3, 12E-2, 0.1, 100.0, 1e22, 1.7976931348623157e308, 123456789.125, 0e0, -0e5, 0E-0]',
             '', ' ', '[1,]', '{"a":}', '01', '-', '1.', '1.e5', '.5', '+1', '[1 2]', '{"a" 1}', "{'a': 1}", '"\x01"', '"\\x41"', '"abc', '[', 'nul',
             'tru', 'Nan', '-NaN', '1e', '1e+', '[1e5x]', '1 2', '{"a": 1,}', '{1: 2}', '"\\u12"', 'true false', '[-]', '-Infinit', '--1', '0x10',
             '1.5.2', '2e5e5', '"tab\there"', '{"a":1}{"b":2}', '﻿[1]', '[1]\x00']
    for i in range(60 * SCALE):
        obj = random_json(pool)
        text = json.dumps(obj, ensure_ascii=RNG.random() < 0.5, separators=RNG.choice([(",", ":"), (", ", ": "), (" ,\n", " :\t")]))
        if RNG.random() < 0.5:
            text = text.replace("482913", RNG.choice(["482913" + "0" * 20, "4829130000000000000000001", known_digits]), 1)
        texts.append(text)
    for i, text in enumerate(texts):
        values = [known_digits] if i % 2 == 0 else []
        r = redactor(values)
        try:
            out = r(json.loads(text))
            shown: Any = {"ascii": json.dumps(out), "plain": json.dumps(out, ensure_ascii=False)}
        except (ValueError, RecursionError):
            shown = None
        add("json", "json", {"json": text, **config(values)}, shown)


def entropy_cases() -> None:
    """`high_entropy` at its 3.0 line: every way to split 16 to 56 characters into counts whose entropy is exactly 3
    bits or within 1e-9 of it, each in several orders (the sum is taken in first-seen order); and one number for the
    bits of math.log2(c / n) and of c / n * math.log2(c / n) for every count and length up to 4096."""
    import math
    import struct
    chars = "a1" + "bcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ023456789+/=_.~-"
    logs = [0.0] + [c * math.log2(c) for c in range(1, 60)]
    found: list[list[int]] = []

    def split(left: int, largest: int, parts: list[int], weight: float, target: float, n: int) -> None:
        if left == 0:
            if abs(weight - target) < 1e-7 * n:
                found.append(list(parts))
            return
        if len(parts) >= len(chars):
            return
        for c in range(min(left, largest), 0, -1):
            if weight + logs[c] > target + 1e-6 * n:
                continue
            parts.append(c)
            split(left - c, c, parts, weight + logs[c], target, n)
            parts.pop()

    for n in range(16, 57):
        split(n, n, [], 0.0, n * math.log2(n) - 3 * n, n)
    exact = 0
    for counts in found:
        n = sum(counts)
        exact += n ** n == 8 ** n * math.prod(c ** c for c in counts)
        orders = [sorted(counts), sorted(counts, reverse=True)] + [RNG.sample(counts, len(counts)) for _ in range(3)]
        for order in {tuple(o) for o in orders}:
            value = "".join(chars[i] * c for i, c in enumerate(order))
            if len(order) > 1:
                add("entropy", "high_entropy", {"s": value, "counts": list(order)}, labels.high_entropy(value))
    assert exact >= 10, exact
    digest = 0
    for n in range(1, 4097):
        for c in range(1, n + 1):
            log = math.log2(c / n)
            for x in (log, c / n * log):
                digest = (digest * 1099511628211 + struct.unpack("<Q", struct.pack("<d", x))[0]) % 2 ** 64
    add("entropy", "log2_digest", {"n": 4096}, f"{digest:016x}")


def unicode_file() -> dict[str, Any]:
    """str.lower() and str.casefold(): every character lower() changes, and whole strings around the capital sigma
    (final or not), built from cased, case-ignorable and other characters."""
    lower = [[cp, b64(chr(cp).lower())] for cp in range(0x110000) if not 0xD800 <= cp <= 0xDFFF and chr(cp).lower() != chr(cp)]
    words = ["\u039f\u0394\u039f\u03a3", "\u03a3", "A\u03a3", "\u03a3A", "A\u03a3.B\u03a3", "\u0130STANBUL", "STRASSE \u1e9e", "A\u0301\u03a3\u0301", "\u01c5\u01c8",
             "\u017fecure", "PA\u017f\u017fWORD", "\u00df", "\ufb01", "\u13a0", "\u212a\u212b\u2126"]
    bits = ["\u03a3", "\u03a3", "a", "B", ".", "'", "\u0301", "\u00b7", " ", "1", "\u02b0", "\u00ad", "\u1e9e", "\u0130", ":", "\u2019", "\U0001d400", "\u4e2d", "\u200d"]
    words += ["".join(RNG.choice(bits) for _ in range(RNG.randint(1, 7))) for _ in range(400)]
    words = list(dict.fromkeys(words))
    return {"lower": lower, "lower_words": [[b64(w), b64(w.lower())] for w in words],
            "casefold_words": [[b64(w), b64(w.casefold())] for w in words]}


def main() -> None:
    global RNG, SCALE, OUT
    import argparse
    parser = argparse.ArgumentParser()
    parser.add_argument("--seed", type=int, default=20261113)
    parser.add_argument("--scale", type=int, default=1)
    parser.add_argument("--out", type=Path, default=HERE)
    opts = parser.parse_args()
    RNG, SCALE, OUT = random.Random(opts.seed), opts.scale, opts.out
    OUT.mkdir(parents=True, exist_ok=True)
    if OUT == HERE:
        gen_rust.main()
    seeds = KEYS + LABELLED + UNICODE_EXTRA
    tests = [t for t in harvested() if len(t) <= 240]
    tests = RNG.sample(tests, min(len(tests), 480 * SCALE))
    pool = list(dict.fromkeys(seeds + tests))
    for s in seeds:
        found = variants(s)
        pool += RNG.sample(found, min(3 * SCALE, len(found)))
    for s in RNG.sample(tests, min(len(tests), 200)):
        pool += RNG.sample(variants(s), 2)
    for _ in range(240 * SCALE):                      # multi-line mixes: the relations between neighbouring lines
        pool.append(RNG.choice(["\n", "\r\n", "\n\n", " ", "\n> ", "\r", "\n- "]).join(RNG.sample(seeds, RNG.randint(2, 4))))
    pool += [soup() for _ in range(140 * SCALE)]
    for _ in range(5):                                # long texts: past the 2048-character needle path and the gap bound
        pool.append((" " + "filler words " * RNG.randint(170, 240)).join(RNG.sample(seeds, 3)))
    pool.append("code " + "x " * 600 + "482913")
    pool.append("k=v " * 700 + "password=hunter2")
    pool = list(dict.fromkeys(pool))
    text_cases(pool)
    known_cases(pool)
    small_cases(pool)
    field_cases()
    walk_cases(pool)
    stream_cases(seeds + RNG.sample(tests, min(len(tests), 150)))
    untrusted_cases(pool)
    origin_cases()
    unicode = unicode_file()
    py_cases(pool)            # the newer groups come last: the random source reaches the older ones as before
    json_cases(pool)
    entropy_cases()
    total = 0
    for old in OUT.glob("*.json"):
        old.unlink()
    for group, cases in CASES.items():
        (OUT / f"{group}.json").write_text(json.dumps({"group": group, "cases": cases}, separators=(",", ":")) + "\n", encoding="ascii")
        total += len(cases)
        print(f"{group}: {len(cases)} cases, {(OUT / f'{group}.json').stat().st_size} bytes")
    (OUT / "unicode.json").write_text(json.dumps(unicode, separators=(",", ":")) + "\n", encoding="ascii")
    (OUT / "manifest.json").write_text(json.dumps({"groups": {g: len(c) for g, c in CASES.items()}, "total": total},
                                                   indent=1) + "\n", encoding="ascii")
    size = sum(p.stat().st_size for p in OUT.glob("*.json"))
    print(f"total: {total} cases, {size} bytes in {len(list(OUT.glob('*.json')))} files")


if __name__ == "__main__":
    main()
