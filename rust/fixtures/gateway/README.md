# Fixtures: the gateway as the Rust port must reproduce it (ENG-3)

Made by `make.py` from the Python build:

    cd <repo> && uv run python rust/fixtures/gateway/make.py

`scenarios.json`: 45 scenarios of the real `ferb.daemon.gateway.Gateway`, each built as
`ferb/daemon/areas/compose.py` builds it, on a fresh store in `/tmp/ferbgw/<scenario>` (`FERB_HOME`) with `HOME` at
`/tmp/ferbgw/user` (emptied per scenario). One fixed folder, written `/FX`: a protected path's question key is a hash of
the absolute path, so the Rust test runs in the same folder (serialized by `/tmp/ferbgw.lock`). FakeClock at
2026-01-01T09:00:00Z, `SequentialIds("id")`, question ids `ap-a00000000001`..., the redactor with no exclusions, the
Mac's running and installed apps stubbed as none. Two runs give identical bytes.

Tools are `SpecCap` (tests/conftest.py's `FakeCap` shape, built from a JSON spec the Rust test builds the same way),
the real `owner.ask`, and in five scenarios the real file tools (`fs_tools`; the Rust side uses `adapters::fs`).
A file's identity facts (inode, device, times, owner, recovery copy identity) are `"<volatile>"` (`volatile`), masked
the same way by the Rust test (`masked`), in replies, events, JSON inside table cells and the recovery columns.

## A scenario

`{name, from (the guarding tests), tools, fs_tools?, files?, tasks?, requests?, known?, standing?, route?, steps,
tool_schemas, volatile, results, journal, tables}`.

Steps: `call` (tool, arguments, `answers` for each question in order: `choice`, `pick` (1-based), `text`, `deny`,
`supersede`, `cancel` (the task's Stop), `leave` (the caller gives up); `caller_left` for a caller gone by the end;
`retry_action: "$last"` is the one FERB's last refusal returned), `stop`, `lift`, `cancel_tasks`, `forget_tasks`,
`unattended`, `clock`, `reopen_noes`, `restart` (the ledger's restart), `failed_act`.

Each call's result: `asked` (every question's whole Approval when it appeared), `reply` (`content`, `is_error`,
`footer`) or `raised: "CancelledError"`, `events` (every published topic and data). At the end: `journal`
(`journal_problem`) and the tables spans, confirmations, action_ledger, known_destinations, standing_permissions,
tasks, recovery.

Covered stages: admission (stop, cancel, forget, checker read-only, lookups and their cap), arguments (invalid,
unknown tool, notes popped), the declaration that only adds, boundaries (card typed, an address with no site, FERB's
own state, P19 repeat, the undelivered result), needs (pay, new recipient, outside value, private data to a new
place, the URL and form sinks, protected read once per tree and write per call, terminal input and the app's Yes,
account security and card forms from facts, a tab chain's one payment, a payment with no amount sent back once),
authorize (Yes, No, skipped, already No, reopened No, the 30-minute window and what it never covers, standing
scope, a fresh need, unattended, superseded, stopped while asking, a caller that left), the ledger (done, failed
before acting, unknown, the retry question and its No), S1 copies on overwrite, GATE-562's refused sealed fill.

Not covered here (the Rust code exists but no Python run checks it): the escape hatch, the terminal input's one
Return (`armed`), GATE-580's press re-check, pay pictures, probes, `destination_allowed`, bulk delete counts,
GATE-539, the browser-page and pairing questions, hooks.
