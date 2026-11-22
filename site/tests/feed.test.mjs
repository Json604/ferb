// decision: REL-433
// decision: REL-432
// The update feed and the files Sparkle downloads stay public: only the site's own download link needs a sign-in.
import assert from "node:assert/strict";
import { existsSync, readFileSync } from "node:fs";
import { test } from "node:test";

const site = new URL("../", import.meta.url);
const vercel = JSON.parse(readFileSync(new URL("vercel.json", site), "utf8"));

// Vercel's path patterns as used here: ":name" matches one segment (or the part of one before a literal suffix).
function match(source, path) {
  const re = new RegExp(`^${source.replace(/[.]/g, "\\.").replace(/:([a-z]+)/g, "(?<$1>[^/]+?)")}$`);
  return path.match(re);
}

function route(path) {
  for (const r of vercel.redirects || []) if (match(r.source, path)) return { kind: "redirect", to: r.destination };
  for (const r of vercel.rewrites || []) if (match(r.source, path)) return { kind: "rewrite", to: r.destination };
  return { kind: "static" };
}

test("the feed, the update files and the notes redirect straight to the public releases", () => {
  for (const path of ["/updates/appcast-arm64.xml", "/updates/appcast-x86_64.xml", "/updates/files/v0.27.3/FERB-AppleSilicon.dmg", "/updates/notes/v0.27.3"]) {
    const r = route(path);
    assert.equal(r.kind, "redirect", path);
    assert.match(r.to, /^https:\/\/github\.com\/Json604\/ferb-update\/releases\//, path);
  }
  for (const r of vercel.redirects) assert.equal(r.permanent, false, "temporary, so the address can move (REL-432)");
});

test("nothing on the site stands between an installed FERB and its updates", () => {
  for (const r of [...(vercel.rewrites || []), ...(vercel.redirects || [])]) {
    assert.ok(!r.source.startsWith("/updates") || r.destination.startsWith("https://github.com/"), r.source);
  }
  for (const f of ["middleware.js", "middleware.mjs", "middleware.ts"]) assert.ok(!existsSync(new URL(f, site)), f);
  assert.equal(vercel.headers, undefined);
});

test("the site's own download goes through the signed link, never straight to the file", () => {
  assert.deepEqual(route("/download/FERB-AppleSilicon.dmg"), { kind: "rewrite", to: "/api/download?file=:file" });
  for (const page of ["index.html", "privacy.html"]) {
    const html = readFileSync(new URL(page, site), "utf8");
    assert.doesNotMatch(html, /href="[^"?]*\.dmg"/, page);
    assert.doesNotMatch(html, /href="https:\/\/github\.com\/[^"]*\/releases/, page);
  }
  assert.match(readFileSync(new URL("index.html", site), "utf8"), /href="\/api\/auth\/google\?file=FERB-AppleSilicon\.dmg"/);
});
