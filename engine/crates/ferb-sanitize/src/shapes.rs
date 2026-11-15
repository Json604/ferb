//! Rust-only secret shapes: a hand-written layer applied with the rules ported from Python (formats.rs, labels.rs),
//! never instead of them. It only adds masks; every rule the Python build had stays as it was, and the parity
//! fixtures (engine/fixtures/sanitize, Python-recorded) are unchanged. The corpus that measures this layer is
//! engine/fixtures/sanitize-shapes (NOT Python-recorded) with tests/shapes.rs.
//!
//! Three parts, each a structural fact about a format, never a judgment of intent (AGENTS.md rule 4):
//!
//! - `format_hits`: named formats (they join `Redactor::kinds`, `hit` and the pasted-secret notice, GATE-501):
//!   vendor tokens whose prefix the vendor documents (a prefix table is a fact about the token's format, like the
//!   Python table's sk-/ghp_), webhook URLs whose path is the credential, an AWS secret written beside its key id,
//!   Bitcoin WIF keys, PuTTY and JWK private keys, and credential parameters in a URL's `;name=value` segments
//!   (JDBC, Azure connection strings).
//! - `labelled_spans`: values a label or a file's own structure names (masked, not named): the label vocabulary
//!   in other syntaxes (XML elements, `key="…" value="…"` pairs, `=>`, `:=`, setter calls), command-line
//!   passwords (`sshpass -p`, `docker login -p`, `curl -u user:pw`, `--password VALUE`), .netrc and .pgpass
//!   lines, a Kubernetes Secret's `data:` values, a seed phrase after its label, password hashes, and label words
//!   in other languages.
//! - `label_class`: the label vocabulary's additions (an environment or version suffix after the naming word,
//!   a glued word such as PGPASSWORD, `…_pass` / `…_pw`, other languages' password words).
//!
//! decision: GATE-596 (agent: what this layer masks by shape, its reference lists and its declared limits).

use crate::formats::{secret_param, Hit};
use crate::generated::Pat;
use crate::labels::{label_class as vocabulary, labelled_value, secret_value, Ends, LabelClass};
use crate::rx::{Rx, Span, RE_I};
use crate::unicode as u;

// ---------------------------------------------------------------- named formats: the vendor prefix table

/// A vendor format: its name, its pattern (Python `re` syntax, run by rx.rs like every other pattern), the group
/// masked, the substrings it is tried at (`anchor` bytes after the match's start; `DIGITS`: after the run of
/// digits before it), and whether its random body must hold a digit (a word-like run after a short prefix is no
/// token). The pattern is matched only where a needle stands, never searched through the text: a text of
/// megabytes costs one substring search per needle.
struct Shape {
    kind: &'static str,
    pat: Pat,
    group: usize,
    needles: &'static [&'static str],
    anchor: usize,
    digit: bool,
}

/// `Shape::anchor` for a token that starts with 8-10 digits before its needle (Telegram's bot id).
const DIGITS: usize = usize::MAX;

macro_rules! shape {
    ($kind:expr, $src:expr, $group:expr, [$($n:expr),+], $digit:expr) => {
        shape!($kind, $src, $group, [$($n),+], $digit, 0)
    };
    ($kind:expr, $src:expr, $group:expr, [$($n:expr),+], $digit:expr, $anchor:expr) => {
        Shape {
            kind: $kind,
            pat: Pat { name: $kind, src: $src, flags: 0 },
            group: $group,
            needles: &[$($n),+],
            anchor: $anchor,
            digit: $digit,
        }
    };
}

