// decision: REL-435
// decision: REL-437
import assert from "node:assert/strict";
import { mkdtempSync, readFileSync, rmSync, symlinkSync, writeFileSync, existsSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { test } from "node:test";
import { fileURLToPath } from "node:url";
import { build, MARKER, PAGES, readAnalytics, withAnalytics } from "../build.mjs";
import { allowExternalScript, cleanUrl, excluded, settings, start, STRINGS } from "../analytics.js";
import { admin, unsubscribe } from "../api/_lib/handlers.js";
import { page } from "../api/_lib/http.js";
import { banner, buttons, fakePage, fakePostHog, posthogScripts } from "./dom.mjs";
import { fakeDeps, get } from "./support.mjs";

const SITE = fileURLToPath(new URL("..", import.meta.url));
const KEY = "phc_TESTKEY0123456789abcdefghij";
const ENV = { POSTHOG_KEY: KEY, POSTHOG_HOST: "https://eu.i.posthog.com/" };

function siteCopy() {
  const root = mkdtempSync(join(tmpdir(), "ferb-site-"));
  for (const f of [...PAGES, "package.json"]) writeFileSync(join(root, f), readFileSync(join(SITE, f)));
  symlinkSync(join(SITE, "node_modules"), join(root, "node_modules"));
  return root;
}

test("the pages carry the analytics marker once and no script of their own", () => {
  for (const name of PAGES) {
    const html = readFileSync(join(SITE, name), "utf8");
    assert.equal(html.split(MARKER).length, 2, name);
    assert.doesNotMatch(html, /<script/i, name);
  }
});

test("without a PostHog key the build changes nothing and the pages load no script", () => {
  const root = siteCopy();
  try {
    for (const env of [{}, { POSTHOG_KEY: "", POSTHOG_HOST: "" }]) {
      assert.equal(build(root, env), null);
      for (const name of PAGES) {
        assert.equal(readFileSync(join(root, name), "utf8"), readFileSync(join(SITE, name), "utf8"));
      }
      assert.equal(existsSync(join(root, "vendor")), false);
    }
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test("with a key every page gets the consent script, told where the pinned, hash-checked PostHog is", () => {
  const root = siteCopy();
  try {
    const { lib, integrity } = build(root, ENV);
    const version = JSON.parse(readFileSync(join(SITE, "package.json"), "utf8")).dependencies["posthog-js"];
    assert.equal(lib, `/vendor/posthog-js-${version}.js`);
    assert.match(integrity, /^sha384-/);
    assert.ok(existsSync(join(root, lib)));
    for (const name of PAGES) {
      const html = readFileSync(join(root, name), "utf8");
      assert.ok(!html.includes(MARKER), name);
      const tags = html.match(/<script[^>]*>/g);
      assert.equal(tags.length, 1, name);
      assert.match(tags[0], /src="\/analytics\.js"/);
      assert.match(tags[0], new RegExp(`data-posthog-key="${KEY}"`));
      assert.match(tags[0], /data-posthog-host="https:\/\/eu\.i\.posthog\.com"/);
      assert.ok(tags[0].includes(`data-posthog-lib="${lib}"`) && tags[0].includes(`data-posthog-integrity="${integrity}"`));
    }
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test("a malformed key or host stops the build instead of deploying analytics that does nothing", () => {
  assert.equal(readAnalytics({}), null);
  for (const env of [
    { POSTHOG_KEY: "phx_nope", POSTHOG_HOST: "https://eu.i.posthog.com" },
    { POSTHOG_KEY: KEY },
    { POSTHOG_KEY: KEY, POSTHOG_HOST: "http://eu.i.posthog.com" },
    { POSTHOG_KEY: KEY, POSTHOG_HOST: "https://eu.i.posthog.com/ingest" },
    { POSTHOG_KEY: `${KEY}" onload="x`, POSTHOG_HOST: "https://eu.i.posthog.com" },
  ]) {
    assert.throws(() => readAnalytics(env), undefined, JSON.stringify(env));
  }
  assert.throws(() => withAnalytics("<head></head>", readAnalytics(ENV), { lib: "/v.js", integrity: "sha384-x" }));
});

test("the server's pages (sign-in, download, unsubscribe, the owner's list) never load analytics", async () => {
  const pages = [
    page(200, { title: "users", body: "<p>list</p>", wide: true }),
    await admin(get("/api/admin"), fakeDeps().deps),
    await unsubscribe(get("/api/unsubscribe?t=bad"), fakeDeps().deps),
  ];
  for (const r of pages) {
    const body = await r.text();
    assert.doesNotMatch(body, /<script|posthog|analytics/i);
    assert.match(r.headers.get("content-security-policy"), /default-src 'none'/);
    assert.doesNotMatch(r.headers.get("content-security-policy"), /script-src/);
  }
  for (const path of ["/api/admin", "/api/admin/", "/api/auth/callback", "/api"]) {
    assert.equal(excluded(path), true, path);
    const { doc, win } = fakePage({ path, stored: "yes" });
    assert.equal(start(doc, win), null);
    assert.equal(posthogScripts(doc).length, 0);
    assert.equal(banner(doc), null);
  }
  assert.equal(excluded("/"), false);
  assert.equal(excluded("/apiary"), false);
});

test("analytics is off until the visitor says OK: nothing loads, nothing is stored, the question is asked", () => {
  const { doc, win, store } = fakePage();
  start(doc, win);
  assert.equal(posthogScripts(doc).length, 0);
  assert.equal(win.posthog, undefined);
  assert.equal(store.size, 0);
  const b = banner(doc);
  assert.ok(b);
  assert.ok(b.text.includes(STRINGS.banner));
  assert.deepEqual(buttons(b).map((n) => n.textContent), [STRINGS.ok, STRINGS.no]);
});

test("No thanks keeps PostHog unloaded and is remembered; OK loads the pinned script and starts PostHog", () => {
  const no = fakePage();
  start(no.doc, no.win);
  buttons(banner(no.doc))[1].click();
  assert.equal(no.store.get("ferb-analytics"), "no");
  assert.equal(banner(no.doc), null);
  assert.equal(posthogScripts(no.doc).length, 0);
  const again = fakePage({ stored: "no" });
  start(again.doc, again.win);
  assert.equal(banner(again.doc), null);
  assert.equal(posthogScripts(again.doc).length, 0);

  const yes = fakePage();
  start(yes.doc, yes.win);
  buttons(banner(yes.doc))[0].click();
  assert.equal(yes.store.get("ferb-analytics"), "yes");
  const [script] = posthogScripts(yes.doc);
  assert.equal(script.src, "/vendor/posthog-js-1.0.0.js");
  assert.equal(script.integrity, "sha384-test");
  const { ph, calls } = fakePostHog();
  yes.win.posthog = ph;
  script.dispatch("load");
  assert.equal(calls[0][0], "init");
  assert.equal(calls[0][1], KEY);
  assert.equal(calls[0][2].api_host, "https://eu.i.posthog.com");
  assert.equal(banner(yes.doc), null);
});

test("Do Not Track and Global Privacy Control count as No thanks, even over an earlier OK", () => {
  for (const opts of [{ dnt: "1" }, { gpc: true }, { dnt: "1", stored: "yes" }, { gpc: true, stored: "yes" }]) {
    const { doc, win, store } = fakePage(opts);
    start(doc, win);
    assert.equal(banner(doc), null, JSON.stringify(opts));
    assert.equal(posthogScripts(doc).length, 0, JSON.stringify(opts));
    assert.equal(store.get("ferb-analytics"), opts.stored);
  }
});

test("an earlier OK loads PostHog at once; blocked storage still asks and still honours the answer", () => {
  const earlier = fakePage({ stored: "yes" });
  start(earlier.doc, earlier.win);
  assert.equal(banner(earlier.doc), null);
  assert.equal(posthogScripts(earlier.doc).length, 1);

  const blocked = fakePage({ storageThrows: true });
  start(blocked.doc, blocked.win);
  assert.equal(posthogScripts(blocked.doc).length, 0);
  buttons(banner(blocked.doc))[0].click();
  assert.equal(posthogScripts(blocked.doc).length, 1);
});

test("the privacy page's button asks again, and No thanks after OK opts PostHog out", () => {
  const { doc, win, store } = fakePage({ stored: "yes" });
  const controls = doc.body.appendChild(doc.createElement("p"));
  controls.setAttribute("data-analytics-controls", "");
  controls.hidden = true;
  const state = controls.appendChild(doc.createElement("span"));
  state.setAttribute("data-analytics-state", "");
  const change = controls.appendChild(doc.createElement("button"));
  change.setAttribute("data-analytics-choice", "");
  start(doc, win);
  assert.equal(controls.hidden, false);
  assert.equal(state.textContent, STRINGS.on);
  const { ph, calls } = fakePostHog();
  win.posthog = ph;
  posthogScripts(doc)[0].dispatch("load");

  change.click();
  buttons(banner(doc))[1].click();
  assert.equal(store.get("ferb-analytics"), "no");
  assert.deepEqual(calls.slice(1), [["stop_recording"], ["opt_out"], ["set_config", { disable_persistence: true }]]);
  assert.equal(state.textContent, STRINGS.off);

  change.click();
  buttons(banner(doc))[0].click();
  assert.deepEqual(calls.slice(-2), [["set_config", { disable_persistence: false }], ["opt_in"]]);
  assert.equal(posthogScripts(doc).length, 1);
});

test("Continue with Google is a named event, only after OK, and the link itself is left as it is", () => {
  const { doc, win } = fakePage({ stored: "yes" });
  const link = doc.body.appendChild(doc.createElement("a"));
  link.setAttribute("href", "/api/auth/google?file=FERB-AppleSilicon.dmg");
  const icon = link.appendChild(doc.createElement("svg"));
  start(doc, win);
  const { ph, calls } = fakePostHog();
  win.posthog = ph;
  posthogScripts(doc)[0].dispatch("load");
  icon.click();
  assert.deepEqual(calls.at(-1), ["capture", "continue with google clicked", { file: "FERB-AppleSilicon.dmg" }, { transport: "sendBeacon" }]);
  assert.equal(link.getAttribute("href"), "/api/auth/google?file=FERB-AppleSilicon.dmg");

  const off = fakePage({ stored: "no" });
  const plain = off.doc.body.appendChild(off.doc.createElement("a"));
  plain.setAttribute("href", "/api/auth/google?file=FERB-AppleSilicon.dmg");
  start(off.doc, off.win);
  const quiet = fakePostHog();
  off.win.posthog = quiet.ph;
  plain.click();
  assert.deepEqual(quiet.calls, []);
});

test("recordings mask every input, keep no request headers or bodies, and the cookie stays on this site", () => {
  const s = settings({ host: "https://eu.i.posthog.com" });
  assert.equal(s.session_recording.maskAllInputs, true);
  assert.equal(s.session_recording.recordHeaders, false);
  assert.equal(s.session_recording.recordBody, false);
  assert.equal(s.enable_recording_console_log, false);
  assert.equal(s.cross_subdomain_cookie, false);
  assert.equal(s.respect_dnt, true);
  assert.equal(s.person_profiles, "identified_only");
});

test("PostHog runs no code from its servers except the toolbar the owner opens; settings come as data", () => {
  const script = (src) => ({ src });
  assert.equal(allowExternalScript(script("https://eu-assets.i.posthog.com/array/phc_x/config.js")), null);
  assert.equal(allowExternalScript(script("https://eu-assets.i.posthog.com/static/recorder.js?v=1")), null);
  assert.equal(allowExternalScript(script("https://eu-assets.i.posthog.com/static/surveys.js")), null);
  assert.equal(allowExternalScript(script("not a url")), null);
  for (const src of ["https://eu-assets.i.posthog.com/static/toolbar.js?v=1&t=2", "https://eu-assets.i.posthog.com/static/1.447.10/toolbar.js"]) {
    assert.deepEqual(allowExternalScript(script(src)), script(src));
  }
  assert.equal(settings({ host: "https://eu.i.posthog.com" }).prepare_external_dependency_script, allowExternalScript);
});

test("addresses PostHog keeps lose sign-in codes, link tokens, PostHog ids and emails", () => {
  assert.equal(
    cleanUrl("https://ferb.kartikey.xyz/download/FERB-AppleSilicon.dmg?t=abc.def&utm_source=x"),
    "https://ferb.kartikey.xyz/download/FERB-AppleSilicon.dmg?utm_source=x",
  );
  assert.equal(
    cleanUrl("https://ferb.kartikey.xyz/api/auth/callback?code=4%2F0A&state=s&scope=email&authuser=0"),
    "https://ferb.kartikey.xyz/api/auth/callback?scope=email",
  );
  assert.equal(cleanUrl("https://ferb.kartikey.xyz/?Email=a%40b.c&ph=anon"), "https://ferb.kartikey.xyz/");
  assert.equal(cleanUrl("https://ferb.kartikey.xyz/#access_token=x"), "https://ferb.kartikey.xyz/");
  assert.equal(cleanUrl("https://ferb.kartikey.xyz/#download"), "https://ferb.kartikey.xyz/#download");
  assert.equal(cleanUrl("not a url"), "not a url");
  const event = settings({ host: "https://eu.i.posthog.com" }).before_send({
    event: "$snapshot",
    properties: {
      $current_url: "https://ferb.kartikey.xyz/?t=x",
      $snapshot_data: [{ type: 4, data: { href: "https://ferb.kartikey.xyz/?token=y" } }],
    },
    $set: { $referrer: "https://ferb.kartikey.xyz/api/auth/callback?code=z" },
  });
  assert.equal(event.properties.$current_url, "https://ferb.kartikey.xyz/");
  assert.equal(event.properties.$snapshot_data[0].data.href, "https://ferb.kartikey.xyz/");
  assert.equal(event.$set.$referrer, "https://ferb.kartikey.xyz/api/auth/callback");
});
