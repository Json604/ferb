// The extension's side of the crypto, for rust/fixtures/browser/make.py (ENG-3): loads the unchanged
// extensions/chromium/protocol.js under node and runs its own functions on the cases make.py passes in.
//
//     node rust/fixtures/browser/js_vectors.js <protocol.js> <cases.json>   (prints the results as JSON)
//
// The two HMACs are built the way background.js builds them (its `hmac` and `post`, which protocol.js doesn't export):
// WebCrypto HMAC-SHA256 over the UTF-8 text, base64. A seal's 12-byte IV comes from crypto.getRandomValues, so the
// driver hands out each case's fixed IV there for that one call; everything else is protocol.js's own code.
"use strict";

const fs = require("fs");

const random = crypto.getRandomValues.bind(crypto);
let fixed = null;
crypto.getRandomValues = (a) => {
  if (fixed) {
    a.set(fixed.subarray(0, a.length));
    return a;
  }
  return random(a);
};

const p = require(process.argv[2]);
const cases = JSON.parse(fs.readFileSync(process.argv[3], "utf8"));

const b64d = (s) => Uint8Array.from(atob(s), (c) => c.charCodeAt(0));
const b64e = (buf) => Buffer.from(new Uint8Array(buf)).toString("base64");

async function hmac(keyB64, text) {           // background.js `hmac`
  const k = await crypto.subtle.importKey("raw", b64d(keyB64), { name: "HMAC", hash: "SHA-256" }, false, ["sign"]);
  return b64e(await crypto.subtle.sign("HMAC", k, new TextEncoder().encode(text)));
}

async function main() {
  const out = { protocol: p.FERB_PROTOCOL, ops: p.OPS, read_ops: [...p.READ_OPS].sort(), daemon_signed: [...p.DAEMON_SIGNED].sort(),
                upload_part: p.UPLOAD_PART, download_part: p.DOWNLOAD_PART, daemon: [], extension: [], seal: [], unseal: [],
                enters_text: [] };
  for (const c of cases.daemon) {            // the daemon's welcome / cmd mac, as the extension checks it
    out.daemon.push({ text: p.daemonMacText(c.nonce, c.seq, c.type, c.body),
                      mac: await hmac(c.key, p.daemonMacText(c.nonce, c.seq, c.type, c.body)) });
  }
  for (const c of cases.extension) {         // background.js post(): body_json = JSON.stringify(body)
    const bodyJson = JSON.stringify(c.body);
    const text = `${c.install_id}|${c.seq}|${c.type}|${bodyJson}`;
    out.extension.push({ body_json: bodyJson, mac: await hmac(c.key, text) });
  }
  for (const c of cases.seal) {
    fixed = b64d(c.iv);
    try {
      out.seal.push(await p.seal(c.key, c.text, c.command, c.origin));
    } finally {
      fixed = null;
    }
  }
  for (const c of cases.unseal) {
    try {
      out.unseal.push({ ok: true, text: await p.unseal(c.key, c.sealed, c.command, c.origin) });
    } catch (_) {
      out.unseal.push({ ok: false });
    }
  }
  for (const spec of cases.enters_text) out.enters_text.push(p.entersText(spec));
  process.stdout.write(JSON.stringify(out));
}

main().catch((e) => {
  process.stderr.write(String(e && e.stack || e));
  process.exit(1);
});
