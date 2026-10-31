// FERB's failure-report relay (owner decisions 2026-11-01): each Send in FERB.app becomes exactly one issue in the
// owner's private repo (GITHUB_REPO) holding a short summary, plus the full cleaned report as one file in that repo,
// linked from the issue. The token (the GITHUB_TOKEN secret) can only read and write that repo's issues and contents.
// The person sending needs no GitHub account, browser or mail app.
//
// One Send is two requests, so the Worker never parses or re-encodes the big file (the free plan gives 10 ms of CPU
// per request; waiting on GitHub doesn't count):
//   1. POST /file?kind=failure|diagnostic&report_id=ID with Content-Type application/json and a Content-Length of at
//      most MAX_UPLOAD. The body is exactly GitHub's "create a file" JSON ({"message", "content": base64}), built by
//      FERB; the Worker checks only the headers and the query, picks the file's path and streams the body untouched to
//      GitHub's Contents API. Answers {"ok": true, "ticket": ...}: the path, kind and report id, signed, for step 2.
//      A conflict with another commit answers 409 with "retry": true; FERB sends the file again.
//   2. POST /report with the small JSON {"version": 2, "ticket", "title", "summary", "app_version"}: the Worker checks
//      it and the ticket's signature, then files the issue: a link to the file, then the summary as a code block.
//      Answers {"ok": true, "report_id": ...}.
// Errors answer {"ok": false, "error": ...}. It never answers with the issue's URL or number (the repo is private)
// and never logs a report's text.

export const MAX_REPORT = 25_000_000;                         // bytes of one report file (owner, 2026-11-01)
export const MAX_UPLOAD = Math.ceil(MAX_REPORT / 3) * 4 + 4096;   // its base64 plus GitHub's JSON around it
export const MAX_BODY = 64 * 1024;                            // bytes of step 2's JSON
export const MAX_SUMMARY = 30000;                             // characters of the summary in the issue
export const MAX_TITLE = 200;
export const MAX_LABEL = 50;                                  // GitHub's limit on a label's name
export const TICKET_MS = 60 * 60 * 1000;                      // how long step 1's ticket can file its issue
export const KINDS = ["failure", "diagnostic"];
const FIELDS = ["version", "ticket", "title", "summary", "app_version"];
const REPORT_ID = /^[A-Za-z0-9][A-Za-z0-9._-]{0,127}$/;
const CONTROL = /[\u0000-\u001f\u007f]/;
const CONTROLS = /[\u0000-\u001f\u007f]/g;
const GITHUB = "https://api.github.com";

// Per sender address, kept in this Worker instance only: a second guard beside the REPORT_LIMIT binding (whose window
// can only be 10 or 60 seconds). Cloudflare may run several instances, so this is a best-effort cap, not a promise.
// Files and issues are counted apart, so one Send's two requests never use up each other's allowance.
export const WINDOW_MS = 10 * 60 * 1000;
export const PER_WINDOW = 5;
const recent = new Map();
const filed = new Set();      // tickets this instance already turned into an issue (best effort; tickets also expire)

function answer(status, data, extra = {}) {
  return new Response(JSON.stringify(data), {
    status,
    headers: { "Content-Type": "application/json; charset=utf-8", "Cache-Control": "no-store", ...extra },
  });
}

const refuse = (status, error, extra) => answer(status, { ok: false, error }, extra);

// The body as text, refused past `cap` bytes without reading the rest (step 2 only: its body is small).
async function readCapped(request, cap) {
  const declared = Number(request.headers.get("Content-Length"));
  if (Number.isFinite(declared) && declared > cap) return null;
  if (!request.body) return "";
  const reader = request.body.getReader();
  const chunks = [];
  let size = 0;
  for (;;) {
    const { done, value } = await reader.read();
    if (done) break;
    size += value.byteLength;
    if (size > cap) {
      await reader.cancel();
      return null;
    }
    chunks.push(value);
  }
  const all = new Uint8Array(size);
  let at = 0;
  for (const c of chunks) {
    all.set(c, at);
    at += c.byteLength;
  }
  return new TextDecoder("utf-8", { fatal: true }).decode(all);
}