/// Sources: gitleaks' default config, trufflehog's detectors, GitHub secret scanning's supported patterns (the
/// test corpus names the list each shape came from). Lengths follow the vendors' documented formats, loosened
/// where a vendor has changed a length before.
static SHAPES: &[Shape] = &[
    shape!(
        "DigitalOcean token",
        r"(?<![A-Za-z0-9])do[opr]_v1_[a-f0-9]{64}(?![A-Za-z0-9])",
        0,
        ["dop_v1_", "doo_v1_", "dor_v1_"],
        false
    ),
    shape!(
        "Heroku key",
        r"(?<![A-Za-z0-9])HRKU-AA[0-9A-Za-z_-]{58}(?![0-9A-Za-z_-])",
        0,
        ["HRKU-AA"],
        false
    ),
    shape!(
        "Alibaba access key id",
        r"(?<![A-Za-z0-9])LTAI[A-Za-z0-9]{12,20}(?![A-Za-z0-9])",
        0,
        ["LTAI"],
        true
    ),
    shape!(
        "GitLab token",
        r"(?<![A-Za-z0-9])gl(?:dt|rt|ptt|cbt|soat|ft|oas|imt|agent|ffct)-[0-9A-Za-z_-]{20,}(?![0-9A-Za-z_-])",
        0,
        [
            "gldt-", "glrt-", "glptt-", "glcbt-", "glsoat-", "glft-", "gloas-", "glimt-",
            "glagent-", "glffct-"
        ],
        true
    ),
    shape!(
        "GitLab token",
        r"(?<![A-Za-z0-9])GR1348941[0-9A-Za-z_-]{20}(?![0-9A-Za-z_-])",
        0,
        ["GR1348941"],
        false
    ),
    shape!(
        "Atlassian token",
        r"(?<![A-Za-z0-9])ATBB[A-Za-z0-9]{32,}(?![A-Za-z0-9])",
        0,
        ["ATBB"],
        true
    ),
    shape!(
        "Atlassian token",
        r"(?<![A-Za-z0-9])ATATT3[A-Za-z0-9_=-]{100,}(?![A-Za-z0-9_=-])",
        0,
        ["ATATT3"],
        true
    ),
    shape!(
        "PyPI token",
        r"(?<![A-Za-z0-9])pypi-AgE(?:IcHlwaS5vcmc|NdGVzdC5weXBpLm9yZw)[A-Za-z0-9_-]{50,}(?![A-Za-z0-9_-])",
        0,
        ["pypi-AgE"],
        false
    ),
    shape!(
        "RubyGems key",
        r"(?<![A-Za-z0-9])rubygems_[a-f0-9]{48}(?![A-Za-z0-9])",
        0,
        ["rubygems_"],
        false
    ),
    shape!(
        "Docker Hub token",
        r"(?<![A-Za-z0-9])dckr_(?:pat|oat)_[A-Za-z0-9_-]{27,}(?![A-Za-z0-9_-])",
        0,
        ["dckr_"],
        true
    ),
    shape!(
        "crates.io token",
        r"(?<![A-Za-z0-9])cio[A-Za-z0-9]{32}(?![A-Za-z0-9])",
        0,
        ["cio"],
        true
    ),
    shape!(
        "Hugging Face token",
        r"(?<![A-Za-z0-9])api_org_[A-Za-z]{34}(?![A-Za-z0-9])",
        0,
        ["api_org_"],
        false
    ),
    shape!(
        "Replicate token",
        r"(?<![A-Za-z0-9])r8_[A-Za-z0-9]{37}(?![A-Za-z0-9])",
        0,
        ["r8_"],
        true
    ),
    shape!(
        "Groq key",
        r"(?<![A-Za-z0-9])gsk_[A-Za-z0-9]{52}(?![A-Za-z0-9])",
        0,
        ["gsk_"],
        true
    ),
    shape!(
        "xAI key",
        r"(?<![A-Za-z0-9])xai-[A-Za-z0-9]{70,90}(?![A-Za-z0-9])",
        0,
        ["xai-"],
        true
    ),
    shape!(
        "Perplexity key",
        r"(?<![A-Za-z0-9])pplx-[A-Za-z0-9]{40,60}(?![A-Za-z0-9])",
        0,
        ["pplx-"],
        true
    ),
    shape!(
        "LangSmith key",
        r"(?<![A-Za-z0-9])lsv2_(?:pt|sk)_[a-f0-9]{32}_[a-f0-9]{10}(?![A-Za-z0-9])",
        0,
        ["lsv2_"],
        false
    ),
    shape!(
        "Pinecone key",
        r"(?<![A-Za-z0-9])pcsk_[A-Za-z0-9]{4,10}_[A-Za-z0-9]{40,}(?![A-Za-z0-9])",
        0,
        ["pcsk_"],
        true
    ),
    shape!(
        "Stripe webhook secret",
        r"(?<![A-Za-z0-9])whsec_[A-Za-z0-9+/=]{32,}(?![A-Za-z0-9+/=])",
        0,
        ["whsec_"],
        true
    ),
    shape!(
        "Braintree token",
        r"access_token\$(?:production|sandbox)\$[0-9a-z]{16}\$[0-9a-f]{32}(?![A-Za-z0-9])",
        0,
        ["access_token$"],
        false
    ),
    shape!(
        "PayPal token",
        r"(?<![A-Za-z0-9])A21AA[A-Za-z0-9_-]{50,}(?![A-Za-z0-9_-])",
        0,
        ["A21AA"],
        true
    ),
    shape!(
        "Square token",
        r"(?<![A-Za-z0-9])sq0(?:atp|csp)-[0-9A-Za-z_-]{22,60}(?![0-9A-Za-z_-])",
        0,
        ["sq0"],
        false
    ),
    shape!(
        "Square token",
        r"(?<![A-Za-z0-9])EAAA[A-Za-z0-9_-]{60}(?![A-Za-z0-9_-])",
        0,
        ["EAAA"],
        true
    ),
    shape!(
        "Facebook token",
        r"(?<![A-Za-z0-9])EAA[A-Za-z0-9]{100,}(?![A-Za-z0-9])",
        0,
        ["EAA"],
        true
    ),
    shape!(
        "Shopify token",
        r"(?<![A-Za-z0-9])shp(?:at|ss|ca|pa)_[a-fA-F0-9]{32}(?![A-Za-z0-9])",
        0,
        ["shp"],
        false
    ),
    shape!(
        "Slack token",
        r"(?<![A-Za-z0-9])xapp-[0-9]-[A-Z0-9]+-[0-9]+-[a-f0-9]+(?![A-Za-z0-9])",
        0,
        ["xapp-"],
        false
    ),
    shape!(
        "Slack token",
        r"(?<![A-Za-z0-9])xoxe(?:\.xox[bp])?-[0-9]-[A-Za-z0-9]{100,}(?![A-Za-z0-9])",
        0,
        ["xoxe"],
        false
    ),
    shape!(
        "Slack webhook",
        r"hooks\.slack\.com/(?:services|workflows|triggers)/((?:[A-Za-z0-9]+/){1,3}[A-Za-z0-9]{16,})",
        1,
        ["hooks.slack.com/"],
        false
    ),
    shape!(
        "Discord webhook",
        r"discord(?:app)?\.com/api/webhooks/([0-9]{17,20}/[A-Za-z0-9_-]{60,})",
        1,
        ["discord"],
        false
    ),
    shape!(
        "Telegram bot token",
        r"(?<![0-9A-Za-z_:])[0-9]{8,10}:(AA[A-Za-z0-9_-]{33})(?![A-Za-z0-9_-])",
        1,
        [":AA"],
        false,
        DIGITS
    ),
    shape!(
        "Mailgun key",
        r"(?<![A-Za-z0-9-])key-[0-9a-f]{32}(?![A-Za-z0-9])",
        0,
        ["key-"],
        false
    ),
    shape!(
        "Mailchimp key",
        r"(?<![A-Za-z0-9])[0-9a-f]{32}-us[0-9]{1,2}(?![A-Za-z0-9])",
        0,
        ["-us"],
        false,
        32
    ),
    shape!(
        "Supabase key",
        r"(?<![A-Za-z0-9])sb_secret_[A-Za-z0-9_-]{20,}(?![A-Za-z0-9_-])",
        0,
        ["sb_secret_"],
        false
    ),
    shape!(
        "Firebase server key",
        r"(?<![A-Za-z0-9])AAAA[A-Za-z0-9_-]{7}:APA91b[A-Za-z0-9_-]{100,}(?![A-Za-z0-9_-])",
        0,
        [":APA91b"],
        false,
        11
    ),
    shape!(
        "PlanetScale token",
        r"(?<![A-Za-z0-9])pscale_(?:pw|tkn|oauth)_[A-Za-z0-9_=-]{32,64}(?![A-Za-z0-9_=-])",
        0,
        ["pscale_"],
        false
    ),
    shape!(
        "New Relic key",
        r"(?<![A-Za-z0-9])NR(?:AK|II)-[A-Za-z0-9-]{27,32}(?![A-Za-z0-9-])",
        0,
        ["NRAK-", "NRII-"],
        true
    ),
    shape!(
        "Sentry token",
        r"(?<![A-Za-z0-9])sntrys_eyJ[A-Za-z0-9+/=_-]{60,}(?![A-Za-z0-9+/=_-])",
        0,
        ["sntrys_"],
        false
    ),
    shape!(
        "Sentry token",
        r"(?<![A-Za-z0-9])sntryu_[a-f0-9]{64}(?![A-Za-z0-9])",
        0,
        ["sntryu_"],
        false
    ),
    shape!(
        "Mapbox secret token",
        r"(?<![A-Za-z0-9])[st]k\.eyJ[A-Za-z0-9_-]{20,}\.[A-Za-z0-9_-]{20,}(?![A-Za-z0-9_-])",
        0,
        ["k.eyJ"],
        false,
        1
    ),
    shape!(
        "Linear key",
        r"(?<![A-Za-z0-9])lin_(?:api|oauth)_[A-Za-z0-9]{40}(?![A-Za-z0-9])",
        0,
        ["lin_"],
        false
    ),
    shape!(
        "Notion token",
        r"(?<![A-Za-z0-9])ntn_[A-Za-z0-9]{46}(?![A-Za-z0-9])",
        0,
        ["ntn_"],
        true
    ),
    shape!(
        "Notion token",
        r"(?<![A-Za-z0-9_])secret_[A-Za-z0-9]{43}(?![A-Za-z0-9])",
        0,
        ["secret_"],
        true
    ),
    shape!(
        "Airtable token",
        r"(?<![A-Za-z0-9])pat[A-Za-z0-9]{14}\.[a-f0-9]{64}(?![A-Za-z0-9])",
        0,
        ["pat"],
        false
    ),
    shape!(
        "Figma token",
        r"(?<![A-Za-z0-9])figd_[A-Za-z0-9_-]{40,}(?![A-Za-z0-9_-])",
        0,
        ["figd_"],
        false
    ),
    shape!(
        "Netlify token",
        r"(?<![A-Za-z0-9])nfp_[A-Za-z0-9]{36}(?![A-Za-z0-9])",
        0,
        ["nfp_"],
        true
    ),
    shape!(
        "Doppler token",
        r"(?<![A-Za-z0-9])dp\.(?:pt|st|sa|ct|scim|audit)\.[A-Za-z0-9]{40,44}(?![A-Za-z0-9])",
        0,
        ["dp."],
        false
    ),
    shape!(
        "1Password token",
        r"(?<![A-Za-z0-9])ops_eyJ[A-Za-z0-9+/=_-]{100,}(?![A-Za-z0-9+/=_-])",
        0,
        ["ops_eyJ"],
        false
    ),
    shape!(
        "1Password secret key",
        r"(?<![A-Za-z0-9])A3-[A-Z0-9]{6}-(?:[A-Z0-9]{11}|[A-Z0-9]{6}-[A-Z0-9]{5})-[A-Z0-9]{5}-[A-Z0-9]{5}-[A-Z0-9]{5}(?![A-Za-z0-9])",
        0,
        ["A3-"],
        false
    ),
    shape!(
        "Vault token",
        r"(?<![A-Za-z0-9])hv[sbr]\.[A-Za-z0-9_-]{24,}(?![A-Za-z0-9_-])",
        0,
        ["hvs.", "hvb.", "hvr."],
        true
    ),
    shape!(
        "Terraform Cloud token",
        r"(?<![A-Za-z0-9])[A-Za-z0-9]{14}\.atlasv1\.[A-Za-z0-9_=-]{60,70}(?![A-Za-z0-9_=-])",
        0,
        [".atlasv1."],
        false,
        14
    ),
    shape!(
        "age secret key",
        r"AGE-SECRET-KEY-1[QPZRY9X8GF2TVDW0S3JN54KHCE6MUA7L]{58}(?![A-Za-z0-9])",
        0,
        ["AGE-SECRET-KEY-1"],
        false
    ),
    shape!(
        "Azure client secret",
        r"(?<![A-Za-z0-9_~.-])[A-Za-z0-9_~.]{3}[0-9]Q~[A-Za-z0-9_~.-]{31,34}(?![A-Za-z0-9_~.-])",
        0,
        ["Q~"],
        false,
        4
    ),
    // An AWS secret written right after its key id (the console's CSV, `id:secret`, `id secret`): the pair is the
    // credential, and the Python table masks the secret only after an aws_secret… label.
    shape!(
        "AWS secret access key",
        r#"(?<![A-Z0-9])A(?:KIA|SIA)[0-9A-Z]{16}["']?[ \t]*[,;:|= \t][ \t]*["']?([A-Za-z0-9/+]{40})(?![A-Za-z0-9/+=])"#,
        1,
        ["AKIA", "ASIA"],
        false
    ),
];

fn shape_rx() -> &'static [Rx] {
    static RX: std::sync::OnceLock<Vec<Rx>> = std::sync::OnceLock::new();
    RX.get_or_init(|| SHAPES.iter().map(|s| Rx::new(&s.pat)).collect())
}

// ---------------------------------------------------------------- named formats: structures

static PUTTY_LINES: Pat = Pat {
    name: "SHAPES_PUTTY_LINES",
    src: r"Private-Lines:[ \t]*[0-9]+[ \t]*\r?\n((?:[A-Za-z0-9+/=]+[ \t]*(?:\r?\n|$))+)",
    flags: 0,
};
static PUTTY_MAC: Pat = Pat {
    name: "SHAPES_PUTTY_MAC",
    src: r"Private-MAC:[ \t]*([0-9a-fA-F]{16,})",
    flags: 0,
};
/// A JSON Web Key's private members (RFC 7518: RSA d, p, q, dp, dq, qi; EC and OKP d; a symmetric key's k), in a
/// text that names a key type; an escaped object (`\"d\":\"…\"`) too.
static JWK_PRIVATE: Pat = Pat {
    name: "SHAPES_JWK_PRIVATE",
    src: r#"\\?"(?:d|p|q|dp|dq|qi|k)\\?"[ \t]*:[ \t]*\\?"([A-Za-z0-9_-]{16,})"#,
    flags: 0,
};
static PUTTY_LINES_RX: Rx = Rx::new(&PUTTY_LINES);
static PUTTY_MAC_RX: Rx = Rx::new(&PUTTY_MAC);
static JWK_PRIVATE_RX: Rx = Rx::new(&JWK_PRIVATE);

