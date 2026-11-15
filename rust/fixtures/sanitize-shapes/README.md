# sanitize-shapes: the secret-shape and personal-identifier corpus (NOT Python-recorded)

`corpus.json` is not a recording of the Python build. It measures the Rust redactor (`ferb-sanitize`) against the
widely used secret shapes, the personal identifiers that work like credentials, and the ordinary text that must
stay readable:

- `positives`: one invented value per shape, in the syntax it usually appears in, with the public lists that know
  the shape (gitleaks' default rules, trufflehog's detectors, GitHub secret scanning's supported patterns,
  detect-secrets' plugins; for identifiers, ids starting `pii-`: Microsoft Presidio's recognisers, Google Cloud
  Sensitive Data Protection's infoTypes, AWS Macie's managed data identifiers, Microsoft Purview's sensitive
  information types). Each must come out with every listed value gone. A positive with a `limit` is a declared
  gap: the test asserts it still leaks, so the list of limits stays true.
- `negatives`: ordinary text (hashes, ids, code, docs, config, phone numbers, timestamps, tracking numbers, contact
  data) that must come out unchanged. A negative with `known_fp` is a false positive of a rule ported from Python,
  kept as it is and declared.

`make.py` (with `pii_make.py`, the identifiers' check digits written apart from the Rust ones) writes `corpus.json`
from a seeded random source (`python3 rust/fixtures/sanitize-shapes/make.py`; the same run writes the same file).
Every value is invented: random secret bodies fail every vendor checksum, the fixed parts are the prefixes vendors
publish, and identifier numbers are random digits made check-digit-valid (whose number a random valid one would be
is nobody's business: the corpus never names anyone). The values are written as they are, not encoded: they are
key-shaped on purpose, so the repo's commit gate (`secrets` check) needs allow entries for `corpus.json` and
`make.py`.

`rust/crates/ferb-sanitize/tests/shapes.rs` runs the real `Redactor` over the corpus (and the stream in small
chunks). `FERB_SHAPES_REPORT=<file> cargo test -p ferb-sanitize --test shapes report` writes the coverage table.

The Python-recorded fixtures are in `rust/fixtures/sanitize` and are unchanged by this corpus.