// What is wrong with a parsed step-2 report, or "" when it is right.
export function problem(report) {
  if (report === null || typeof report !== "object" || Array.isArray(report)) return "the body must be a JSON object";
  const keys = Object.keys(report);
  const extra = keys.filter((k) => !FIELDS.includes(k));
  if (extra.length) return `unexpected field: ${extra[0]}`;
  const missing = FIELDS.filter((k) => !keys.includes(k));
  if (missing.length) return `missing field: ${missing[0]}`;
  if (report.version !== 2) return "version must be 2";
  for (const k of ["ticket", "title", "summary", "app_version"]) {
    if (typeof report[k] !== "string") return `${k} must be a string`;
  }
  if (!report.title.trim() || report.title.length > MAX_TITLE) return `title must be 1 to ${MAX_TITLE} characters`;
  if (!report.summary.trim() || report.summary.length > MAX_SUMMARY) {
    return `summary must be 1 to ${MAX_SUMMARY} characters`;
  }
  if (!report.app_version.trim() || report.app_version.length > MAX_LABEL || CONTROL.test(report.app_version)) {
    return `app_version must be 1 to ${MAX_LABEL} characters with no control characters`;
  }
  return "";
}

// Text as a code block (fenced longer than any run of backticks in it), so nothing in a report is read as Markdown:
// no @mention notifies anyone, no #number links an issue, no heading or link renders.
export function fenced(text) {
  const longest = Math.max(0, ...(text.match(/`+/g) || []).map((run) => run.length));
  const fence = "`".repeat(Math.max(3, longest + 1));
  return `${fence}text\n${text}${text.endsWith("\n") ? "" : "\n"}${fence}`;
}

function limited(key, now) {
  const times = (recent.get(key) || []).filter((t) => now - t < WINDOW_MS);
  if (times.length >= PER_WINDOW) {
    recent.set(key, times);
    return true;
  }
  times.push(now);
  recent.set(key, times);
  if (recent.size > 10000) recent.delete(recent.keys().next().value);   // never grows without end
  return false;
}

async function overLimit(env, key, now) {
  if (env.REPORT_LIMIT) {
    const { success } = await env.REPORT_LIMIT.limit({ key });
    if (!success) return true;
  }
  return limited(key, now);
}

export function resetLimits() {
  recent.clear();
  filed.clear();
}

const b64url = (bytes) => btoa(String.fromCharCode(...bytes)).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");

function unb64url(text) {
  const plain = atob(text.replace(/-/g, "+").replace(/_/g, "/"));
  return Uint8Array.from(plain, (c) => c.charCodeAt(0));
}

// Tickets are signed with a key derived from the token: nothing more to set up, and rotating the token voids them.
async function ticketKey(env) {
  const raw = new TextEncoder().encode(`ferb-reports-ticket:${env.GITHUB_TOKEN}`);
  return crypto.subtle.importKey("raw", raw, { name: "HMAC", hash: "SHA-256" }, false, ["sign", "verify"]);
}

export async function sign(env, data) {
  const body = b64url(new TextEncoder().encode(JSON.stringify(data)));
  const mac = await crypto.subtle.sign("HMAC", await ticketKey(env), new TextEncoder().encode(body));
  return `${body}.${b64url(new Uint8Array(mac))}`;
}

// The ticket's contents when its signature is right and it hasn't expired, else null.
export async function verify(env, ticket, now) {
  const [body, mac, more] = ticket.split(".");
  if (!body || !mac || more !== undefined || ticket.length > 1024) return null;
  try {
    const ok = await crypto.subtle.verify("HMAC", await ticketKey(env), unb64url(mac), new TextEncoder().encode(body));
    if (!ok) return null;
    const data = JSON.parse(new TextDecoder().decode(unb64url(body)));
    if (typeof data.p !== "string" || !KINDS.includes(data.k) || !REPORT_ID.test(data.r) || !(data.e > now)) return null;
    return data;
  } catch {
    return null;
  }
}

function headers(env) {
  return {
    Authorization: `Bearer ${env.GITHUB_TOKEN}`,
    Accept: "application/vnd.github+json",
    "X-GitHub-Api-Version": "2022-11-28",
    "Content-Type": "application/json",
    "User-Agent": "ferb-reports-relay",
  };
}

// The file's path in the repo, by month, with a random part: report ids can repeat across Macs, and a path that
// never existed can't be overwritten.
export function filePath(reportId, now) {
  const at = new Date(now);
  const month = String(at.getUTCMonth() + 1).padStart(2, "0");
  const random = b64url(crypto.getRandomValues(new Uint8Array(6))).replace(/[-_]/g, "x");
  return `reports/${at.getUTCFullYear()}/${month}/${reportId}-${random}.txt`;
}

function common(request, env, method) {
  if (request.method !== method) return refuse(405, `only ${method} is accepted`, { Allow: method });
  const type = (request.headers.get("Content-Type") || "").split(";")[0].trim().toLowerCase();
  if (type !== "application/json") return refuse(415, "Content-Type must be application/json");
  if (!env.GITHUB_TOKEN || !env.GITHUB_REPO) return refuse(503, "the relay isn't set up");
  return null;
}

// Step 1: the file, streamed to GitHub untouched.
async function file(request, env, fetcher, now) {
  const url = new URL(request.url);
  const kind = url.searchParams.get("kind") || "";
  const reportId = url.searchParams.get("report_id") || "";
  if (!KINDS.includes(kind)) return refuse(400, `kind must be one of ${KINDS.join(", ")}`);
  if (!REPORT_ID.test(reportId)) return refuse(400, "report_id must be letters, digits, dots, dashes or underscores");
  const length = request.headers.get("Content-Length");
  if (length === null) return refuse(411, "Content-Length is required");
  if (!/^\d{1,12}$/.test(length) || Number(length) === 0) return refuse(400, "the body must not be empty");
  if (Number(length) > MAX_UPLOAD) return refuse(413, `the report is larger than ${MAX_REPORT} bytes`);
  if (!request.body) return refuse(400, "the body must not be empty");
  const ip = request.headers.get("CF-Connecting-IP") || "unknown";
  if (await overLimit(env, `file:${ip}`, now)) {
    return refuse(429, "too many reports from this network; try again in a few minutes");
  }

  const path = filePath(reportId, now);
  let put;
  try {
    put = await fetcher(`${GITHUB}/repos/${env.GITHUB_REPO}/contents/${path}`,
      { method: "PUT", headers: headers(env), body: request.body, duplex: "half" });
    await put.body?.cancel();                       // GitHub's answer repeats the file's details; nothing is needed
  } catch {
    return refuse(502, "the relay couldn't reach GitHub");
  }
  if (put.status === 409) {
    return answer(409, { ok: false, retry: true, error: "another report was being saved at the same time" });
  }
  if (put.status === 413) return refuse(413, "GitHub refused the file as too large");
  if (put.status !== 201) return refuse(502, `GitHub refused the file (HTTP ${put.status})`);
  return answer(200, { ok: true, ticket: await sign(env, { p: path, k: kind, r: reportId, e: now + TICKET_MS }) });
}

// Step 2: the issue, with the summary and a link to the file step 1 saved.
async function issue(request, env, fetcher, now) {
  let text;
  try {
    text = await readCapped(request, MAX_BODY);
  } catch {
    return refuse(400, "the body must be UTF-8 JSON");
  }
  if (text === null) return refuse(413, `the summary is larger than ${MAX_BODY} bytes`);
  let report;
  try {
    report = JSON.parse(text);
  } catch {
    return refuse(400, "the body must be JSON");
  }
  const wrong = problem(report);
  if (wrong) return refuse(400, wrong);
  const ticket = await verify(env, report.ticket, now);
  if (!ticket) return refuse(403, "the ticket isn't valid; send the report again");
  if (filed.has(report.ticket)) return refuse(409, "this report was already filed");
  const ip = request.headers.get("CF-Connecting-IP") || "unknown";
  if (await overLimit(env, `issue:${ip}`, now)) {
    return refuse(429, "too many reports from this network; try again in a few minutes");
  }

  const version = report.app_version.trim();
  const link = `https://github.com/${env.GITHUB_REPO}/blob/HEAD/${ticket.p}`;
  const head = `FERB ${ticket.k} report ${ticket.r} · app ${version}`;
  try {
    const made = await fetcher(`${GITHUB}/repos/${env.GITHUB_REPO}/issues`, {
      method: "POST",
      headers: headers(env),
      body: JSON.stringify({
        title: report.title.replace(CONTROLS, " ").trim(),
        body: `Full report: [${ticket.p}](${link})\n\n${fenced(`${head}\n\n${report.summary}`)}`,
        labels: [ticket.k, version],
      }),
    });
    await made.body?.cancel();
    if (made.status !== 201) return refuse(502, `GitHub refused the report (HTTP ${made.status})`);
  } catch {
    return refuse(502, "the relay couldn't reach GitHub");
  }
  filed.add(report.ticket);
  if (filed.size > 10000) filed.delete(filed.values().next().value);
  return answer(200, { ok: true, report_id: ticket.r });
}

export async function handle(request, env, fetcher = fetch, now = Date.now()) {
  const path = new URL(request.url).pathname;
  if (path === "/file") return common(request, env, "POST") || file(request, env, fetcher, now);
  if (path === "/report") return common(request, env, "POST") || issue(request, env, fetcher, now);
  return refuse(404, "not found");
}

export default {
  fetch(request, env) {
    return handle(request, env);
  },
};