/// Every pattern of this layer, for `compile_patterns` and the tests.
pub(crate) fn patterns() -> Vec<&'static Pat> {
    let mut out: Vec<&'static Pat> = SHAPES.iter().map(|s| &s.pat).collect();
    out.extend([
        &PUTTY_LINES,
        &PUTTY_MAC,
        &JWK_PRIVATE,
        &SEED_LABEL,
        &SEED_WORDS,
        &HASHES,
        &OTHER_WORDS,
    ]);
    out
}

const BASE58: &[u8] = b"123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz";

/// Bitcoin WIF private keys: base58 of 0x80 + 32 bytes (+ 0x01) + checksum, 51 characters starting 5H/5J/5K or
/// 52 starting K/L, as a whole run of letters and digits. Read by hand over the text's runs (linear).
fn wif_hits(s: &str) -> Vec<Hit> {
    let b = s.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        if !b[i].is_ascii_alphanumeric() {
            i += 1;
            continue;
        }
        let a = i;
        while i < b.len() && b[i].is_ascii_alphanumeric() {
            i += 1;
        }
        let run = &b[a..i];
        let shaped = match run.len() {
            51 => run[0] == b'5' && matches!(run[1], b'H' | b'J' | b'K'),
            52 => matches!(run[0], b'K' | b'L'),
            _ => false,
        };
        if shaped
            && run.iter().all(|c| BASE58.contains(c))
            && run.iter().any(u8::is_ascii_digit)
            && run.iter().any(u8::is_ascii_lowercase)
        {
            out.push(Hit {
                start: a,
                end: i,
                kind: "Bitcoin private key",
            });
        }
    }
    out
}

/// A URL's `;name=value` segments before its query (JDBC's `;password=`, an Azure connection string's
/// `;SharedAccessSignature=`): a credential parameter's value, up to the next `;`, `?` or `#`. The Python rules
/// read a URL's query and fragment only, and skip labels inside a URL.
fn matrix_hits(s: &str, found: &[Span]) -> Vec<Hit> {
    let mut out = Vec::new();
    for &(a, b) in found {
        let url = &s[a..b];
        let rest = url.find("://").map_or(0, |i| i + 3);
        let stop = url[rest..].find(['?', '#']).map_or(url.len(), |i| rest + i);
        let mut pos = rest;
        while let Some(i) = url[pos..stop].find(';') {
            let seg = pos + i + 1;
            let end = url[seg..stop].find(';').map_or(stop, |j| seg + j);
            if let Some(eq) = url[seg..end].find('=') {
                let (name, value) = (&url[seg..seg + eq], seg + eq + 1);
                if value < end && !name.is_empty() && secret_param(name) {
                    out.push(Hit {
                        start: a + value,
                        end: a + end,
                        kind: "URL credential",
                    });
                }
            }
            pos = end;
        }
    }
    out
}

/// The vendor table's tokens in `s`, each with its entry's index.
fn vendor_hits(s: &str) -> Vec<(usize, Hit)> {
    let mut out = Vec::new();
    let bytes = s.as_bytes();
    for (index, (shape, rx)) in SHAPES.iter().zip(shape_rx()).enumerate() {
        let mut starts: Vec<usize> = Vec::new();
        for needle in shape.needles {
            for (i, _) in s.match_indices(needle) {
                let start = if shape.anchor == DIGITS {
                    let mut a = i;
                    while a > 0 && i - a < 10 && bytes[a - 1].is_ascii_digit() {
                        a -= 1;
                    }
                    a
                } else if i >= shape.anchor && s.is_char_boundary(i - shape.anchor) {
                    i - shape.anchor
                } else {
                    continue;
                };
                starts.push(start);
            }
        }
        starts.sort_unstable();
        starts.dedup();
        let mut taken = 0;
        for start in starts {
            if start < taken {
                continue; // inside the token just found
            }
            let Some(c) = rx.match_caps(s, start) else {
                continue;
            };
            if let (Some((a, b)), Some(whole)) = (c[shape.group], c[0]) {
                if !shape.digit || s[a..b].bytes().any(|c| c.is_ascii_digit()) {
                    out.push((
                        index,
                        Hit {
                            start: a,
                            end: b,
                            kind: shape.kind,
                        },
                    ));
                    taken = whole.1;
                }
            }
        }
    }
    out
}

/// This layer's named formats in `s`. `found`: `urls(s)`.
pub(crate) fn format_hits(s: &str, found: &[Span]) -> Vec<Hit> {
    let mut out = Vec::new();
    out.extend(vendor_hits(s).into_iter().map(|(_, h)| h));
    if s.len() >= 51 {
        out.extend(wif_hits(s));
    }
    // PuTTY's own field names: no other format writes them (read without the file's first line, so a stream's
    // later lines are masked too)
    if s.contains("Private-Lines:") || s.contains("Private-MAC:") {
        for c in PUTTY_LINES_RX
            .caps_all(s)
            .into_iter()
            .chain(PUTTY_MAC_RX.caps_all(s))
        {
            if let Some((a, mut b)) = c[1] {
                while b > a && s.as_bytes()[b - 1].is_ascii_whitespace() {
                    b -= 1;
                }
                out.push(Hit {
                    start: a,
                    end: b,
                    kind: "private key",
                });
            }
        }
    }
    if s.contains("\"kty") {
        for c in JWK_PRIVATE_RX.caps_all(s) {
            if let Some((a, b)) = c[1] {
                out.push(Hit {
                    start: a,
                    end: b,
                    kind: "private key",
                });
            }
        }
    }
    if s.contains(';') {
        out.extend(matrix_hits(s, found));
    }
    out
}

// ---------------------------------------------------------------- labelled values

/// A seed phrase's label, then its link (":", "=", "is", a quote or a line break; one is required, so prose such
/// as "seed rows, runs the same steps" is no label) (BIP-39 wallets' wording).
static SEED_LABEL: Pat = Pat {
    name: "SHAPES_SEED_LABEL",
    src: r#"(?<![A-Za-z0-9])(?:(?:secret[ \t_-]+)?recovery[ \t_-]+phrase|seed(?:[ \t_-]*(?:phrase|words))?|mnemonic(?:[ \t_-]*(?:phrase|words))?|backup[ \t_-]+phrase|wallet[ \t_-]+words)(?![A-Za-z0-9])(?:["']?[ \t]*(?:[:=]|is(?![A-Za-z0-9]))[ \t]*(?:\r?\n[ \t]*)?["']?|[ \t]*\r?\n[ \t]*["']?|[ \t]*["'])"#,
    flags: RE_I,
};
/// 12 to 24 lowercase words of 3-8 letters (a BIP-39 word is 3-8 letters), spaces, commas or line breaks
/// between.
static SEED_WORDS: Pat = Pat {
    name: "SHAPES_SEED_WORDS",
    src: r"(?:[a-z]{3,8}(?:[ \t]*,?[ \t]+|[ \t]*,?[ \t]*\r?\n[ \t]*)){11,23}[a-z]{3,8}(?![A-Za-z0-9])",
    flags: 0,
};
/// Password hashes: crypt(3) forms ($1$ md5, $apr1$ Apache, $5$/$6$ sha-crypt, $2a$-$2y$ bcrypt, $y$ yescrypt),
/// argon2, Django's pbkdf2 and LDAP's {SHA}/{SSHA}.
static HASHES: Pat = Pat {
    name: "SHAPES_HASHES",
    src: concat!(
        r"(?<![A-Za-z0-9$./])(?:\$2[abxy]?\$[0-9]{2}\$[./A-Za-z0-9]{53}",
        r"|\$(?:1|apr1)\$[./A-Za-z0-9]{1,8}\$[./A-Za-z0-9]{22}",
        r"|\$5\$(?:rounds=[0-9]+\$)?[./A-Za-z0-9]{1,16}\$[./A-Za-z0-9]{43}",
        r"|\$6\$(?:rounds=[0-9]+\$)?[./A-Za-z0-9]{1,16}\$[./A-Za-z0-9]{86}",
        r"|\$y\$[./A-Za-z0-9]+\$[./A-Za-z0-9]{1,86}\$[./A-Za-z0-9]{43}",
        r"|\$argon2(?:id|i|d)\$v=[0-9]+\$m=[0-9]+,t=[0-9]+,p=[0-9]+\$[A-Za-z0-9+/]{11,}\$[A-Za-z0-9+/]{16,}",
        r"|pbkdf2_sha(?:1|256|512)\$[0-9]+\$[A-Za-z0-9./+]+\$[A-Za-z0-9./+=]{20,}",
        r"|\{S?SHA\}[A-Za-z0-9+/]{27,}={0,2})(?![./A-Za-z0-9])"
    ),
    flags: 0,
};
/// Password words the Python vocabulary lacks: German, Spanish, Portuguese, French, Hindi (Devanagari), common
/// misspellings, and "access key" in prose. Read like the Python build's secret words (`secret_value`).
static OTHER_WORDS: Pat = Pat {
    name: "SHAPES_OTHER_WORDS",
    src: "(?<![A-Za-z0-9])(?:passwort|kennwort|contrase\u{f1}a|contrasena|senha|mot[ \\t]+de[ \\t]+passe|\
          \u{92a}\u{93e}\u{938}\u{935}\u{930}\u{94d}\u{921}|pasword|passwrd|access[ \\t]+key)(?![A-Za-z0-9])",
    flags: RE_I,
};
static SEED_LABEL_RX: Rx = Rx::new(&SEED_LABEL);
static SEED_WORDS_RX: Rx = Rx::new(&SEED_WORDS);
static HASHES_RX: Rx = Rx::new(&HASHES);
static OTHER_WORDS_RX: Rx = Rx::new(&OTHER_WORDS);

