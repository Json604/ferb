#!/usr/bin/env node
// Send an update email to everyone on the list who hasn't unsubscribed (REL-433). Run by the owner, by hand, from
// site/ after `npm ci` and `vercel env pull .env.local`:
//
//   node --env-file=.env.local scripts/send-update.mjs --subject "FERB 0.28" --body update.txt
//       shows how many people would get it and the first email, and sends nothing
//   ... --test-to you@example.com --send     sends it to that one address only
//   ... --send                               sends it to the whole list
//
// Needs AUTH_SECRET, SITE_URL, RESEND_API_KEY, EMAIL_FROM, EMAIL_REPLY_TO and (for the list) DATABASE_URL.
import { readFileSync } from "node:fs";
import { parseArgs } from "node:util";
import { sendUpdate } from "../api/_lib/mail.js";
import { neonStore } from "../api/_lib/store.js";

const { values } = parseArgs({
  options: {
    subject: { type: "string" },
    body: { type: "string" },
    "test-to": { type: "string" },
    send: { type: "boolean", default: false },
  },
});

function need(name) {
  const value = process.env[name];
  if (!value) {
    console.error(`${name} is not set (vercel env pull .env.local, then --env-file=.env.local)`);
    process.exit(2);
  }
  return value;
}

if (!values.subject || !values.body) {
  console.error('usage: send-update.mjs --subject "..." --body FILE [--test-to EMAIL] [--send]');
  process.exit(2);
}
const body = readFileSync(values.body, "utf8");
const mail = {
  from: need("EMAIL_FROM"),
  replyTo: need("EMAIL_REPLY_TO"),
  siteUrl: need("SITE_URL").replace(/\/+$/, ""),
  secret: need("AUTH_SECRET"),
};
const apiKey = values.send ? need("RESEND_API_KEY") : "";
const recipients = values["test-to"]
  ? [values["test-to"].trim().toLowerCase()]
  : (await (await neonStore(need("DATABASE_URL"))).subscribedUsers()).map((u) => u.email);

const result = await sendUpdate({
  recipients,
  subject: values.subject,
  body,
  mail,
  send: values.send,
  apiKey,
  wait: (ms) => new Promise((r) => setTimeout(r, ms)),
  log: (m) => console.error(m),
});

if (!values.send) {
  console.log(`Would send to ${result.planned} ${result.planned === 1 ? "person" : "people"}. Nothing was sent.`);
  if (result.preview) console.log(`\nFirst email, to ${result.preview.to[0]}:\n\n${result.preview.text}`);
  console.log("Add --send to send it.");
} else {
  console.log(`Sent ${result.sent} of ${result.planned}.${result.failed.length ? ` Not sent: ${result.failed.join(", ")}` : ""}`);
  process.exit(result.failed.length ? 1 : 0);
}
