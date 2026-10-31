// The relay's handler with a fake GitHub (fetch) and a fake env. Run: node relay/test.mjs (no packages).
import assert from "node:assert/strict";
import {
  handle, MAX_BODY, MAX_REPORT, MAX_SUMMARY, MAX_UPLOAD, PER_WINDOW, TICKET_MS, resetLimits, sign, verify,
} from "./worker.mjs";

const env = { GITHUB_TOKEN: "test-token", GITHUB_REPO: "owner/reports" };
const NOW = 1_793_500_000_000;                      // 2026-11-01 UTC
const ORIGIN = "https://ferb-reports.example.workers.dev";

function fakeGitHub({ putStatus = 201, issueStatus = 201, fail = false } = {}) {
  const calls = [];
  const fetcher = async (url, init) => {
    // the file's body is read here as GitHub would; the Worker passed it on without reading it
    const body = typeof init.body === "string" ? init.body : await new Response(init.body).text();
    calls.push({ url, init, body, json: JSON.parse(body) });
    if (fail) throw new Error("network down");
    if (init.method === "PUT") return new Response(JSON.stringify({ content: { path: "x" } }), { status: putStatus });
    return new Response(JSON.stringify({ number: 41, html_url: "https://github.com/owner/reports/issues/41" }),
      { status: issueStatus });
  };
  return { calls, fetcher };
}

const FILE = JSON.stringify({ message: "FERB failure report r-1", content: btoa("FERB report for t-1\n") });

function upload(body = FILE, { query = "kind=failure&report_id=FERB-report-2026-11-01-t-1", type = "application/json",
  ip = "203.0.113.9", length, method = "POST", path = "/file" } = {}) {
  const headers = { "Content-Type": type, "CF-Connecting-IP": ip };
  const size = length ?? (body === null ? undefined : String(new TextEncoder().encode(body).byteLength));
  if (size !== undefined) headers["Content-Length"] = size;
  const init = { method, headers, duplex: "half" };
  if (body !== null && method !== "GET") init.body = body;
  return new Request(`${ORIGIN}${path}?${query}`, init);
}

function summary(extra = {}) {
  return { version: 2, title: "FERB report: the folder was protected", summary: "Status: failed\nErrors (1): denied",
    app_version: "0.28.0 (1)", ...extra };
}

function post(body, { ip = "203.0.113.9", method = "POST", type = "application/json" } = {}) {
  const init = { method, headers: { "Content-Type": type, "CF-Connecting-IP": ip } };
  if (method !== "GET") init.body = typeof body === "string" ? body : JSON.stringify(body);
  return new Request(`${ORIGIN}/report`, init);
}

async function ticketFor(gh, opts = {}) {
  const r = await handle(upload(FILE, opts), env, gh.fetcher, NOW);
  assert.equal(r.status, 200);
  return (await r.json()).ticket;
}

