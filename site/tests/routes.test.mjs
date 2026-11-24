// The site fits Vercel's Hobby plan (at most 12 functions) and every endpoint still answers at its address (REL-451).
import { test } from "node:test";
import assert from "node:assert/strict";
import { readdirSync, statSync } from "node:fs";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { APP_ROUTES, AUTH_ROUTES, dispatch, stepOf } from "../api/_lib/routes.js";

function functions(dir) {
  return readdirSync(dir).flatMap((name) => {
    const path = join(dir, name);
    if (statSync(path).isDirectory()) return name.startsWith("_") ? [] : functions(path);
    return name.endsWith(".js") ? [path] : [];
  });
}

// decision: REL-451
test("the site deploys at most 12 functions", () => {
  const all = functions(fileURLToPath(new URL("../api", import.meta.url)));
  assert.ok(all.length > 0 && all.length <= 12, all.join(", "));
});

// decision: REL-451
test("each step is found by its segment, its path or a rewritten address, and the segment is dropped", () => {
  assert.deepEqual(stepOf(new Request("https://f.example/api/app/callback?step=callback&code=c&state=s")), {
    step: "callback",
    url: "https://f.example/api/app/callback?code=c&state=s",
  });
  assert.equal(stepOf(new Request("https://f.example/api/app/token")).step, "token");
  assert.equal(stepOf(new Request("https://f.example/.well-known/assetlinks.json")).step, "assetlinks");
  assert.equal(stepOf(new Request("https://f.example/app/callback?code=x")).step, "landing");
});

// decision: REL-451
test("every route names a handler for its methods; an unknown step or method is refused", async () => {
  for (const routes of [APP_ROUTES, AUTH_ROUTES])
    for (const [step, byMethod] of Object.entries(routes))
      for (const [method, [handler, needs]] of Object.entries(byMethod)) {
        assert.equal(typeof handler, "function", `${step} ${method}`);
        assert.equal(typeof needs, "function", `${step} ${method}`);
      }
  assert.deepEqual(Object.keys(APP_ROUTES).sort(), [
    "assertion", "assetlinks", "callback", "delete", "landing", "me", "ready", "signout", "start", "token",
  ]);
  assert.equal((await dispatch(APP_ROUTES, new Request("https://f.example/api/app/nope"))).status, 404);
  assert.equal((await dispatch(APP_ROUTES, new Request("https://f.example/api/app/constructor"))).status, 404);
  const wrong = await dispatch(APP_ROUTES, new Request("https://f.example/api/app/token"));
  assert.equal(wrong.status, 405);
  assert.equal(wrong.headers.get("allow"), "POST");
});
