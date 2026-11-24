// decision: REL-449
// decision: REL-451
// The two plain pages Android's sign-in needs, the routes that reach them, and the settings' names.
import assert from "node:assert/strict";
import { existsSync, readdirSync, readFileSync } from "node:fs";
import { test } from "node:test";
import { appLanding, appReady, assetLinks } from "../api/_lib/app.js";
import { appFakes } from "./app-support.mjs";
import { parseFingerprints, readAppConfig } from "../api/_lib/deps.js";

const site = new URL("../", import.meta.url);
const read = (name) => readFileSync(new URL(name, site), "utf8");
const FP = Array.from({ length: 32 }, (_, i) => i.toString(16).padStart(2, "0").toUpperCase()).join(":");

test("assetlinks.json names the Android app and its certificates; it's an empty list until they're set", async () => {
  const none = assetLinks(new Request("https://ferb.example.com/.well-known/assetlinks.json"), { config: readAppConfig({}) });
  assert.equal(none.status, 200);
  assert.match(none.headers.get("content-type"), /^application\/json/);
  assert.deepEqual(await none.json(), []);
  const config = readAppConfig({ ANDROID_CERT_SHA256: `${FP.toLowerCase()}, not-a-fingerprint, ${FP.replace(/^00/, "FF")}` });
  const r = assetLinks(new Request("https://ferb.example.com/.well-known/assetlinks.json"), { config });
  assert.deepEqual(await r.json(), [
    {
      relation: ["delegate_permission/common.handle_all_urls"],
      target: { namespace: "android_app", package_name: "com.ferb.phone", sha256_cert_fingerprints: [FP, FP.replace(/^00/, "FF")] },
    },
  ]);
  assert.deepEqual(parseFingerprints("AA:BB"), []);
});

test("the App Link's fallback page has no script and links to the same address, with only the sign-in's values", async () => {
  const deps = { config: readAppConfig({ SITE_URL: "https://ferb.example.com/" }) };
  const r = appLanding(new Request("https://ferb.example.com/api/app/landing?code=abc_DEF-123&state=st4te-0123456789&evil=%3Cscript%3E&error="), deps);
  assert.equal(r.status, 200);
  assert.equal(r.headers.get("cache-control"), "no-store");
  assert.equal(r.headers.get("referrer-policy"), "no-referrer");
  const html = await r.text();
  assert.doesNotMatch(html, /<script/i);
  assert.ok(html.includes('href="https://ferb.example.com/app/callback?code=abc_DEF-123&amp;state=st4te-0123456789"'));
  const odd = await appLanding(new Request('https://ferb.example.com/app/callback?code="><b>x'), deps).text();
  assert.ok(odd.includes('href="https://ferb.example.com/app/callback"'));
});

test("vercel.json sends /.well-known/assetlinks.json and /app/callback to their functions, and the functions exist", () => {
  const vercel = JSON.parse(read("vercel.json"));
  const rewrite = (source) => vercel.rewrites.find((r) => r.source === source)?.destination;
  assert.equal(rewrite("/.well-known/assetlinks.json"), "/api/app/assetlinks");
  assert.equal(rewrite("/app/callback"), "/api/app/landing");
  const methods = {
    start: ["GET"],
    callback: ["GET", "POST"],
    token: ["POST"],
    me: ["GET"],
    signout: ["POST"],
    delete: ["POST"],
    assertion: ["POST"],
    assetlinks: ["GET"],
    landing: ["GET"],
    ready: ["GET"],
  };
  for (const [name, verbs] of Object.entries(methods)) {
    const source = read(`api/app/${name}.js`);
    for (const verb of verbs) assert.match(source, new RegExp(`export (async )?function ${verb}\\(`), `${name} ${verb}`);
  }
  assert.match(read(".vercelignore"), /^sql$/m, "the SQL for running by hand isn't served");
});

test("env.example names every setting the apps' sign-in reads", () => {
  const names = new Set(read("env.example").match(/^[A-Z0-9_]+(?==)/gm));
  for (const name of ["AUTH_SECRET", "DATABASE_URL", "SITE_URL", "GOOGLE_CLIENT_ID", "GOOGLE_CLIENT_SECRET", "APPLE_TEAM_ID", "APPLE_SERVICES_ID", "APPLE_KEY_ID", "APPLE_PRIVATE_KEY", "APP_ASSERTION_KEY", "APP_ASSERTION_KID", "ANDROID_CERT_SHA256"]) {
    assert.ok(names.has(name), name);
  }
  for (const line of read("env.example").split("\n")) if (/^[A-Z0-9_]+=/.test(line)) assert.match(line, /=$/, "names only");
});

// decision: REL-449
test("the site sends no email: no send-link, no unsubscribe, no mail settings, no mail service", () => {
  for (const gone of ["api/app/send-link.js", "api/unsubscribe.js", "api/_lib/mail.js", "scripts/send-update.mjs"]) {
    assert.equal(existsSync(new URL(gone, site)), false, gone);
  }
  assert.doesNotMatch(read("env.example"), /RESEND|EMAIL_FROM|EMAIL_REPLY_TO/);
  const code = (dir) =>
    readdirSync(new URL(dir, site), { withFileTypes: true }).flatMap((e) =>
      e.isDirectory() ? code(`${dir}${e.name}/`) : e.name.endsWith(".js") ? [`${dir}${e.name}`] : [],
    );
  for (const file of code("api/")) assert.doesNotMatch(read(file), /resend|sendMail|send-link|unsubscribe/i, file);
  for (const page of ["index.html", "privacy.html"]) {
    assert.doesNotMatch(read(page), /Resend|one-click unsubscribe|may email you|Send me the link/i, page);
  }
});

// decision: APP-1128
test("ready says whether a provider's sign-in is set up, so an app says so instead of opening an error page", async () => {
  const ask = async (deps, provider) => {
    const r = appReady(new Request(`https://ferb.example.com/api/app/ready?provider=${provider}`), deps);
    return { status: r.status, body: await r.json() };
  };
  assert.deepEqual(await ask(appFakes().deps, "google"), { status: 200, body: { ready: true } });
  assert.deepEqual(await ask(appFakes().deps, "apple"), { status: 200, body: { ready: true } });
  assert.deepEqual(await ask(appFakes({ apple: false }).deps, "apple"), { status: 200, body: { ready: false } });
  assert.deepEqual(await ask(appFakes({ google: false }).deps, "google"), { status: 200, body: { ready: false } });
  assert.deepEqual(await ask(appFakes({ ready: false }).deps, "google"), { status: 200, body: { ready: false } });
  assert.deepEqual(await ask({ config: null }, "google"), { status: 200, body: { ready: false } });
  assert.equal((await ask(appFakes().deps, "github")).status, 400);
});