/// Where `needle` (ASCII, lowercase) stands in `s`, any case: byte offsets. `lower`: `s` lowercased, read first
/// (a text without the needle costs one substring search).
fn positions(s: &str, lower: &str, needle: &str) -> Vec<usize> {
    if !lower.contains(needle) {
        return Vec::new();
    }
    let (b, n) = (s.as_bytes(), needle.as_bytes());
    let mut out = Vec::new();
    let mut i = 0;
    while i + n.len() <= b.len() {
        if b[i].eq_ignore_ascii_case(&n[0]) && b[i..i + n.len()].eq_ignore_ascii_case(n) {
            out.push(i);
            i += n.len();
        } else {
            i += 1;
        }
    }
    out
}

fn word_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

/// A value at `pos`: a quoted string closed on its line (its inside), or a bare run up to whitespace, a quote,
/// "<", ">", or one of `stops`, its trailing punctuation left out.
fn value_at(s: &str, pos: usize, stops: &str) -> Option<Span> {
    let b = s.as_bytes();
    match b.get(pos) {
        Some(&q @ (b'"' | b'\'' | b'`')) => {
            let mut i = pos + 1;
            while i < b.len() && b[i] != q && b[i] != b'\n' && b[i] != b'\r' {
                i += if b[i] == b'\\' { 2 } else { 1 };
            }
            (i < b.len() && b[i] == q && i > pos + 1).then_some((pos + 1, i))
        }
        Some(_) => {
            let mut end = s[pos..]
                .char_indices()
                .find(|&(_, c)| u::is_space(c) || "\"'`<>".contains(c) || stops.contains(c))
                .map_or(s.len(), |(i, _)| pos + i);
            while end > pos && b".,;:!?)]}".contains(&b[end - 1]) {
                end -= 1;
            }
            (end > pos).then_some((pos, end))
        }
        None => None,
    }
}

/// A value a CLI flag or `-p` takes that is a help text's placeholder, not a value: an upper-case metavariable,
/// `<…>`, `[…]`, `{…}`, `$VAR`, or a type word.
fn placeholder(v: &str) -> bool {
    let first = v.chars().next().unwrap_or(' ');
    "<[{$-=".contains(first)
        || (v.len() > 1
            && v.chars()
                .all(|c| c.is_ascii_uppercase() || c == '_' || c == '-'))
        || matches!(
            v,
            "string"
                | "str"
                | "text"
                | "int"
                | "integer"
                | "bool"
                | "boolean"
                | "value"
                | "path"
                | "file"
        )
}

/// A value the label names in a syntax the Python rules don't read. A code word names no value here: "<code>" is
/// markup, and the Python build's code rules already read codes with their words.
fn labelled(label: &str, value: &str) -> bool {
    let cls = vocabulary(label);
    cls.is_some_and(|c| c != LabelClass::Code)
        && labelled_value(cls, value)
        && value != crate::known::REDACTED
}

/// The identifier that ends at `end` (letters, digits, "_", "$"): its start.
fn ident_before(s: &str, end: usize) -> usize {
    let b = s.as_bytes();
    let mut a = end;
    while a > 0 && (word_byte(b[a - 1]) || b[a - 1] == b'$') {
        a -= 1;
    }
    a
}

fn skip_blanks(b: &[u8], mut i: usize) -> usize {
    while i < b.len() && (b[i] == b' ' || b[i] == b'\t') {
        i += 1;
    }
    i
}

fn blanks_before(b: &[u8], mut i: usize) -> usize {
    while i > 0 && (b[i - 1] == b' ' || b[i - 1] == b'\t') {
        i -= 1;
    }
    i
}

/// `<password>value</password>`: an element the label vocabulary names, its text on one line (found from its
/// closing tag, whose name is read first).
fn xml_elements(s: &str, out: &mut Vec<Span>) {
    let b = s.as_bytes();
    for (close, _) in s.match_indices("</") {
        let n0 = close + 2;
        let mut n1 = n0;
        while n1 < b.len() && (word_byte(b[n1]) || matches!(b[n1], b'.' | b':' | b'-')) {
            n1 += 1;
        }
        if n1 == n0 || !b[n0].is_ascii_alphabetic() && b[n0] != b'_' {
            continue;
        }
        let gt = skip_blanks(b, n1);
        if b.get(gt) != Some(&b'>') {
            continue;
        }
        let name = &s[n0..n1];
        let local = name.rsplit(':').next().unwrap_or(name);
        let cls = vocabulary(local);
        if !cls.is_some_and(|c| c != LabelClass::Code) {
            continue;
        }
        // the opening tag on the same line, the text between it and the closing tag holding no tag
        let line_start = s[..close].rfind(['\n', '\r']).map_or(0, |i| i + 1);
        let Some(open_end) = s[line_start..close].rfind('>').map(|i| line_start + i + 1) else {
            continue;
        };
        let Some(open) = s[line_start..open_end].rfind('<').map(|i| line_start + i) else {
            continue;
        };
        let tag = &s[open + 1..open_end - 1];
        let opened = tag.strip_prefix(name).is_some_and(|rest| {
            rest.is_empty() || rest.starts_with([' ', '\t']) && !rest.ends_with('/')
        });
        let text = &s[open_end..close];
        if opened
            && !text.is_empty()
            && text.len() <= 512
            && !text.contains('<')
            && labelled_value(cls, text)
        {
            out.push((open_end, close));
        }
    }
}

/// `key="DbPassword" value="…"` (.NET appSettings, HTML inputs, Java properties XML): the label in a key, name, id
/// or property attribute of the same tag as the value attribute.
fn key_value_attrs(s: &str, lower: &str, out: &mut Vec<Span>) {
    let b = s.as_bytes();
    for i in positions(s, lower, "value") {
        if i > 0 && (word_byte(b[i - 1]) || b[i - 1] == b'-') {
            continue;
        }
        let eq = skip_blanks(b, i + 5);
        if b.get(eq) != Some(&b'=') {
            continue;
        }
        let q = skip_blanks(b, eq + 1);
        let Some(v) = value_at(s, q, "").filter(|_| matches!(b.get(q), Some(b'"' | b'\''))) else {
            continue;
        };
        let from = i.saturating_sub(384);
        let from = (from..i).find(|&k| s.is_char_boundary(k)).unwrap_or(i);
        let Some(open) = s[from..i].rfind('<').map(|k| from + k) else {
            continue;
        };
        if s[open..i].contains(['>', '\n']) {
            continue;
        }
        // the attributes before `value`: name="…" pairs
        let tag = &s[open..i];
        let mut label = None;
        for attr in ["key", "name", "id", "property"] {
            for (k, _) in tag.match_indices(attr) {
                let at = open + k;
                if at > 0 && (word_byte(b[at - 1]) || b[at - 1] == b'-') {
                    continue;
                }
                let e = skip_blanks(b, at + attr.len());
                if b.get(e) != Some(&b'=') {
                    continue;
                }
                let vq = skip_blanks(b, e + 1);
                if let Some(l) = value_at(s, vq, "").filter(|&(_, y)| y < i) {
                    label = Some(l);
                }
            }
        }
        if let Some((x, y)) = label {
            if labelled(&s[x..y], &s[v.0..v.1]) {
                out.push(v);
            }
        }
    }
}

