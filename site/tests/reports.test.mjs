// decision: REL-447
// decision: REL-448
// Send to developer reaches the report relay at the owner's domain: the site forwards (rewrites, never redirects: the
// upload's body) exactly the relay's two requests to the relay's own address, and nothing else under /reports.
import assert from "node:assert/strict";
import { existsSync, readFileSync } from "node:fs";
import { test } from "node:test";

const site = new URL("../", import.meta.url);
const vercel = JSON.parse(readFileSync(new URL("vercel.json", site), "utf8"));
const RELAY = "https://ferb-reports.ferb-reports.workers.dev";

// Vercel's path patterns as used here: ":name" matches one segment, ":name(a|b)" one of those words.
function match(source, path) {
  const pattern = source
    .replace(/[.]/g, "\\.")
    .replace(/:([a-z]+)\(([a-z|]+)\)/g, "(?<$1>$2)")
    .replace(/:([a-z]+)(?![a-z(])/g, "(?<$1>[^/]+?)");
  return path.match(new RegExp(`^${pattern}$`));
}

function route(path) {
  for (const r of vercel.redirects || []) if (match(r.source, path)) return { kind: "redirect", to: r.destination };
  for (const r of vercel.rewrites || []) {
    const m = match(r.source, path);
    if (m) return { kind: "rewrite", to: r.destination.replace(/:([a-z]+)/g, (_, k) => m.groups[k]) };
  }
  return { kind: "static" };
}

test("the relay's two requests are forwarded to the relay's own address, unchanged", () => {
  assert.deepEqual(route("/reports/file"), { kind: "rewrite", to: `${RELAY}/file` });
  assert.deepEqual(route("/reports/report"), { kind: "rewrite", to: `${RELAY}/report` });
});

test("nothing else under /reports reaches the relay, and nothing redirects there", () => {
  for (const path of ["/reports", "/reports/", "/reports/files", "/reports/file/x", "/reports/admin", "/report"]) {
    assert.equal(route(path).kind, "static", path);
  }
  for (const r of vercel.redirects || []) assert.ok(!r.destination.startsWith(RELAY), r.source);
  const toRelay = (vercel.rewrites || []).filter((r) => r.destination.startsWith(RELAY));
  assert.equal(toRelay.length, 1);
  // a file or function at that path would win over the rewrite (Vercel serves the filesystem first)
  assert.ok(!existsSync(new URL("reports", site)));
  assert.ok(!existsSync(new URL("api/reports", site)));
});

test("the forwarded answer is never cached or given new headers by the site", () => {
  // the relay answers Cache-Control: no-store itself; the site adds nothing over it
  assert.equal(vercel.headers, undefined);
  assert.equal(vercel.routes, undefined);
});
