// FERB's failure-report relay (owner decision 2026-11-01): FERB.app's Send posts one cleaned report here, and this
// Worker files it as an issue in the owner's private repo (GITHUB_REPO) with a token that can only read and write that
// repo's issues (the GITHUB_TOKEN secret). The person sending needs no GitHub account, browser or mail app.
//
// It accepts only POST /report with a JSON body of exactly
//   {"version": 1, "kind": "failure"|"diagnostic", "title": str, "body": str, "app_version": str, "report_id": str}
// and answers {"ok": true, "report_id": ...} or {"ok": false, "error": ...}. It never answers with the issue's URL or
// number (the repo is private) and never logs a report's text.

export const MAX_BODY = 512 * 1024;          // bytes of the request body; FERB checks the same cap before sending
export const MAX_TITLE = 200;
export const MAX_LABEL = 50;                  // GitHub's limit on a label's name
export const ISSUE_CHARS = 65536;             // GitHub's limit on an issue's or a comment's body
export const PART_CHARS = 60000;              // a report's text per issue or comment, leaving room for its framing
export const KINDS = ["failure", "diagnostic"];
const FIELDS = ["version", "kind", "title", "body", "app_version", "report_id"];
const REPORT_ID = /^[A-Za-z0-9][A-Za-z0-9._-]{0,127}$/;
const CONTROL = /[\u0000-\u001f\u007f]/;
const CONTROLS = /[\u0000-\u001f\u007f]/g;

// Per sender address, kept in this Worker instance only: a second guard beside the REPORT_LIMIT binding (whose window
// can only be 10 or 60 seconds). Cloudflare may run several instances, so this is a best-effort cap, not a promise.
export const WINDOW_MS = 10 * 60 * 1000;
export const PER_WINDOW = 5;
const recent = new Map();

function answer(status, data, extra = {}) {
  return new Response(JSON.stringify(data), {
    status,
    headers: { "Content-Type": "application/json; charset=utf-8", "Cache-Control": "no-store", ...extra },
  });
}

const refuse = (status, error, extra) => answer(status, { ok: false, error }, extra);