/// `'password' => 'x'`, `:password => "x"` (PHP, Ruby, Perl): a quoted or symbol label and a quoted value only,
/// so a JavaScript arrow function (`token => token.trim()`) or a match arm (`"secret" => {`) is not read as one.
fn arrows(s: &str, out: &mut Vec<Span>, in_url: &dyn Fn(Span) -> bool) {
    let b = s.as_bytes();
    for (i, _) in s.match_indices("=>") {
        let end = blanks_before(b, i);
        if end == 0 {
            continue;
        }
        let label = match b[end - 1] {
            q @ (b'"' | b'\'' | b'`') => {
                let Some(open) = s[..end - 1].rfind(q as char) else {
                    continue;
                };
                (open + 1, end - 1)
            }
            c if word_byte(c) => {
                let a = ident_before(s, end);
                if a == 0 || b[a - 1] != b':' || (a >= 2 && b[a - 2] == b':') {
                    continue; // a bare name is a parameter; `::` is a path
                }
                (a, end)
            }
            _ => continue,
        };
        let name = &s[label.0..label.1];
        if name.is_empty()
            || !name
                .bytes()
                .all(|c| word_byte(c) || matches!(c, b'.' | b'$' | b'-'))
        {
            continue;
        }
        let q = skip_blanks(b, i + 2);
        if !matches!(b.get(q), Some(b'"' | b'\'' | b'`')) {
            continue; // a value that is code (a match arm's block, a variable) is no literal
        }
        if let Some(v) = value_at(s, q, "") {
            if labelled(name, &s[v.0..v.1]) && !in_url(v) {
                out.push(v);
            }
        }
    }
}

/// `password := "x"` (Go, Pascal): a quoted value only, so an expression (`token := strings.TrimSpace(t)`)
/// stays.
fn walruses(s: &str, out: &mut Vec<Span>) {
    let b = s.as_bytes();
    for (i, _) in s.match_indices(":=") {
        let end = blanks_before(b, i);
        let a = ident_before(s, end);
        let q = skip_blanks(b, i + 2);
        if a == end || !matches!(b.get(q), Some(b'"' | b'\'' | b'`')) {
            continue;
        }
        if let Some(v) = value_at(s, q, "") {
            if labelled(&s[a..end], &s[v.0..v.1]) {
                out.push(v);
            }
        }
    }
}

/// `password == "hunter2"`, `token != 'abc'`: a quoted literal compared with a value the label names is that
/// value written into code (the comparison itself is no value: `not_a_value`).
fn comparisons(s: &str, out: &mut Vec<Span>) {
    let b = s.as_bytes();
    for op in ["==", "!="] {
        for (i, _) in s.match_indices(op) {
            if i > 0 && matches!(b[i - 1], b'=' | b'!' | b'<' | b'>') {
                continue; // inside "===" or "!=="; the first two characters are read
            }
            let end = blanks_before(b, i);
            let a = ident_before(s, end);
            let mut q = i + 2;
            while q < b.len() && b[q] == b'=' {
                q += 1;
            }
            let q = skip_blanks(b, q);
            if a == end || !matches!(b.get(q), Some(b'"' | b'\'' | b'`')) {
                continue;
            }
            // `typeof token === "string"` compares a type's name, not the value
            let mut path = a;
            while path > 0 && (word_byte(b[path - 1]) || b[path - 1] == b'.') {
                path -= 1;
            }
            if s[..path].trim_end().ends_with("typeof") {
                continue;
            }
            if let Some(v) = value_at(s, q, "") {
                if labelled(&s[a..end], &s[v.0..v.1]) {
                    out.push(v);
                }
            }
        }
    }
}

/// `setPassword("x")`, `set_password('x')`, `withToken("x")`: a quoted literal passed to a setter the vocabulary
/// names.
fn setters(s: &str, out: &mut Vec<Span>) {
    let b = s.as_bytes();
    for (i, _) in s.match_indices('(') {
        let q = skip_blanks(b, i + 1);
        if !matches!(b.get(q), Some(b'"' | b'\'')) {
            continue;
        }
        let end = blanks_before(b, i);
        let a = ident_before(s, end);
        let name = &s[a..end];
        let field = ["set", "with"].iter().find_map(|p| {
            let rest = name.strip_prefix(p)?;
            rest.strip_prefix('_').or_else(|| {
                rest.starts_with(|c: char| c.is_ascii_uppercase())
                    .then_some(rest)
            })
        });
        let Some(field) = field.filter(|f| !f.is_empty()) else {
            continue;
        };
        if let Some(v) = value_at(s, q, "") {
            if labelled(field, &s[v.0..v.1]) {
                out.push(v);
            }
        }
    }
}

