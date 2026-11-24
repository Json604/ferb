// decision: REL-433
// decision: REL-436
import assert from "node:assert/strict";
import { test } from "node:test";
import { unsubscribe } from "../api/_lib/handlers.js";
import { downloadToken, unsubscribeToken } from "../api/_lib/tokens.js";
import { fakeDeps, get, NOW, SECRET } from "./support.mjs";

function withUser() {
  const f = fakeDeps();
  f.users.set("ada@example.com", { email: "ada@example.com", name: "Ada", first_signin_at: new Date(), last_download_at: null, unsubscribed: false });
  return f;
}

test("the email's link unsubscribes in one click", async () => {
  const { deps, users } = withUser();
  const r = await unsubscribe(get(`/api/unsubscribe?t=${unsubscribeToken(SECRET, "ada@example.com")}`), deps);
  assert.equal(r.status, 200);
  assert.equal(users.get("ada@example.com").unsubscribed, true);
});

test("the mail app's one-click POST (RFC 8058) unsubscribes", async () => {
  const { deps, users } = withUser();
  const r = await unsubscribe(
    new Request(`https://ferb.example.com/api/unsubscribe?t=${unsubscribeToken(SECRET, "ada@example.com")}`, {
      method: "POST",
      headers: { "content-type": "application/x-www-form-urlencoded" },
      body: "List-Unsubscribe=One-Click",
    }),
    deps,
  );
  assert.equal(r.status, 200);
  assert.equal(users.get("ada@example.com").unsubscribed, true);
});

test("a missing, forged or other-purpose token unsubscribes nobody", async () => {
  const { deps, users } = withUser();
  const other = unsubscribeToken("another-secret-0123456789abcdef01234567", "ada@example.com");
  for (const t of ["", "garbage", other, downloadToken(SECRET, "ada@example.com", "FERB-AppleSilicon.dmg", NOW)]) {
    const r = await unsubscribe(get(`/api/unsubscribe?t=${t}`), deps);
    assert.equal(r.status, 400, t);
  }
  assert.equal(users.get("ada@example.com").unsubscribed, false);
});

test("an unsubscribe that couldn't be saved says so, so the person can try again", async () => {
  const { deps } = fakeDeps({ failStore: true });
  const r = await unsubscribe(get(`/api/unsubscribe?t=${unsubscribeToken(SECRET, "ada@example.com")}`), deps);
  assert.equal(r.status, 503);
});
