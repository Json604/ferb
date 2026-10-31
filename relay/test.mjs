// The relay's handler with a fake GitHub (fetch) and a fake env. Run: node relay/test.mjs (no packages).
import assert from "node:assert/strict";
import { handle, MAX_BODY, PART_CHARS, ISSUE_CHARS, PER_WINDOW, resetLimits, split } from "./worker.mjs";

const env = { GITHUB_TOKEN: "test-token", GITHUB_REPO: "owner/reports" };

function fakeGitHub({ issueStatus = 201, commentStatus = 201, fail = false } = {}) {
  const calls = [];
  const fetcher = async (url, init) => {
    calls.push({ url, init, body: JSON.parse(init.body) });
    if (fail) throw new Error("network down");
    if (url.endsWith("/issues")) {
      return new Response(JSON.stringify({ number: 41, html_url: "https://github.com/owner/reports/issues/41" }),
        { status: issueStatus });
    }
    return new Response("{}", { status: commentStatus });
  };
  return { calls, fetcher };
}

function report(extra = {}) {
  return { version: 1, kind: "failure", title: "FERB report: the folder was protected",
    body: "FERB report for t-1\n\n## Summary\nStatus: failed\n", app_version: "0.28.0 (1)",
    report_id: "FERB-report-2026-11-01-t-1", ...extra };
}

function post(body, { method = "POST", type = "application/json", path = "/report", ip = "203.0.113.9" } = {}) {
  const init = { method, headers: { "Content-Type": type, "CF-Connecting-IP": ip } };
  if (method !== "GET" && method !== "HEAD") init.body = typeof body === "string" ? body : JSON.stringify(body);
  return new Request(`https://ferb-reports.example.workers.dev${path}`, init);
}

