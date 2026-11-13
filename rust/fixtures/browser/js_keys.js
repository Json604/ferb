// The extension's key table, for rust/fixtures/browser/hands.py (ENG-3): loads the unchanged
// extensions/chromium/protocol.js under node and runs its own keyEvent and entersText on each spec.
//
//     node rust/fixtures/browser/js_keys.js <protocol.js> <specs.json>   (prints the results as JSON)
//
// A spec the extension's code throws on is recorded as {"throws": <the error's name>}.
"use strict";

const fs = require("fs");
const p = require(process.argv[2]);
const specs = JSON.parse(fs.readFileSync(process.argv[3], "utf8"));

const out = specs.map((spec) => {
  try {
    const k = p.keyEvent(spec);
    return { spec, event: k === null ? null : JSON.parse(JSON.stringify(k)), enters: p.entersText(spec) };
  } catch (e) {
    return { spec, throws: e.name };
  }
});
process.stdout.write(JSON.stringify(out));