/// `--password VALUE` (a space, not "="; "=" is the Python rule's), and the same for token, secret and key flags.
fn cli_flags(s: &str, out: &mut Vec<Span>, in_url: &dyn Fn(Span) -> bool) {
    const FLAGS: &[&str] = &[
        "password",
        "passwd",
        "pass",
        "token",
        "secret",
        "api-key",
        "apikey",
        "auth-token",
        "access-token",
        "client-secret",
        "otp",
    ];
    let b = s.as_bytes();
    for (i, _) in s.match_indices("--") {
        if i > 0 && (b[i - 1].is_ascii_alphanumeric() || b[i - 1] == b'-') {
            continue;
        }
        let rest = &s[i + 2..];
        let Some(flag) = FLAGS.iter().find(|f| {
            rest.starts_with(**f)
                && !rest[f.len()..]
                    .starts_with(|c: char| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        }) else {
            continue;
        };
        let after = i + 2 + flag.len();
        let v0 = skip_blanks(b, after);
        if v0 == after {
            continue; // "--password=" is the Python rule's; a flag alone takes nothing
        }
        if let Some(v) = value_at(s, v0, "") {
            if !placeholder(&s[v.0..v.1]) && !in_url(v) {
                out.push(v);
            }
        }
    }
}

/// `-p VALUE` / `-pVALUE` / `--password VALUE` among one command's arguments (up to the line's end, "|", ";",
/// "&").
fn command_password(s: &str, from: usize, out: &mut Vec<Span>) {
    let end = s[from..]
        .find(['\r', '\n', '|', ';', '&'])
        .map_or(s.len(), |i| from + i);
    let args = &s[from..end];
    let b = args.as_bytes();
    let mut i = 0;
    while i + 1 < b.len() {
        let word_start = i == 0 || b[i - 1].is_ascii_whitespace();
        if word_start && b[i] == b'-' && b[i + 1] == b'p' {
            let mut v = i + 2;
            if args[v..].starts_with("assword") {
                v += 7;
            }
            if v < b.len() && b[v] == b'=' {
                v += 1;
            }
            v = skip_blanks(b, v);
            if let Some((x, y)) = value_at(args, v, "") {
                if !placeholder(&args[x..y]) {
                    out.push((from + x, from + y));
                }
                i = y;
                continue;
            }
        }
        i += 1;
    }
}

/// Commands whose `-p` takes the password as the next word: sshpass, and a container registry's login.
fn commands(s: &str, lower: &str, out: &mut Vec<Span>) {
    let b = s.as_bytes();
    let bounded = |a: usize, z: usize| {
        (a == 0 || !(word_byte(b[a - 1]) || matches!(b[a - 1], b'.' | b'/' | b'-')))
            && (z >= b.len() || !(word_byte(b[z]) || b[z] == b'-'))
    };
    for i in positions(s, lower, "sshpass") {
        if bounded(i, i + 7) && s.is_char_boundary(i + 7) {
            command_password(s, i + 7, out);
        }
    }
    for i in positions(s, lower, "login") {
        if !bounded(i, i + 5) || !s.is_char_boundary(i + 5) {
            continue;
        }
        let end = blanks_before(b, i);
        if end == i {
            continue;
        }
        let a = ident_before(s, end);
        let tool = &s[a..end];
        let tool_ok = matches!(
            tool,
            "docker" | "podman" | "nerdctl" | "buildah" | "skopeo" | "oras"
        ) || (tool == "registry" && s[..blanks_before(b, a)].ends_with("helm"));
        if tool_ok && bounded(a, end) {
            command_password(s, i + 5, out);
        }
    }
}

/// curl's `-u user:password` / `--user user:password` (the part after the first ":").
fn curl_users(s: &str, lower: &str, out: &mut Vec<Span>) {
    let b = s.as_bytes();
    for i in positions(s, lower, "curl") {
        if i > 0 && (word_byte(b[i - 1]) || b[i - 1] == b'-') {
            continue;
        }
        let end = s[i..]
            .find(['\r', '\n', '|', ';', '&'])
            .map_or(s.len(), |k| i + k);
        let mut k = i + 4;
        while k < end {
            let word_start = b[k - 1].is_ascii_whitespace();
            let flag = if !word_start {
                None
            } else if s[k..end].starts_with("--user") {
                Some(k + 6)
            } else if s[k..end].starts_with("-u") {
                Some(k + 2)
            } else {
                None
            };
            if let Some(f) = flag {
                let v0 = skip_blanks(b, f);
                if let Some((x, y)) = value_at(s, v0, "") {
                    if let Some(colon) = s[x..y].find(':') {
                        let pw = (x + colon + 1, y);
                        if pw.1 > pw.0 && !placeholder(&s[pw.0..pw.1]) {
                            out.push(pw);
                        }
                    }
                    k = y.max(k + 1);
                    continue;
                }
            }
            k += 1;
        }
    }
}

/// .netrc: from each `machine HOST` (or `default`) token, its keyword/value pairs (login, password, account,
/// port); the value after `password`. Tokens, not lines, as netrc reads them; prose ("each machine needs a
/// password") breaks the pairs at once.
fn netrc_spans(s: &str, out: &mut Vec<Span>) {
    let b = s.as_bytes();
    let token_from = |mut i: usize| -> Option<Span> {
        while i < b.len() && b[i].is_ascii_whitespace() {
            i += 1;
        }
        let a = i;
        while i < b.len() && !b[i].is_ascii_whitespace() {
            i += 1;
        }
        (i > a).then_some((a, i))
    };
    for word in ["machine", "default"] {
        for (i, _) in s.match_indices(word) {
            let z = i + word.len();
            if (i > 0 && !b[i - 1].is_ascii_whitespace())
                || (z < b.len() && !b[z].is_ascii_whitespace())
            {
                continue;
            }
            let mut pos = z;
            if word == "machine" {
                match token_from(pos) {
                    Some((_, e)) => pos = e,
                    None => continue,
                }
            }
            while let Some(k) = token_from(pos) {
                if !matches!(&s[k.0..k.1], "login" | "password" | "account" | "port") {
                    break;
                }
                let Some(v) = token_from(k.1) else { break };
                if &s[k.0..k.1] == "password" {
                    out.push(v);
                }
                pos = v.1;
            }
        }
    }
}

/// .pgpass: `host:port:database:user:password`, a host with a dot, localhost, `*` or a socket folder, a port's
/// digits or `*`, whole lines only.
fn pgpass_spans(s: &str, out: &mut Vec<Span>) {
    let mut start = 0;
    for line in s.split_inclusive('\n') {
        let text = line.trim_end_matches(['\r', '\n']);
        if text.bytes().filter(|&c| c == b':').count() >= 4 {
            let fields: Vec<&str> = text.splitn(5, ':').collect();
            let (host, port, pw) = (fields[0], fields[1], fields[4]);
            let host_ok = !host.is_empty()
                && !host.contains(char::is_whitespace)
                && (host.contains('.')
                    || host == "localhost"
                    || host == "*"
                    || host.starts_with('/'));
            let port_ok = port == "*"
                || (!port.is_empty()
                    && port.len() <= 5
                    && port.bytes().all(|c| c.is_ascii_digit()));
            let rest_ok = fields[2..4]
                .iter()
                .all(|f| !f.is_empty() && !f.contains(char::is_whitespace));
            if host_ok && port_ok && rest_ok && !pw.is_empty() && !pw.contains(char::is_whitespace)
            {
                let a = start + text.len() - pw.len();
                out.push((a, a + pw.len()));
            }
        }
        start += line.len();
    }
}

fn indent(l: &str) -> usize {
    l.len() - l.trim_start_matches([' ', '\t']).len()
}

fn secret_kind_line(l: &str) -> bool {
    let t = l.trim_end();
    indent(t) == 0
        && t.strip_prefix("kind:")
            .is_some_and(|k| matches!(k.trim(), "Secret" | "\"Secret\"" | "'Secret'"))
}

/// A Kubernetes Secret's `data:` and `stringData:` values (YAML): within each document (`---`) that says
/// `kind: Secret`, every value more indented than its `data:` line; a block scalar (`|`, `>`) whole.
fn k8s_spans(s: &str, out: &mut Vec<Span>) {
    let mut lines: Vec<(usize, &str)> = Vec::new();
    let mut pos = 0;
    for line in s.split_inclusive('\n') {
        lines.push((pos, line.trim_end_matches(['\r', '\n'])));
        pos += line.len();
    }
    let mut docs: Vec<&[(usize, &str)]> = Vec::new();
    let mut from = 0;
    for (i, (_, l)) in lines.iter().enumerate() {
        if l.trim_end() == "---" {
            docs.push(&lines[from..i]);
            from = i + 1;
        }
    }
    docs.push(&lines[from..]);
    for doc in docs {
        if !doc.iter().any(|(_, l)| secret_kind_line(l)) {
            continue;
        }
        let mut i = 0;
        while i < doc.len() {
            let (_, l) = doc[i];
            let t = l.trim();
            if !(t == "data:" || t == "stringData:") {
                i += 1;
                continue;
            }
            let block = indent(l);
            i += 1;
            while i < doc.len() {
                let (at, l) = doc[i];
                if l.trim().is_empty() || l.trim_start().starts_with('#') {
                    i += 1;
                    continue;
                }
                let ind = indent(l);
                if ind <= block {
                    break;
                }
                let body = &l[ind..];
                let Some(colon) = body
                    .find(": ")
                    .or_else(|| body.ends_with(':').then(|| body.len() - 1))
                else {
                    i += 1;
                    continue;
                };
                let rest = &body[colon + 1..];
                let v = rest.trim_start();
                let vstart = at + ind + colon + 1 + (rest.len() - v.len());
                let v = v.trim_end();
                if v.is_empty() || v.starts_with('|') || v.starts_with('>') {
                    // a block scalar: the lines under it, more indented
                    i += 1;
                    while i < doc.len() && (doc[i].1.trim().is_empty() || indent(doc[i].1) > ind) {
                        let (a2, l2) = doc[i];
                        let t2 = l2.trim();
                        if !t2.is_empty() {
                            let x = a2 + indent(l2);
                            out.push((x, x + t2.len()));
                        }
                        i += 1;
                    }
                    continue;
                }
                let (mut a, mut b) = (vstart, vstart + v.len());
                let quoted = v.len() >= 2
                    && ((v.starts_with('"') && v.ends_with('"'))
                        || (v.starts_with('\'') && v.ends_with('\'')));
                if quoted {
                    a += 1;
                    b -= 1;
                }
                if b > a {
                    out.push((a, b));
                }
                i += 1;
            }
        }
    }
}

/// This layer's labelled values in `s`. `lower`: `s` lowercased; `in_url`: whether a span lies inside a URL (a
/// URL's credentials are the URL rules'). Every rule starts from a substring it needs, so a long text without
/// them costs a few substring searches.
pub(crate) fn labelled_spans(
    s: &str,
    lower: &str,
    in_url: &dyn Fn(Span) -> bool,
    ends: &mut Ends<'_>,
) -> Vec<Span> {
    let mut out: Vec<Span> = Vec::new();
    // password words of other languages, read like the Python build's secret words
    let mut words: Vec<usize> = Vec::new();
    for needle in [
        "passwort", "kennwort", "contrase", "senha", "mot", "pasword", "passwrd", "access",
    ] {
        words.extend(positions(s, lower, needle));
    }
    words.extend(s.match_indices('\u{92a}').map(|(i, _)| i));
    words.sort_unstable();
    for i in words {
        if let Some(word) = OTHER_WORDS_RX.match_at(s, i) {
            if !in_url(word) {
                if let Some(v) = secret_value(s, word, ends) {
                    out.push(v);
                }
            }
        }
    }
    if s.contains("</") {
        xml_elements(s, &mut out);
    }
    key_value_attrs(s, lower, &mut out);
    if s.contains("=>") {
        arrows(s, &mut out, in_url);
    }
    if s.contains(":=") {
        walruses(s, &mut out);
    }
    if s.contains("==") || s.contains("!=") {
        comparisons(s, &mut out);
    }
    if lower.contains("set") || lower.contains("with") {
        setters(s, &mut out);
    }
    if s.contains("--") {
        cli_flags(s, &mut out, in_url);
    }
    commands(s, lower, &mut out);
    curl_users(s, lower, &mut out);
    if s.contains("machine") || s.contains("default") {
        netrc_spans(s, &mut out);
    }
    if s.bytes().filter(|&c| c == b':').nth(3).is_some() {
        pgpass_spans(s, &mut out);
    }
    if s.contains("kind:") && s.contains("Secret") {
        k8s_spans(s, &mut out);
    }
    let mut labels: Vec<usize> = Vec::new();
    for needle in ["seed", "mnemonic", "recovery", "backup", "wallet"] {
        labels.extend(positions(s, lower, needle));
    }
    labels.sort_unstable();
    for i in labels {
        if let Some(m) = SEED_LABEL_RX.match_at(s, i) {
            if let Some(w) = SEED_WORDS_RX.match_at(s, m.1) {
                out.push(w);
            }
        }
    }
    let mut marks: Vec<usize> = s.match_indices('$').map(|(i, _)| i).collect();
    for needle in ["pbkdf2_sha", "{SHA}", "{SSHA}"] {
        marks.extend(s.match_indices(needle).map(|(i, _)| i));
    }
    marks.sort_unstable();
    let mut taken = 0;
    for i in marks {
        if i >= taken {
            if let Some(h) = HASHES_RX.match_at(s, i) {
                out.push(h);
                taken = h.1;
            }
        }
    }
    out
}

// ---------------------------------------------------------------- what a label is followed by that is no value

/// Built-in type names of the languages FERB reads (Python, TypeScript/JavaScript, Rust, Go, Java/Kotlin/C#,
/// Swift) and the secret wrappers of pydantic and secrecy: a fact about those languages, read only right after a
/// label's ":" (an annotation), never as a list of safe values anywhere else.
const TYPE_NAMES: &[&str] = &[
    "str",
    "string",
    "String",
    "int",
    "integer",
    "Integer",
    "Int",
    "bool",
    "boolean",
    "Boolean",
    "Bool",
    "float",
    "double",
    "Double",
    "Float",
    "number",
    "Number",
    "bytes",
    "bytearray",
    "char",
    "any",
    "unknown",
    "object",
    "Object",
    "u8",
    "u16",
    "u32",
    "u64",
    "u128",
    "usize",
    "i8",
    "i16",
    "i32",
    "i64",
    "i128",
    "isize",
    "f32",
    "f64",
    "rune",
    "byte",
    "SecretStr",
    "SecretBytes",
    "SecretString",
    "SecretBox",
    "CharArray",
    "Data",
];

/// The roots of an environment or configuration lookup (APIs of the languages FERB reads): a value that starts
/// with one is code that fetches the secret, not the secret.
const LOOKUP_ROOTS: &[&str] = &[
    "process.env",
    "import.meta.env",
    "os.environ",
    "os.getenv(",
    "getenv(",
    "env::var(",
    "std::env::var(",
    "ENV[",
    "ENV.fetch(",
    "System.getenv(",
    "Environment.GetEnvironmentVariable(",
    "config.",
    "settings.",
    "self.",
    "this.",
    "secrets.",
    "vars.",
    "env.",
    "keyring.get_password(",
    "Deno.env.get(",
    "Bun.env.",
];

/// Prefixes of keys the vendors publish to be public (the owner, 2026-11-15: they pass, GATE-597): masked under a
/// secret-sounding label they would hide nothing.
const PUBLISHABLE: &[&str] = &["pk_live_", "pk_test_", "rzp_live_", "rzp_test_", "pk.eyJ"];

fn all_caps_name(v: &str) -> bool {
    // API_KEY, YOUR_API_KEY_HERE: upper-case words joined by "_", each with a letter
    v.contains('_')
        && !v.starts_with('_')
        && !v.ends_with('_')
        && v.split('_').all(|w| {
            !w.is_empty()
                && w.bytes()
                    .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit())
                && w.bytes().any(|c| c.is_ascii_uppercase())
        })
}

