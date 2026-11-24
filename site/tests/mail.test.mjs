// decision: REL-433
// decision: REL-436
import assert from "node:assert/strict";
import { test } from "node:test";
import { sendUpdate, updateEmail } from "../api/_lib/mail.js";
import { makeStore } from "../api/_lib/store.js";
import { verify } from "../api/_lib/tokens.js";
import { NOW, SECRET } from "./support.mjs";

const mail = { from: "FERB <updates@ferb.example.com>", replyTo: "hello@example.com", siteUrl: "https://ferb.example.com", secret: SECRET };

test("every update email carries the person's own one-click unsubscribe, as a link and as headers", () => {
  const e = updateEmail({ ...mail, to: "ada@example.com", subject: "FERB 0.28", body: "Line one.\n\nA <b>second</b> one." });
  const url = e.headers["List-Unsubscribe"].slice(1, -1);
  assert.equal(e.headers["List-Unsubscribe-Post"], "List-Unsubscribe=One-Click");
  assert.match(url, /^https:\/\/ferb\.example\.com\/api\/unsubscribe\?t=/);
  assert.equal(verify(SECRET, new URL(url).searchParams.get("t"), "unsubscribe", NOW).e, "ada@example.com");
  assert.ok(e.text.includes(url));
  assert.ok(e.html.includes(url));
  assert.ok(!e.html.includes("<b>"));
  assert.deepEqual(e.to, ["ada@example.com"]);
  assert.equal(e.reply_to, "hello@example.com");
});

test("without --send nothing is sent", async () => {
  let calls = 0;
  const r = await sendUpdate({
    recipients: ["a@example.com", "b@example.com"],
    subject: "s",
    body: "b",
    mail,
    send: false,
    fetch: () => calls++,
  });
  assert.equal(calls, 0);
  assert.equal(r.planned, 2);
  assert.equal(r.sent, 0);
  assert.deepEqual(r.preview.to, ["a@example.com"]);
});

test("--send sends one email per person to Resend, each with its own idempotency key, and reports failures", async () => {
  const seen = [];
  const fetch = async (url, init) => {
    seen.push({ url, key: init.headers["idempotency-key"], auth: init.headers.authorization, to: JSON.parse(init.body).to[0] });
    return seen.length === 2 ? new Response("rate limited", { status: 429 }) : Response.json({ id: "x" });
  };
  const logs = [];
  const r = await sendUpdate({
    recipients: ["a@example.com", "b@example.com", "c@example.com"],
    subject: "s",
    body: "b",
    mail,
    send: true,
    apiKey: "re_test",
    fetch,
    wait: async () => {},
    log: (m) => logs.push(m),
  });
  assert.deepEqual(seen.map((s) => s.to), ["a@example.com", "b@example.com", "c@example.com"]);
  assert.ok(seen.every((s) => s.url === "https://api.resend.com/emails" && s.auth === "Bearer re_test"));
  assert.equal(new Set(seen.map((s) => s.key)).size, 3);
  assert.deepEqual(r, { planned: 3, sent: 2, failed: ["b@example.com"] });
});

test("the user list: signing in again changes only the name, and only an unsubscribe touches the flag", async () => {
  const queries = [];
  const store = makeStore({ query: async (text, params) => (queries.push({ text, params }), []) });
  await store.recordSignIn({ email: "ada@example.com", name: "Ada", at: new Date(0) });
  await store.unsubscribe("ada@example.com");
  assert.match(queries[0].text, /CREATE TABLE IF NOT EXISTS ferb_users/);
  const upsert = queries[1].text.replace(/\s+/g, " ");
  assert.match(upsert, /ON CONFLICT \(email\) DO UPDATE SET name = EXCLUDED\.name$/);
  assert.deepEqual(queries[1].params, ["ada@example.com", "Ada", new Date(0)]);
  assert.equal(queries.filter((q) => /CREATE TABLE/.test(q.text)).length, 1, "the table is made once per instance");
  assert.match(queries[2].text, /SET unsubscribed = true WHERE email = \$1/);
});
