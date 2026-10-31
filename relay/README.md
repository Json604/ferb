# FERB report relay

A Cloudflare Worker that receives a failure report or diagnostic file from FERB.app's **Send** and files it as an
issue in the private repo `Json604/ferb-reports`. The person sending needs no GitHub account, browser or mail app.
The Worker holds a fine-grained GitHub token that can only read and write that repo's issues.

- `worker.mjs`: the Worker (plain JavaScript module, no build step, no dependencies).
- `wrangler.toml`: its name (`ferb-reports`), the repo (`GITHUB_REPO`) and the per-address rate limit.
- `test.mjs`: tests with a fake GitHub; run `node relay/test.mjs` from the repo root.

What it accepts: only `POST /report` with `Content-Type: application/json` and a body of at most 512 KB:

```json
{"version": 1, "kind": "failure", "title": "FERB report: …", "body": "…the cleaned report…",
 "app_version": "0.28.0 (1)", "report_id": "FERB-report-2026-11-01-t-1"}
```

It answers `{"ok": true, "report_id": "…"}`, or `{"ok": false, "error": "…"}` with 400 (wrong shape), 404, 405,
413 (too big), 415, 429 (too many from one address), 502 (GitHub refused or was unreachable) or 503 (no token set).
It never returns the issue's URL or number and never logs a report's text. A report longer than one issue holds
(65,536 characters) continues in comments on that issue, in order. Each part is filed as a code block, so nothing in
a report renders as Markdown or notifies anyone. Labels: the kind (`failure` / `diagnostic`) and the app version.

Rate limit: at most 2 reports a minute per sender address in each Cloudflare location (the `REPORT_LIMIT` binding,
whose window can only be 10 or 60 seconds), plus 5 per 10 minutes per address in each running Worker instance.

## Deploy (owner)

Needs Node.js (for `npx`); Wrangler 4.36.0 or later is fetched by `npx`.

```sh
cd relay
npx wrangler login                    # opens Cloudflare in the browser once
npx wrangler deploy                   # prints the address, e.g. https://ferb-reports.<your-subdomain>.workers.dev
npx wrangler secret put GITHUB_TOKEN  # paste the fine-grained token (Issues: read and write on Json604/ferb-reports)
```

FERB's address for it is `https://ferb-reports.<your-subdomain>.workers.dev/report`: it goes into
`ferb/daemon/reports.py` as `RELAY_URL` (a config `[feedback] relay_url` overrides it on one Mac).

## Test with one request

```sh
curl -sS -X POST https://ferb-reports.<your-subdomain>.workers.dev/report \
  -H 'Content-Type: application/json' \
  -d '{"version":1,"kind":"diagnostic","title":"Relay test","body":"hello from curl","app_version":"test","report_id":"relay-test-1"}'
```

Expected: `{"ok":true,"report_id":"relay-test-1"}` and a new issue "Relay test" in `Json604/ferb-reports`
(labels `diagnostic` and `test`). Close or delete that issue afterwards.

## Rotate or revoke the token

- Rotate: create a new fine-grained token on GitHub (Settings → Developer settings → Personal access tokens →
  Fine-grained tokens; repository access only `Json604/ferb-reports`; permission Issues: Read and write), then
  `cd relay && npx wrangler secret put GITHUB_TOKEN` and paste it; then delete the old token on GitHub.
- Revoke: delete the token on the same GitHub page. The relay then answers 502 until a new one is set; FERB keeps
  each report saved and says it couldn't send.
- Remove the secret from the Worker: `cd relay && npx wrangler secret delete GITHUB_TOKEN` (the relay answers 503).

## Delete the Worker

```sh
cd relay
npx wrangler delete                   # removes the ferb-reports Worker and its secret
```

FERB then says it couldn't send and keeps each report saved.