// The body as text, refused past MAX_BODY bytes without reading the rest.
async function readCapped(request) {
  const declared = Number(request.headers.get("Content-Length"));
  if (Number.isFinite(declared) && declared > MAX_BODY) return null;
  if (!request.body) return "";
  const reader = request.body.getReader();
  const chunks = [];
  let size = 0;
  for (;;) {
    const { done, value } = await reader.read();
    if (done) break;
    size += value.byteLength;
    if (size > MAX_BODY) {
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

// What is wrong with a parsed report, or "" when it is right.
export function problem(report) {
  if (report === null || typeof report !== "object" || Array.isArray(report)) return "the body must be a JSON object";
  const keys = Object.keys(report);
  const extra = keys.filter((k) => !FIELDS.includes(k));
  if (extra.length) return `unexpected field: ${extra[0]}`;
  const missing = FIELDS.filter((k) => !keys.includes(k));
  if (missing.length) return `missing field: ${missing[0]}`;
  if (report.version !== 1) return "version must be 1";
  if (!KINDS.includes(report.kind)) return `kind must be one of ${KINDS.join(", ")}`;
  for (const k of ["title", "body", "app_version", "report_id"]) {
    if (typeof report[k] !== "string") return `${k} must be a string`;
  }
  if (!report.title.trim() || report.title.length > MAX_TITLE) return `title must be 1 to ${MAX_TITLE} characters`;
  if (!report.body.trim()) return "body must not be empty";
  if (!report.app_version.trim() || report.app_version.length > MAX_LABEL || CONTROL.test(report.app_version)) {
    return `app_version must be 1 to ${MAX_LABEL} characters with no control characters`;
  }
  if (!REPORT_ID.test(report.report_id)) return "report_id must be letters, digits, dots, dashes or underscores";
  return "";
}

// The text in pieces of at most `size` characters, cut after a line where one ends in the second half of a piece and
// never between the two halves of a surrogate pair.
export function split(text, size = PART_CHARS) {
  const parts = [];
  let rest = text;
  while (rest.length > size) {
    let cut = rest.lastIndexOf("\n", size - 1) + 1;
    if (cut < size / 2) cut = size;
    const code = rest.charCodeAt(cut - 1);
    if (code >= 0xd800 && code <= 0xdbff) cut -= 1;
    parts.push(rest.slice(0, cut));
    rest = rest.slice(cut);
  }
  parts.push(rest);
  return parts;
}

// One part as a code block (fenced longer than any run of backticks in it), so nothing in a report is read as
// Markdown: no @mention notifies anyone, no #number links an issue, no heading or link renders.
export function fenced(text) {
  const longest = Math.max(0, ...(text.match(/`+/g) || []).map((run) => run.length));
  const fence = "`".repeat(Math.max(3, longest + 1));
  return `${fence}text\n${text}${text.endsWith("\n") ? "" : "\n"}${fence}`;
}

function limited(ip, now) {
  const times = (recent.get(ip) || []).filter((t) => now - t < WINDOW_MS);
  if (times.length >= PER_WINDOW) {
    recent.set(ip, times);
    return true;
  }
  times.push(now);
  recent.set(ip, times);
  if (recent.size > 10000) recent.delete(recent.keys().next().value);   // never grows without end
  return false;
}

export function resetLimits() {
  recent.clear();
}

async function github(env, fetcher, path, data) {
  return fetcher(`https://api.github.com/repos/${env.GITHUB_REPO}${path}`, {
    method: "POST",
    headers: {
      Authorization: `Bearer ${env.GITHUB_TOKEN}`,
      Accept: "application/vnd.github+json",
      "X-GitHub-Api-Version": "2022-11-28",
      "Content-Type": "application/json",
      "User-Agent": "ferb-reports-relay",
    },
    body: JSON.stringify(data),
  });
}

export async function handle(request, env, fetcher = fetch, now = Date.now()) {
  const url = new URL(request.url);
  if (url.pathname !== "/report") return refuse(404, "not found");
  if (request.method !== "POST") return refuse(405, "only POST is accepted", { Allow: "POST" });
  const type = (request.headers.get("Content-Type") || "").split(";")[0].trim().toLowerCase();
  if (type !== "application/json") return refuse(415, "Content-Type must be application/json");
  if (!env.GITHUB_TOKEN || !env.GITHUB_REPO) return refuse(503, "the relay isn't set up");

  let text;
  try {
    text = await readCapped(request);
  } catch {
    return refuse(400, "the body must be UTF-8 JSON");
  }
  if (text === null) return refuse(413, `the report is larger than ${MAX_BODY} bytes`);
  let report;
  try {
    report = JSON.parse(text);
  } catch {
    return refuse(400, "the body must be JSON");
  }
  const wrong = problem(report);
  if (wrong) return refuse(400, wrong);

  // Counted only for reports that would reach GitHub
  const ip = request.headers.get("CF-Connecting-IP") || "unknown";
  if (env.REPORT_LIMIT) {
    const { success } = await env.REPORT_LIMIT.limit({ key: ip });
    if (!success) return refuse(429, "too many reports from this network; try again in a few minutes");
  }
  if (limited(ip, now)) return refuse(429, "too many reports from this network; try again in a few minutes");

  const parts = split(report.body);
  const head = (n) => `FERB ${report.kind} report \`${report.report_id}\` · app ${report.app_version.replace(/`/g, "'")}` +
    (parts.length > 1 ? ` · part ${n} of ${parts.length}` : "");
  const title = report.title.replace(CONTROLS, " ").trim();
  try {
    const made = await github(env, fetcher, "/issues", {
      title,
      body: `${head(1)}\n\n${fenced(parts[0])}`,
      labels: [report.kind, report.app_version.trim()],
    });
    if (made.status !== 201) return refuse(502, `GitHub refused the report (HTTP ${made.status})`);
    const issue = await made.json();
    for (let i = 1; i < parts.length; i++) {
      const added = await github(env, fetcher, `/issues/${issue.number}/comments`,
        { body: `${head(i + 1)}\n\n${fenced(parts[i])}` });
      if (added.status !== 201) {
        return refuse(502, `GitHub kept only part of the report (part ${i + 1} of ${parts.length}: HTTP ${added.status})`);
      }
    }
  } catch {
    return refuse(502, "the relay couldn't reach GitHub");
  }
  return answer(200, { ok: true, report_id: report.report_id });
}

export default {
  fetch(request, env) {
    return handle(request, env);
  },
};
