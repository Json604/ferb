// Update emails through Resend (REL-433, REL-434). Every email carries the person's own one-click unsubscribe: a link
// at the bottom and the List-Unsubscribe headers (RFC 8058) mail apps show as a button. Used only by
// scripts/send-update.mjs, which the owner runs by hand; nothing on the site sends mail.
import { createHash } from "node:crypto";
import { escapeHtml } from "./http.js";
import { unsubscribeToken } from "./tokens.js";

const RESEND = "https://api.resend.com/emails";

export const unsubscribeUrl = (siteUrl, secret, email) =>
  `${siteUrl}/api/unsubscribe?t=${unsubscribeToken(secret, email)}`;

export function updateEmail({ to, subject, body, from, replyTo, siteUrl, secret }) {
  const url = unsubscribeUrl(siteUrl, secret, to);
  const why = "You're getting this because you signed in to download FERB.";
  const paragraphs = body
    .trim()
    .split(/\n\s*\n/)
    .map((p) => `<p>${escapeHtml(p.trim()).replace(/\n/g, "<br>")}</p>`)
    .join("\n");
  return {
    from,
    to: [to],
    reply_to: replyTo,
    subject,
    text: `${body.trim()}\n\n--\n${why}\nUnsubscribe with one click: ${url}\n`,
    html: `${paragraphs}\n<hr>\n<p style="font-size:12px;color:#666">${why} <a href="${escapeHtml(url)}">Unsubscribe</a> with one click.</p>\n`,
    headers: {
      "List-Unsubscribe": `<${url}>`,
      "List-Unsubscribe-Post": "List-Unsubscribe=One-Click",
    },
  };
}

// The same update to the same person is sent once, even if the script is run again within Resend's 24 hours.
export const idempotencyKey = (subject, body, email) =>
  createHash("sha256").update(`${subject}\n${body}\n${email}`).digest("hex").slice(0, 48);

export async function sendEmail(email, { apiKey, key, fetch = globalThis.fetch }) {
  const response = await fetch(RESEND, {
    method: "POST",
    headers: { authorization: `Bearer ${apiKey}`, "content-type": "application/json", "idempotency-key": key },
    body: JSON.stringify(email),
  });
  if (!response.ok) throw new Error(`Resend answered ${response.status}: ${(await response.text()).slice(0, 300)}`);
  return response.json();
}

// Plans the emails for `recipients` (emails), and sends them only when `send` is true, one at a time.
export async function sendUpdate({ recipients, subject, body, mail, send, apiKey, fetch, wait, log }) {
  const emails = recipients.map((to) => updateEmail({ ...mail, to, subject, body }));
  const result = { planned: emails.length, sent: 0, failed: [] };
  if (!send) return { ...result, preview: emails[0] || null };
  for (const [i, email] of emails.entries()) {
    if (i > 0) await wait(600); // under Resend's default rate limit
    try {
      await sendEmail(email, { apiKey, key: idempotencyKey(subject, body, email.to[0]), fetch });
      result.sent += 1;
    } catch (error) {
      result.failed.push(email.to[0]);
      log(`not sent to ${email.to[0]}: ${error.message}`);
    }
  }
  return result;
}