fn generic_type(t: &str) -> bool {
    // Option<String>, Optional[str], Vec<u8>, dict[str, Any]: a name, an opening bracket, a closing one at the end
    let name_end = t
        .find(|c: char| !(c.is_ascii_alphanumeric() || matches!(c, '_' | ':' | '.')))
        .unwrap_or(t.len());
    name_end > 0
        && t.as_bytes()[0].is_ascii_alphabetic()
        && matches!(
            (t[name_end..].chars().next(), t.chars().last()),
            (Some('<'), Some('>')) | (Some('['), Some(']'))
        )
        && t[name_end + 1..t.len() - 1]
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || " _:,&'<>[]".contains(c))
}

/// The type expression at `a`: a name with `::`/`.` paths, `&`, `'` and bracketed arguments (`Option<String>`,
/// `Optional[str]`, `dict[str, Any]`), up to the first character no type holds outside its brackets.
fn type_expr_at(s: &str, a: usize) -> &str {
    let mut depth = 0i32;
    let mut end = a;
    for (i, c) in s[a..].char_indices() {
        match c {
            '<' | '[' => depth += 1,
            '>' | ']' if depth > 0 => depth -= 1,
            c if c.is_ascii_alphanumeric() || "_:.&'".contains(c) => {}
            ' ' | ',' if depth > 0 => {}
            _ => break,
        }
        end = a + i + c.len_utf8();
    }
    &s[a..end]
}

/// Whether `s[a..b]` is only HTML tags (the last one may run on past `b` with its attributes): markup a page or a
/// template put after a label, not a value.
fn markup_only(s: &str, a: usize, b: usize) -> bool {
    let bytes = s.as_bytes();
    let mut i = a;
    while i < b {
        if bytes[i] != b'<' {
            return false;
        }
        let mut j = i + 1;
        if j < bytes.len() && bytes[j] == b'/' {
            j += 1;
        }
        let name = j;
        while j < bytes.len() && bytes[j].is_ascii_alphanumeric() {
            j += 1;
        }
        if j == name || !bytes[name].is_ascii_alphabetic() {
            return false;
        }
        match bytes.get(j) {
            Some(b'>') => i = j + 1,
            // the tag's attributes follow: it runs on to its ">" (past the value's end)
            Some(c) if c.is_ascii_whitespace() || *c == b'/' => return j >= b,
            None => return j >= b,
            _ => return false,
        }
    }
    true
}

/// Whether the value a label names at `span` is, by its structure, not a secret value: a comparison operator, a
/// type annotation, a placeholder, an environment or configuration lookup, markup, or a publishable key. The
/// Python rules mask whatever follows a secret label; these are the cases where what follows is code or a
/// template, so masking it hid nothing and blanked the code a task reads (GATE-598). Anything doubtful stays
/// masked: a run of digits (a PIN of 1111), a capitalised word that is no type, a dotted value with no known root.
pub(crate) fn not_a_value(s: &str, (a, b): Span) -> bool {
    let v = &s[a..b];
    let rest = &s[b..];
    let before = s[..a].trim_end_matches([' ', '\t']);
    // `password === ""`, `token == x`: the label's "=" was the start of a comparison
    if !v.is_empty() && v.bytes().all(|c| c == b'=') {
        return true;
    }
    // a type annotation: `password: str`, `token: Option<String>`, `secret: &str`, `x: Optional[str] = None`
    if before.ends_with(':') && !before.ends_with("::") {
        let whole = type_expr_at(s, a);
        // the type is the whole value, or code follows it (a call, a union, a list): `str-ong` is no type
        let after_type = s.get(a + whole.len()..b).unwrap_or("");
        let typed =
            after_type.is_empty() || after_type.starts_with(['(', '[', ',', ')', '|', ';', '=']);
        let t = whole.strip_prefix('&').map_or(whole, |r| {
            r.trim_start_matches("mut ").trim_start_matches('\'')
        });
        if typed && (TYPE_NAMES.contains(&t) || generic_type(t)) {
            return true;
        }
    }
    // placeholders: <token>, ${API_KEY}, {{ secrets.X }}, $API_KEY, %API_KEY%, %s, {name}, xxxxxxxx, API_KEY
    let closed;
    let v = if v.starts_with('{') && !v.ends_with('}') && rest.starts_with('}') {
        closed = format!("{v}}}");
        closed.as_str()
    } else {
        v
    };
    let bytes = v.as_bytes();
    if let (Some(&first), Some(&last)) = (bytes.first(), bytes.last()) {
        if first == b'<'
            && last == b'>'
            && v.len() > 2
            && bytes[1].is_ascii_alphabetic()
            && v[1..v.len() - 1]
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || b" _.-".contains(&c))
        {
            return true;
        }
    }
    if v.starts_with("${") || v.starts_with("{{") || v.starts_with("$(") || v.starts_with("#{") {
        return true;
    }
    if let Some(name) = v.strip_prefix('$') {
        if !name.is_empty() && name.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_') {
            return true;
        }
    }
    if v.len() > 2
        && v.starts_with('%')
        && v.ends_with('%')
        && v[1..v.len() - 1]
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'_')
    {
        return true;
    }
    if matches!(v, "%s" | "%d" | "%r" | "{}" | "?")
        || (v.starts_with("%(") && v.ends_with(")s"))
        || (v.len() > 2
            && v.starts_with('{')
            && v.ends_with('}')
            && v[1..v.len() - 1]
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || c == b'_'))
    {
        return true;
    }
    let mut chars = v.chars();
    if let Some(c) = chars.next() {
        // xxxxxxxx, XXXX, ********, ••••: four or more of the placeholder letter x or of one mark. A run of
        // digits stays (a PIN of 1111 is a PIN), and so does a run of another letter ('zzz' is a test's value)
        if v.chars().count() >= 4
            && (matches!(c, 'x' | 'X') || !c.is_alphanumeric())
            && chars.all(|d| d == c)
        {
            return true;
        }
    }
    if all_caps_name(v) {
        return true;
    }
    // an environment or configuration lookup
    if LOOKUP_ROOTS.iter().any(|r| v.starts_with(r)) {
        return true;
    }
    // markup after the label: `Password: <input type="password">`, `Password:</td><td><input …>`
    if v.starts_with('<') && markup_only(s, a, b) {
        return true;
    }
    // a publishable key under a secret-sounding label
    PUBLISHABLE.iter().any(|p| v.starts_with(p))
}

// ---------------------------------------------------------------- the label vocabulary's additions