const tests = {
  async "a file is streamed to GitHub's Contents API untouched, under a path the relay picks"() {
    const gh = fakeGitHub();
    const r = await handle(upload(), env, gh.fetcher, NOW);
    assert.equal(r.status, 200);
    const out = await r.json();
    assert.deepEqual(Object.keys(out).sort(), ["ok", "ticket"]);
    assert.equal(gh.calls.length, 1);
    const [call] = gh.calls;
    assert.equal(call.init.method, "PUT");
    assert.match(call.url, /^https:\/\/api\.github\.com\/repos\/owner\/reports\/contents\/reports\/2026\/11\/FERB-report-2026-11-01-t-1-[A-Za-z0-9]{8}\.txt$/);
    assert.equal(typeof call.init.body, "object");                       // the request's stream, not a copy
    assert.equal(call.body, FILE);                                       // byte for byte
    assert.equal(call.init.headers.Authorization, "Bearer test-token");
    assert.ok(call.init.headers["User-Agent"]);
    const again = fakeGitHub();
    await handle(upload(), env, again.fetcher, NOW);
    assert.notEqual(again.calls[0].url, call.url);                       // a repeated report id never collides
  },

  async "one Send files exactly one issue: a link to the file, then the summary as a code block"() {
    const gh = fakeGitHub();
    const ticket = await ticketFor(gh);
    const r = await handle(post(summary({ ticket })), env, gh.fetcher, NOW + 1000);
    assert.equal(r.status, 200);
    assert.deepEqual(await r.json(), { ok: true, report_id: "FERB-report-2026-11-01-t-1" });   // no issue URL or number
    assert.equal(gh.calls.length, 2);
    const [put, made] = gh.calls;
    const path = put.url.split("/contents/")[1];
    assert.equal(made.url, "https://api.github.com/repos/owner/reports/issues");
    assert.equal(made.init.method, "POST");
    assert.equal(made.json.title, "FERB report: the folder was protected");
    assert.deepEqual(made.json.labels, ["failure", "0.28.0 (1)"]);
    const body = made.json.body;
    assert.ok(body.startsWith(`Full report: [${path}](https://github.com/owner/reports/blob/HEAD/${path})\n\n`));
    assert.ok(body.includes("```text\nFERB failure report FERB-report-2026-11-01-t-1 · app 0.28.0 (1)\n\n"));
    assert.ok(body.includes("Status: failed\nErrors (1): denied\n```"));
  },

  async "nothing in the summary, title or version renders: no mention, link or heading"() {
    const gh = fakeGitHub();
    const ticket = await ticketFor(gh);
    const sneaky = "@owner see #1 ```\n# Heading\n[x](https://evil.example)";
    const r = await handle(post(summary({ ticket, summary: sneaky, app_version: "@owner" })), env, gh.fetcher, NOW);
    assert.equal(r.status, 200);
    const body = gh.calls[1].json.body;
    const [linkLine, rest] = [body.slice(0, body.indexOf("\n")), body.slice(body.indexOf("\n\n") + 2)];
    assert.ok(!linkLine.includes("@") && !linkLine.includes("evil"));
    assert.ok(rest.startsWith("````text\n") && rest.endsWith("\n````"));    // fenced longer than any run inside
    assert.ok(!rest.slice(9, -5).includes("````"));
  },

  async "a file's size is judged by its Content-Length alone: over the cap, missing or empty"() {
    const gh = fakeGitHub();
    assert.equal((await handle(upload(FILE, { length: String(MAX_UPLOAD + 1) }), env, gh.fetcher, NOW)).status, 413);
    const missing = new Request(`${ORIGIN}/file?kind=failure&report_id=r-1`, {
      method: "POST", headers: { "Content-Type": "application/json" }, duplex: "half",
      body: new ReadableStream({ start(c) { c.enqueue(new TextEncoder().encode(FILE)); c.close(); } }),
    });
    assert.equal((await handle(missing, env, gh.fetcher, NOW)).status, 411);
    assert.equal((await handle(upload("", { length: "0" }), env, gh.fetcher, NOW)).status, 400);
    assert.equal(gh.calls.length, 0);
    assert.ok(MAX_UPLOAD >= Math.ceil(MAX_REPORT / 3) * 4);              // a full 25 MB report's base64 fits
  },

  async "a wrong method, path, content type or query is refused"() {
    const gh = fakeGitHub();
    assert.equal((await handle(upload(null, { method: "GET" }), env, gh.fetcher, NOW)).status, 405);
    assert.equal((await handle(upload(FILE, { method: "PUT" }), env, gh.fetcher, NOW)).status, 405);
    assert.equal((await handle(upload(FILE, { path: "/" }), env, gh.fetcher, NOW)).status, 404);
    assert.equal((await handle(upload(FILE, { type: "text/plain" }), env, gh.fetcher, NOW)).status, 415);
    for (const query of ["kind=spam&report_id=r-1", "kind=failure&report_id=../x", "kind=failure", "report_id=r-1"]) {
      assert.equal((await handle(upload(FILE, { query }), env, gh.fetcher, NOW)).status, 400, query);
    }
    assert.equal((await handle(post(summary(), { method: "PUT" }), env, gh.fetcher, NOW)).status, 405);
    assert.equal((await handle(post(summary(), { type: "text/plain" }), env, gh.fetcher, NOW)).status, 415);
    assert.equal(gh.calls.length, 0);
  },

  async "a wrong summary shape is refused with 400"() {
    const gh = fakeGitHub();
    const ticket = await sign(env, { p: "reports/x.txt", k: "failure", r: "r-1", e: NOW + 1000 });
    const bad = [
      "not json", "[]", JSON.stringify(null),
      summary({ ticket, version: 1 }), summary({ ticket, title: "" }), summary({ ticket, title: "t".repeat(201) }),
      summary({ ticket, summary: "" }), summary({ ticket, summary: "s".repeat(MAX_SUMMARY + 1) }),
      summary({ ticket, summary: 5 }), summary({ ticket, app_version: "" }),
      summary({ ticket, app_version: "v".repeat(51) }), summary({ ticket, app_version: "a\nb" }),
      summary({ ticket, extra: 1 }), summary({ ticket, body: "the old shape" }), summary(),
    ];
    for (const body of bad) {
      const r = await handle(post(body), env, gh.fetcher, NOW);
      assert.equal(r.status, 400, JSON.stringify(body).slice(0, 80));
      const out = await r.json();
      assert.equal(out.ok, false);
      assert.ok(out.error);
    }
    const huge = await handle(post(summary({ ticket, summary: "s".repeat(MAX_BODY) })), env, gh.fetcher, NOW);
    assert.equal(huge.status, 413);
    assert.equal(gh.calls.length, 0);
  },

  async "an issue needs a ticket the relay signed, unexpired, used once"() {
    const gh = fakeGitHub();
    const forged = await sign({ ...env, GITHUB_TOKEN: "other" }, { p: "reports/x.txt", k: "failure", r: "r-1",
      e: NOW + 1000 });
    const expired = await sign(env, { p: "reports/x.txt", k: "failure", r: "r-1", e: NOW - 1 });
    const good = await sign(env, { p: "reports/x.txt", k: "failure", r: "r-1", e: NOW + 1000 });
    const tampered = good.replace(/^[^.]+/, btoa(JSON.stringify({ p: "x", k: "failure", r: "r-2", e: NOW + 1000 })));
    for (const ticket of [forged, expired, tampered, "junk", "a.b.c"]) {
      assert.equal((await handle(post(summary({ ticket })), env, gh.fetcher, NOW)).status, 403, ticket);
    }
    assert.equal(gh.calls.length, 0);
    assert.equal((await handle(post(summary({ ticket: good })), env, gh.fetcher, NOW)).status, 200);
    assert.equal((await handle(post(summary({ ticket: good })), env, gh.fetcher, NOW)).status, 409);
    assert.equal(gh.calls.length, 1);
    assert.ok(await verify(env, good, NOW + 999));
    assert.equal(await verify(env, good, NOW + 1000), null);
    const fresh = await ticketFor(fakeGitHub());
    assert.ok(await verify(env, fresh, NOW + TICKET_MS - 1));
    assert.equal(await verify(env, fresh, NOW + TICKET_MS), null);
  },

  async "a commit conflict asks FERB to send the file again"() {
    const gh = fakeGitHub({ putStatus: 409 });
    const r = await handle(upload(), env, gh.fetcher, NOW);
    assert.equal(r.status, 409);
    const out = await r.json();
    assert.equal(out.ok, false);
    assert.equal(out.retry, true);
    assert.equal(out.ticket, undefined);
  },

  async "a GitHub failure is an error, not a success, and never shows the token"() {
    for (const [opts, status] of [[{ putStatus: 401 }, 502], [{ putStatus: 422 }, 502], [{ putStatus: 413 }, 413],
      [{ fail: true }, 502]]) {
      const gh = fakeGitHub(opts);
      const r = await handle(upload(), env, gh.fetcher, NOW);
      assert.equal(r.status, status, JSON.stringify(opts));
      const out = await r.json();
      assert.equal(out.ok, false);
      assert.ok(!JSON.stringify(out).includes("test-token"));
    }
    for (const [i, opts] of [{ issueStatus: 403 }, { fail: true }].entries()) {
      const ticket = await ticketFor(fakeGitHub(), { ip: `198.51.100.${20 + i}` });
      const r = await handle(post(summary({ ticket })), env, fakeGitHub(opts).fetcher, NOW);
      assert.equal(r.status, 502);
      const out = await r.json();
      assert.equal(out.ok, false);
      assert.ok(!JSON.stringify(out).includes("test-token"));
    }
  },

  async "one address is limited to a few files per window, issues counted apart"() {
    const gh = fakeGitHub();
    const ip = "198.51.100.7";
    const tickets = [];
    for (let i = 0; i < PER_WINDOW; i++) {
      const r = await handle(upload(FILE, { ip }), env, gh.fetcher, NOW);
      assert.equal(r.status, 200);
      tickets.push((await r.json()).ticket);
    }
    assert.equal((await handle(upload(FILE, { ip }), env, gh.fetcher, NOW)).status, 429);
    assert.equal((await handle(upload(FILE, { ip: "198.51.100.8" }), env, gh.fetcher, NOW)).status, 200);
    for (const ticket of tickets) {
      assert.equal((await handle(post(summary({ ticket }), { ip }), env, gh.fetcher, NOW)).status, 200);
    }
    const later = NOW + 11 * 60 * 1000;
    assert.equal((await handle(upload(FILE, { ip }), env, gh.fetcher, later)).status, 200);
  },

  async "the rate-limit binding is asked by sender address before GitHub is reached"() {
    const asked = [];
    const limited = { ...env, REPORT_LIMIT: { limit: async ({ key }) => (asked.push(key), { success: false }) } };
    const gh = fakeGitHub();
    assert.equal((await handle(upload(FILE, { ip: "192.0.2.4" }), limited, gh.fetcher, NOW)).status, 429);
    const ticket = await sign(env, { p: "reports/x.txt", k: "failure", r: "r-1", e: NOW + 1000 });
    assert.equal((await handle(post(summary({ ticket }), { ip: "192.0.2.4" }), limited, gh.fetcher, NOW)).status, 429);
    assert.deepEqual(asked, ["file:192.0.2.4", "issue:192.0.2.4"]);
    assert.equal(gh.calls.length, 0);
  },

  async "without its token the relay says it isn't set up"() {
    const gh = fakeGitHub();
    const bare = { GITHUB_REPO: "owner/reports" };
    assert.equal((await handle(upload(), bare, gh.fetcher, NOW)).status, 503);
    assert.equal((await handle(post(summary({ ticket: "x.y" })), bare, gh.fetcher, NOW)).status, 503);
    assert.equal(gh.calls.length, 0);
  },
};

let failed = 0;
for (const [name, fn] of Object.entries(tests)) {
  resetLimits();
  try {
    await fn();
    console.log(`ok - ${name}`);
  } catch (e) {
    failed++;
    console.log(`not ok - ${name}\n  ${e.stack}`);
  }
}
console.log(`${Object.keys(tests).length - failed} passed, ${failed} failed`);
process.exit(failed ? 1 : 0);
