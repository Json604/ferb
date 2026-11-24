// The site's one build step (REL-435, REL-436), run by Vercel before each deployment (vercel.json buildCommand).
// With POSTHOG_KEY and POSTHOG_HOST set, it copies the pinned posthog-js bundle to /vendor/ and turns each page's
// `<!-- analytics -->` marker into the tag that loads /analytics.js; without them it changes nothing, so the pages
// carry no script and nothing loads. Vercel builds in a fresh checkout, so the pages are rewritten in place there;
// run here, it rewrites this folder's pages too (git shows it).
import { createHash } from "node:crypto";
import { mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

export const MARKER = "<!-- analytics -->";
export const PAGES = ["index.html", "privacy.html"];
// The full bundle carries every part (recordings, heatmaps, web vitals, exceptions), so PostHog's servers never
// send the page code at run time.
const BUNDLE = "dist/array.full.js";

const KEY = /^phc_[A-Za-z0-9]{20,80}$/;

const attr = (value) => String(value).replace(/&/g, "&amp;").replace(/"/g, "&quot;").replace(/</g, "&lt;");

// { key, host } from the environment, null when analytics isn't set up. A value that is set but malformed stops the
// build rather than deploying a site whose analytics silently does nothing.
export function readAnalytics(env) {
  const key = (env.POSTHOG_KEY || "").trim();
  const host = (env.POSTHOG_HOST || "").trim().replace(/\/+$/, "");
  if (!key && !host) return null;
  if (!KEY.test(key)) throw new Error("POSTHOG_KEY must be the project's public key (phc_…)");
  let url;
  try {
    url = new URL(host);
  } catch {
    url = null;
  }
  if (!url || url.protocol !== "https:" || url.pathname !== "/" || url.search || url.hash) {
    throw new Error("POSTHOG_HOST must be an https address with no path, e.g. https://us.i.posthog.com");
  }
  return { key, host: url.origin };
}

// The tag a page carries when analytics is set up: the site's own consent script, told where PostHog is.
export function loaderTag({ key, host }, { lib, integrity }) {
  return (
    `<script type="module" src="/analytics.js" data-posthog-key="${attr(key)}" data-posthog-host="${attr(host)}"` +
    ` data-posthog-lib="${attr(lib)}" data-posthog-integrity="${attr(integrity)}"></script>`
  );
}

export function withAnalytics(html, analytics, lib) {
  if (!analytics) return html;
  const at = html.indexOf(MARKER);
  if (at < 0 || html.indexOf(MARKER, at + 1) >= 0) throw new Error("a page must carry the analytics marker exactly once");
  return html.slice(0, at) + loaderTag(analytics, lib) + html.slice(at + MARKER.length);
}

export function build(root, env) {
  const analytics = readAnalytics(env);
  if (!analytics) {
    console.log("analytics: POSTHOG_KEY is not set; the pages load no script");
    return null;
  }
  const pkg = join(root, "node_modules", "posthog-js");
  const { version } = JSON.parse(readFileSync(join(pkg, "package.json"), "utf8"));
  const pinned = JSON.parse(readFileSync(join(root, "package.json"), "utf8")).dependencies["posthog-js"];
  if (version !== pinned) throw new Error(`posthog-js ${version} is installed, package.json pins ${pinned}`);
  const integrity = JSON.parse(readFileSync(join(pkg, "dist", "integrity.json"), "utf8")).files["array.full.js"];
  if (!/^sha384-/.test(integrity || "")) throw new Error("posthog-js's integrity.json has no hash for array.full.js");
  const bytes = readFileSync(join(pkg, BUNDLE));
  if (`sha384-${createHash("sha384").update(bytes).digest("base64")}` !== integrity) {
    throw new Error("posthog-js's array.full.js doesn't match its published hash");
  }
  const lib = `/vendor/posthog-js-${version}.js`;
  mkdirSync(join(root, "vendor"), { recursive: true });
  writeFileSync(join(root, lib), bytes);
  for (const page of PAGES) {
    const path = join(root, page);
    writeFileSync(path, withAnalytics(readFileSync(path, "utf8"), analytics, { lib, integrity }));
  }
  console.log(`analytics: PostHog ${version} on ${PAGES.join(", ")}, sending to ${analytics.host}`);
  return { lib, integrity };
}

if (process.argv[1] === fileURLToPath(import.meta.url)) build(dirname(fileURLToPath(import.meta.url)), process.env);