/// Words after the naming word that still name its value: an environment, a version or a slot
/// (MY_APP_DB_PASSWORD_PROD, API_TOKEN_OLD); a trailing number is cut off the word first (DB_PASSWORD2).
const ENV_QUALIFIERS: &[&str] = &[
    "prod",
    "production",
    "dev",
    "development",
    "staging",
    "stage",
    "stg",
    "test",
    "testing",
    "qa",
    "uat",
    "local",
    "live",
    "sandbox",
    "old",
    "new",
    "backup",
    "bak",
    "current",
    "curr",
    "prev",
    "previous",
    "primary",
    "secondary",
    "alt",
    "default",
    "override",
    "tmp",
    "temp",
    "v",
];
/// Glued names of a credential (environment variables and config keys that write their words together): a
/// list, not every word ending in "token" or "password" (LOCAL_PEERTOKEN, a socket option, is no secret).
const GLUED: &[&str] = &[
    "pgpassword",
    "mysqlpassword",
    "dbpassword",
    "rootpassword",
    "adminpassword",
    "userpassword",
    "keypassword",
    "storepassword",
    "keystorepassword",
    "truststorepassword",
    "smtppassword",
    "mailpassword",
    "wifipassword",
    "sshpassword",
    "sudopassword",
    "appsecret",
    "apisecret",
    "jwtsecret",
    "webhooksecret",
    "sessionsecret",
    "cookiesecret",
    "signingsecret",
    "authtoken",
    "apitoken",
    "bottoken",
    "csrftoken",
    "xsrftoken",
    "sessiontoken",
    "identitytoken",
    "bearertoken",
    "oauthtoken",
    "githubtoken",
    "gitlabtoken",
    "npmtoken",
    "slacktoken",
    "discordtoken",
    "telegramtoken",
    "sshpass",
    "dbpass",
    "dbpwd",
];
/// The words before `pass` / `pw` that make it a password's name (DB_PASS, SMTP_PASS, ADMIN_PW): elsewhere
/// "pass" is a round of work (last_pass, MERGES_PER_PASS) or a verdict.
const PASS_OWNERS: &[&str] = &[
    "db", "database", "smtp", "mail", "email", "imap", "pop", "ftp", "sftp", "redis", "mysql",
    "postgres", "pg", "mongo", "mongodb", "ldap", "admin", "root", "user", "proxy", "vnc", "rdp",
    "wifi", "wlan", "basic", "http", "auth", "api", "app", "ssh", "sudo", "login", "account",
    "keystore", "store", "key", "bind", "rabbitmq", "amqp", "kafka", "elastic", "es", "minio",
    "s3", "registry", "docker", "git", "svn", "jenkins", "sql", "oracle", "mssql",
];
/// Words that name a password themselves: other languages' words and common misspellings
/// (an identifier-shaped label: DB_PASSWORT, senha, pasword).
const OTHER_SECRET_WORDS: &[&str] = &[
    "passwort",
    "kennwort",
    "contrasena",
    "senha",
    "pasword",
    "passwrd",
];
/// The Python vocabulary's own words, as labels.rs has them (the ones read again after a suffix is cut).
const SECRET_LAST: &[&str] = &[
    "password",
    "passwords",
    "passwd",
    "pwd",
    "passphrase",
    "passcode",
    "secret",
    "token",
    "otp",
    "totp",
    "pin",
    "apikey",
];

/// The label vocabulary's additions, asked only when the Python vocabulary names nothing (`ws`: the label's
/// words after Python's qualifiers were cut; `prose`: the label holds a space).
pub(crate) fn label_class(ws: &[String], prose: bool) -> Option<LabelClass> {
    if prose || ws.is_empty() {
        return None;
    }
    let mut ws: Vec<String> = ws.to_vec();
    let mut changed = false;
    loop {
        let last = ws.len() - 1;
        let cut = ws[last]
            .trim_end_matches(|c: char| c.is_ascii_digit())
            .to_string();
        if ws.len() > 1 && ENV_QUALIFIERS.contains(&cut.as_str()) {
            ws.pop();
            changed = true;
            continue;
        }
        if !cut.is_empty() && cut != ws[last] {
            ws[last] = cut;
            changed = true;
            continue;
        }
        break;
    }
    let last = ws[ws.len() - 1].as_str();
    if ws.len() >= 2
        && matches!(last, "pass" | "pw")
        && PASS_OWNERS.contains(&ws[ws.len() - 2].as_str())
    {
        return Some(LabelClass::Password);
    }
    if SECRET_LAST.contains(&last) || OTHER_SECRET_WORDS.contains(&last) || GLUED.contains(&last) {
        return Some(LabelClass::Secret);
    }
    // what Python's vocabulary would have named once the suffix is cut (API_KEY_PROD: a key, read by entropy)
    // (its words are plain lowercase words now, so this asks once: nothing more is cut the second time)
    if changed {
        return vocabulary(&ws.join("_"));
    }
    None
}

// ---------------------------------------------------------------- the stream (stream.rs)

/// The line being read holds a seed phrase's label: its words wait for the line's end (the phrase is masked only
/// whole).
pub(crate) fn hold_line(line: &str) -> bool {
    let lower = u::lower(line);
    (lower.contains("seed")
        || lower.contains("phrase")
        || lower.contains("mnemonic")
        || lower.contains("wallet"))
        && SEED_LABEL_RX.search(&format!("{line}\n"), 0).is_some()
}

/// A line of .netrc keyword/value pairs from its `from`th word on.
fn netrc_pairs(line: &str, from: usize) -> bool {
    let toks: Vec<&str> = line.split_whitespace().collect();
    toks.len() > from
        && (toks.len() - from).is_multiple_of(2)
        && toks[from..]
            .chunks(2)
            .all(|p| matches!(p[0], "login" | "password" | "account" | "port"))
}

fn netrc_opener(line: &str) -> bool {
    match line.split_whitespace().next() {
        Some("machine") => line.split_whitespace().count() == 2 || netrc_pairs(line, 2),
        Some("default") => line.split_whitespace().count() == 1 || netrc_pairs(line, 1),
        _ => false,
    }
}

/// Where a structure whose values come on later lines starts, for the stream's context: the line index (in
/// `lines`, the released and held lines; `i` the last that isn't blank) of a seed phrase's label line, PuTTY's
/// `Private-Lines:`, a .netrc stanza's `machine` line, or a Kubernetes Secret's `kind: Secret` line. The stream
/// keeps the lines from there and holds the next line whole (stream.rs `relation`).
pub(crate) fn stream_structure(lines: &[&str], i: usize) -> Option<usize> {
    let line = lines[i];
    if line.trim_start().starts_with("Private-Lines:") {
        return Some(i);
    }
    if hold_line(line) {
        // the line ends with the label: its link is the line break after it
        let ended = format!("{line}\n");
        if let Some(m) = SEED_LABEL_RX.search(&ended, 0) {
            if ended[m.1..].trim().is_empty() {
                return Some(i);
            }
        }
    }
    if netrc_opener(line) {
        return Some(i);
    }
    if netrc_pairs(line, 0) {
        let mut j = i;
        while j > 0 {
            j -= 1;
            let above = lines[j];
            if above.trim().is_empty() {
                continue;
            }
            if netrc_opener(above) {
                return Some(j);
            }
            if !netrc_pairs(above, 0) {
                break;
            }
        }
    }
    // a Kubernetes Secret: from its `kind: Secret` line, within its document
    if lines[..=i].iter().any(|l| l.contains("kind:")) {
        for j in (i.saturating_sub(256)..=i).rev() {
            let l = lines[j].trim_end();
            if l == "---" {
                return None;
            }
            if secret_kind_line(l) {
                return Some(j);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_pattern_of_the_layer_compiles() {
        for pat in patterns() {
            assert!(Rx::new(pat).compile(), "{} does not compile", pat.name);
        }
    }

    /// Each vendor shape is matched only where its needle stands, `anchor` bytes into the token: a wrong anchor
    /// would silently never match. Every shape must find its token somewhere in the corpus.
    #[test]
    fn every_vendor_shape_finds_its_token_in_the_corpus() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../fixtures/sanitize-shapes/corpus.json"
        );
        let corpus: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(path).expect("corpus.json"))
                .expect("JSON");
        let mut found = vec![false; SHAPES.len()];
        for item in corpus["positives"].as_array().expect("positives") {
            let text = item["text"].as_str().unwrap_or_default();
            for (i, _) in vendor_hits(text) {
                found[i] = true;
            }
        }
        let missing: Vec<String> = SHAPES
            .iter()
            .zip(&found)
            .filter(|(_, f)| !**f)
            .map(|(s, _)| format!("{}: {}", s.kind, s.pat.src))
            .collect();
        assert!(missing.is_empty(), "never matched:\n{}", missing.join("\n"));
    }

    #[test]
    fn the_vocabulary_additions() {
        let class = |l: &str| vocabulary(l);
        // decision: GATE-596
        for label in [
            "DB_PASSWORD2",
            "MY_APP_DB_PASSWORD_PROD",
            "API_TOKEN_OLD",
            "PGPASSWORD",
            "SSHPASS",
            "senha",
        ] {
            assert_eq!(class(label), Some(LabelClass::Secret), "{label}");
        }
        for label in ["DB_PASS", "smtp_pass", "ADMIN_PW"] {
            assert_eq!(class(label), Some(LabelClass::Password), "{label}");
        }
        assert_eq!(class("api_key_prod"), Some(LabelClass::Secret));
        for label in [
            "last_pass",
            "MERGES_PER_PASS",
            "REFUSED_PASS",
            "LOCAL_PEERTOKEN",
            "1password",
            "bypass",
            "passport",
            "PASSWORD_MIN_LENGTH",
            "token_count",
            "max_tokens",
            "tokenizer",
            "password_field",
            "probe.pw",
        ] {
            assert_eq!(class(label), None, "{label}");
        }
        assert!(
            !labelled_value(Some(LabelClass::Password), "12"),
            "a count is no password"
        );
        assert!(labelled_value(Some(LabelClass::Password), "hunter"));
    }
}