const tests = {
  async "a valid report becomes one issue with its kind and version as labels"() {
    const gh = fakeGitHub();
    const r = await handle(post(report()), env, gh.fetcher);
    assert.equal(r.status, 200);
    const out = await r.json();
    assert.deepEqual(out, { ok: true, report_id: "FERB-report-2026-11-01-t-1" });   // no issue URL or number
    assert.equal(gh.calls.length, 1);
    const [call] = gh.calls;
    assert.equal(call.url, "https://api.github.com/repos/owner/reports/issues");
    assert.equal(call.init.method, "POST");
    assert.equal(call.init.headers.Authorization, "Bearer test-token");
    assert.ok(call.init.headers["User-Agent"]);
    assert.equal(call.body.title, "FERB report: the folder was protected");
    assert.deepEqual(call.body.labels, ["failure", "0.28.0 (1)"]);
    assert.ok(call.body.body.includes("## Summary\nStatus: failed"));
    assert.ok(call.body.body.includes("```text\n"));                           // inert: a code block
  },

  async "a report over the body cap is refused with 413 and never reaches GitHub"() {
    const gh = fakeGitHub();
    const r = await handle(post(report({ body: "x".repeat(MAX_BODY + 1) })), env, gh.fetcher);
    assert.equal(r.status, 413);
    assert.equal((await r.json()).ok, false);
    assert.equal(gh.calls.length, 0);
  },

  async "a wrong method, path or content type is refused"() {
    const gh = fakeGitHub();
    assert.equal((await handle(post(null, { method: "GET" }), env, gh.fetcher)).status, 405);
    assert.equal((await handle(post(report(), { method: "PUT" }), env, gh.fetcher)).status, 405);
    assert.equal((await handle(post(report(), { path: "/" }), env, gh.fetcher)).status, 404);
    assert.equal((await handle(post(report(), { type: "text/plain" }), env, gh.fetcher)).status, 415);
    assert.equal(gh.calls.length, 0);
  },

  async "a wrong shape is refused with 400"() {
    const gh = fakeGitHub();
    const bad = [
      "not json", "[]", JSON.stringify(null),
      report({ version: 2 }), report({ kind: "spam" }), report({ title: "" }), report({ title: "t".repeat(201) }),
      report({ body: "" }), report({ body: 5 }), report({ app_version: "" }), report({ app_version: "v".repeat(51) }),
      report({ report_id: "../x" }), report({ extra: 1 }),
    ];
    const { title, ...noTitle } = report();
    bad.push(noTitle);
    for (const body of bad) {
      const r = await handle(post(body), env, gh.fetcher);
      assert.equal(r.status, 400, JSON.stringify(body).slice(0, 80));
      const out = await r.json();
      assert.equal(out.ok, false);
      assert.ok(out.error);
    }
    assert.equal(gh.calls.length, 0);
  },

  async "a long report is split into the issue and comments in order"() {
    const gh = fakeGitHub();
    const lines = [];
    for (let i = 0; lines.join("\n").length < 3 * PART_CHARS; i++) lines.push(`line ${i} ` + "y".repeat(90));
    const body = lines.join("\n");
    const r = await handle(post(report({ body })), env, gh.fetcher);
    assert.equal(r.status, 200);
    assert.ok(gh.calls.length >= 4);
    assert.ok(gh.calls[0].url.endsWith("/issues"));
    for (const c of gh.calls.slice(1)) assert.ok(c.url.endsWith("/issues/41/comments"));
    const texts = gh.calls.map((c) => {
      const b = c.body.body;
      assert.ok(b.length <= ISSUE_CHARS);
      return b.slice(b.indexOf("```text\n") + 8, b.lastIndexOf("```"));
    });
    assert.equal(texts.join(""), body + "\n");                                 // the fence closes on its own line
    assert.ok(gh.calls[1].body.body.includes(`part 2 of ${gh.calls.length}`));
  },

  async "split never cuts a surrogate pair and rejoins exactly"() {
    const text = "a" + "😀".repeat(50);
    const parts = split(text, 10);
    assert.equal(parts.join(""), text);
    for (const p of parts) assert.ok(!/[\ud800-\udbff]$/.test(p));
  },

  async "a GitHub failure is an error, not a success"() {
    for (const [opts, status] of [[{ issueStatus: 401 }, 502], [{ fail: true }, 502]]) {
      const gh = fakeGitHub(opts);
      const r = await handle(post(report()), env, gh.fetcher);
      assert.equal(r.status, status);
      const out = await r.json();
      assert.equal(out.ok, false);
      assert.ok(!JSON.stringify(out).includes("test-token"));
    }
    const gh = fakeGitHub({ commentStatus: 403 });
    const r = await handle(post(report({ body: "z\n".repeat(PART_CHARS) })), env, gh.fetcher);
    assert.equal(r.status, 502);
    assert.match((await r.json()).error, /part 2 of/);
  },

  async "one address is limited to a few reports per window"() {
    resetLimits();
    const gh = fakeGitHub();
    const now = 1_000_000;
    for (let i = 0; i < PER_WINDOW; i++) {
      assert.equal((await handle(post(report(), { ip: "198.51.100.7" }), env, gh.fetcher, now)).status, 200);
    }
    assert.equal((await handle(post(report(), { ip: "198.51.100.7" }), env, gh.fetcher, now)).status, 429);
    assert.equal((await handle(post(report(), { ip: "198.51.100.8" }), env, gh.fetcher, now)).status, 200);
    const later = now + 11 * 60 * 1000;
    assert.equal((await handle(post(report(), { ip: "198.51.100.7" }), env, gh.fetcher, later)).status, 200);
  },

  async "the rate-limit binding is asked by sender address"() {
    resetLimits();
    const asked = [];
    const limited = { ...env, REPORT_LIMIT: { limit: async ({ key }) => (asked.push(key), { success: false }) } };
    const gh = fakeGitHub();
    const r = await handle(post(report(), { ip: "192.0.2.4" }), limited, gh.fetcher);
    assert.equal(r.status, 429);
    assert.deepEqual(asked, ["192.0.2.4"]);
    assert.equal(gh.calls.length, 0);
  },

  async "without its token the relay says it isn't set up"() {
    const gh = fakeGitHub();
    const r = await handle(post(report()), { GITHUB_REPO: "owner/reports" }, gh.fetcher);
    assert.equal(r.status, 503);
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
