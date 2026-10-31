# FERB report relay

A Cloudflare Worker that receives a failure report or diagnostic file from FERB.app's **Send** and turns it into
exactly one issue in the private repo `Json604/ferb-reports`: the issue holds a short summary (what failed, the errors
and questions, FERB's version, the report id) and links to the full cleaned report, saved as one file in that repo
under `reports/YYYY/MM/<report id>-<random>.txt`. The person sending needs no GitHub account, browser or mail app. The
Worker holds a fine-grained GitHub token that can only read and write that repo's issues and contents.

- `worker.mjs`: the Worker (plain JavaScript module, no build step, no dependencies).
- `wrangler.toml`: its name (`ferb-reports`), the repo (`GITHUB_REPO`) and the per-address rate limit.
- `test.mjs`: tests with a fake GitHub; run `node relay/test.mjs` from the repo root.

## How one Send works

The Workers free plan allows 10 ms of CPU per request (waiting on GitHub doesn't count), so the Worker never parses
or re-encodes a report. One Send is two requests from FERB:

1. `POST /file?kind=failure&report_id=FERB-report-2026-11-01-t-1` with `Content-Type: application/json` and a
   `Content-Length` of at most 33,337,432 bytes (a 25 MB report in base64, plus the JSON around it). The body is
   exactly what GitHub's "create a file" call takes, built by FERB:
   `{"message": "FERB failure report …", "content": "<the report in base64>"}`. The Worker checks only the headers,
   the query and the rate limit, picks the file's path (by month, with a random part so a repeated report id never
   collides or overwrites) and streams the body untouched to GitHub's Contents API. It answers
   `{"ok": true, "ticket": "…"}`: the file's path, kind and report id, signed with a key derived from the token and
   valid for an hour. If another report's commit landed at the same moment, GitHub answers 409 and so does the relay,
   with `"retry": true`; FERB sends the file again (up to three tries).
2. `POST /report` with `{"version": 2, "ticket": "…", "title": "FERB report: …", "summary": "…",
   "app_version": "0.28.0 (1)"}` (at most 64 KB; the summary at most 30,000 characters). The Worker checks the shape
   and the ticket's signature, then files the issue: a "Full report" link to the file, then the report's kind, id and
   app version and the summary as a code block, so nothing in it renders as Markdown, links an issue or notifies
   anyone. Labels: the kind (`failure` / `diagnostic`) and the app version. It answers
   `{"ok": true, "report_id": "…"}`. A ticket files one issue; a forged, changed or expired one is refused (403).

Errors are `{"ok": false, "error": "…"}` with 400 (wrong shape or query), 403 (bad ticket), 404, 405, 409 (commit
conflict, or a ticket already used), 411 (no Content-Length), 413 (too big), 415, 429 (too many from one address),
502 (GitHub refused or was unreachable) or 503 (no token set). The relay never returns the issue's URL or number and
never logs a report's text.

A report over 25 MB is shortened by FERB before it is sent: the summary, the request, every error and question,
FERB's recorded problems and the last steps are kept, and a note at the top says what was cut. The full report always
stays on the sender's Mac.

Rate limit, per sender address, counted apart for files and for issues: at most 2 a minute in each Cloudflare location
(the `REPORT_LIMIT` binding, whose window can only be 10 or 60 seconds), plus 5 per 10 minutes in each running Worker
instance.

## Give the token access to the repo's contents (owner, once)

The token already has Issues: Read and write on `Json604/ferb-reports`. Add Contents, on that repo only:

GitHub → Settings → Developer settings → Fine-grained tokens → `ferb-reports-relay` → Edit → Repository permissions →
Contents: Read and write → Update.

Nothing else changes: the same token stays in the Worker (no `wrangler secret put` needed), and the repository access
stays only `Json604/ferb-reports`. Contents works on an empty repo too (GitHub's "create a file" call makes its first
commit); the reports land on the repo's default branch.

## Deploy (owner)

Needs Node.js (for `npx`); Wrangler 4.36.0 or later is fetched by `npx`.

```sh
cd relay
npx wrangler login                    # opens Cloudflare in the browser once
npx wrangler deploy                   # prints the address, e.g. https://ferb-reports.<your-subdomain>.workers.dev
npx wrangler secret put GITHUB_TOKEN  # paste the fine-grained token (Issues and Contents: read and write, ferb-reports only)
```

FERB's address for it is `https://ferb-reports.<your-subdomain>.workers.dev`: it goes into `ferb/daemon/reports.py`
as `RELAY_URL` (a config `[feedback] relay_url` overrides it on one Mac; an address ending in `/report`, as the first
relay used, works too).

## Test with two requests

```sh
RELAY=https://ferb-reports.<your-subdomain>.workers.dev
BODY="{\"message\":\"Relay test\",\"content\":\"$(printf 'hello from curl\n' | base64)\"}"
curl -sS -X POST "$RELAY/file?kind=diagnostic&report_id=relay-test-1" -H 'Content-Type: application/json' -d "$BODY"
# → {"ok":true,"ticket":"…"}; paste the ticket below
curl -sS -X POST "$RELAY/report" -H 'Content-Type: application/json' \
  -d '{"version":2,"ticket":"PASTE","title":"Relay test","summary":"hello from curl","app_version":"test"}'
```

Expected: `{"ok":true,"report_id":"relay-test-1"}`, a file `reports/YYYY/MM/relay-test-1-….txt` holding
"hello from curl", and one new issue "Relay test" (labels `diagnostic` and `test`) whose "Full report" link opens that
file. Close or delete that issue and the file afterwards.

After the first large report (a few MB or more), look at the Worker's CPU time in the Cloudflare dashboard
(Workers & Pages → ferb-reports → Metrics): it should stay a few milliseconds, since the file is streamed through.

## Rotate or revoke the token

- Rotate: create a new fine-grained token on GitHub (Settings → Developer settings → Personal access tokens →
  Fine-grained tokens; repository access only `Json604/ferb-reports`; permissions Issues: Read and write and
  Contents: Read and write), then `cd relay && npx wrangler secret put GITHUB_TOKEN` and paste it; then delete the
  old token on GitHub. A Send in progress at that moment fails once (its ticket was signed with the old token); Send
  again works.
- Revoke: delete the token on the same GitHub page. The relay then answers 502 until a new one is set; FERB keeps
  each report saved and says it couldn't send.
- Remove the secret from the Worker: `cd relay && npx wrangler secret delete GITHUB_TOKEN` (the relay answers 503).

## Delete the Worker

```sh
cd relay
npx wrangler delete                   # removes the ferb-reports Worker and its secret
```

FERB then says it couldn't send and keeps each report saved.
