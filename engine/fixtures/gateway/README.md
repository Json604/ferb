# Fixtures: the gateway as the Rust port must reproduce it (ENG-3)

Made by the scripts here from the Python build (each writes the same bytes on every run):

    cd <repo> && uv run python rust/fixtures/gateway/make.py
    cd <repo> && uv run python rust/fixtures/gateway/make_hooks.py
    cd <repo> && uv run python rust/fixtures/gateway/make_locks.py

## scenarios.json (`make.py`)

71 scenarios of the real `ferb.daemon.gateway.Gateway`, each built as `ferb/daemon/areas/compose.py` builds it, on a
fresh store in `/tmp/ferbgw/<scenario>` (`FERB_HOME`) with `HOME` at `/tmp/ferbgw/user` (emptied per scenario).
make.py's folder is written `/FX` everywhere (as made and as the OS resolves it). FakeClock at 2026-01-01T09:00:00Z,
`SequentialIds("id")`, question ids `ap-a00000000001`..., the redactor with no exclusions, the Mac's running and
installed apps stubbed as none, the owner's Keychain a dict (`ferb.identity.helper.request` replaced, as
tests/support/hidden_input.py does).

The Rust test (`tests/scenarios.rs`) runs in a fresh folder of the same length under `/tmp` per run, removed when it
ends (FERB clips long lines, so a path of another length would clip them elsewhere). What FERB hashed with an absolute
path in it (P19's payload hash, an escape's exact-command key) is recorded in `hash_inputs` (the preimage, make.py's
folder written `<real>` as resolved and `<made>` as made) and hashed again with the run's folder.

Tools are `SpecCap` (tests/conftest.py's `FakeCap` shape, built from a JSON spec the Rust test builds the same way),
the real `owner.ask`, the real file tools (`fs_tools`; the Rust side uses `adapters::fs`) and the real timer tools
(`time_tools`; the Rust side uses `adapters::TimeTool`). A file's identity facts are `"<volatile>"` (`volatile`),
masked the same way by the Rust test.

A scenario: `{name, from (the guarding tests), tools, fs_tools?, time_tools?, files?, tasks?, requests?, known?,
standing?, route?, procs? (the shell processes' tasks), made? (paths the tree made, GATE-539), bus_redacts? (the
daemon's events redact each event, marks kept), canary? (a hidden value that must be in no stored byte), unordered?
(tables whose rows Python writes in set order), more_tables?, steps, tool_schemas, volatile, results, journal,
tables, hash_inputs?, page_amounts? (each fake page line's amounts as the browser tools read them), secrets? (the
owner-secret names' file and the Keychain at the end)}`.

Steps: `call` (tool, arguments, `answers` in order: `choice`, `pick`, `text`, `bad_text` (refused, the question stays
open), `deny`, `supersede`, `cancel` (the task's Stop), `leave` (the waiter goes); `page` (the page changes while FERB
asks), `take` (FERB.app's one fetch of the question's picture); `caller_left`; `cancel_running` (the task's Stop once
its tool runs); `retry_action: "$last"`), `ask_secret` / `ask_login_code` (the hidden inputs, with the call's `span`),
`stop`, `lift`, `cancel_tasks`, `forget_tasks`, `unattended`, `clock`, `reopen_noes`, `restart` (the ledger's
restart), `orphan` (a row a dead daemon left executing), `reconcile` (settled from evidence), `failed_act`, `probe`,
`destination`, `take_picture`, `approvals` (every question kept), `span` (a call's record a hidden input reads),
`username`, `page`.

Covered: admission, arguments, the declaration that only adds, boundaries, every need (pay, new recipient, outside
value, private data, URL and form sinks, protected reads and writes, terminal input with its one Return and the app's
Yes, account security and card forms, a tab chain's one payment and GATE-580's press-time re-check, a payment with
no amount, the browser's own page, pairing, permanent deletes counted first and what the tree made, the escape hatch
with the admin password), authorize, the ledger (with the real ledger across a restart and its reconcile), S1
copies, refused sealed fills, the hidden inputs (one-time, saved, by handle, a whole browser refused, the account,
login codes, a code for an approved payment, after a refused fill, unattended, left), pay pictures (taken once,
never stored), probes, the shell proxy's destinations, the timer tools, FERB's own words kept from a second
redaction (the `Redacted` mark), `_known_from`, a `describe` that raises, a cancelled call's measured facts.

## hooks.json (`make_hooks.py`)

25 cases of the real `ferb.daemon.call_hooks.CallHooks` driven op by op over recording fakes of every part it drives
(the disturbance sentinel, the live previews, the evidence store, the change record, the image sanitizer, the
app-window picture and its masking, the shadow advisor, the event bus). Each case records every call a fake received,
in order, each op's result, the events and the journaled problems. `HOME` is `/HOME/user` and the workspace
`/HOME/ferb/workspace`: paths are text there, nothing is read or written.

## locks.json (`make_locks.py`)

7 scenarios of the real `ferb.act.locks.ResourceLocks` (the gateway's locks): calls holding keys (reading or acting),
a call queued behind a holder, a release that wakes it, a cancelled waiter, and the live preview's `try_hold` with and
without `during_reads` (DMN-403, DMN-406): what each look answered and who holds each key after. The cases are
tests/test_hangs_and_previews.py's and tests/test_task_preview.py's lock cases plus edges read from the code (a lock
just handed to a waking worker is busy, several keys are all taken or none, a read is looked at while the other keys
are taken, an unknown kind). Replayed by `tests/locks_python.rs` on a single-threaded runtime.
