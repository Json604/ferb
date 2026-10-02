# 0.19.0 recovery contracts — `astra/version`

This change follows the owner's failed WhatsApp search and Zomato login/order attempts on 2026-10-02.
The observed symptoms exposed reusable driver and orchestration defects; no site-specific click rules or
real account credentials are added. The branch includes the recovered 0.18.0 cold-start documentation.

## What changed

| Failure | Current contract | Evidence |
|---|---|---|
| A busy DOM snapshot prevented trying AX | Every browser snapshot attempts Chromium AX. Roles, names and order come from AX; values, secret detection, forms, geometry and action references come from the isolated DOM walker. DOM-only controls remain available. | Actual extension JS tests; isolated Chrome lab |
| Frames were positioned by URL/name guesses | CDP frame/owner identities bind to DOM references, child debugger sessions are tracked, and owner positions are refreshed before input. Embedded-frame text reaches the worker. | Duplicate-URL unit fixture; same-process, cross-site and nested-frame Chrome tests |
| A trusted click silently became synthetic input | Trusted input either dispatches or reports its failure. Page mode is explicit. Covered/disabled targets and unconfirmed focus are refused. | Actual extension JS tests; trusted-event Chrome assertions |
| A visible ADD control had no semantic target | `browser.screenshot` captures the exact task tab; `browser.point` resolves a visible point into an element id without clicking. `browser.act` then applies normal policy/account checks. The same tools exist for FERB's identity. | Custom div with no DOM/ARIA control role, clicked through the real Chrome lab |
| Login could not ask for an owner's OTP | `browser.login_code` asks through hidden CLI input, displaying the HTTPS frame origin. Only a selected sensitive, non-password field can receive it. The code is sealed to that command/origin; a changed origin or field refuses it. The model never receives the code. | CLI test; fake-extension integration verifies sealed transport, origin-change refusal and absence from model results/database dump |
| Repeated input looked like success | Ledger completion means the tool completed, not that an order/login succeeded. `ledger_done` requires a fresh verifier read. No success conditions means `verified=false`. The verifier sees the requested goal. | Verification and acceptance tests |
| New snapshot ids hid repeated actions | Loop digests normalize snapshot generation bookkeeping and ignore explanatory `why` text. An unchanged consequential action stops after one retry. | Guard regressions, including changing state that must remain progress |
| A search-labelled AX button accepted a useless value | Raw AX role determines whether a field is editable. Set-value success needs matching readback; focus can belong to a descendant, but not an unrelated control. AX typing checks focus and measures state after focusing. | Mac fake-driver regressions; real messaging-app acceptance pending |
| A failed input mechanism stayed disabled | AX keyboard attempts use the same miss limit as other input mechanisms; learned failures are eligible for re-probing after an hour. | Profile persistence and retry tests |
| A community preview containing “Buy” triggered payment approval | Label floors inspect the control name before comma-separated descriptive text. Explicit Send/Pay/Delete/Video call names keep their floors. Scroll/edit/select do not borrow a press label's floor. | Policy/contact tests and the supplied preview shape |
| A paired, sleeping browser was declared unavailable | Availability retains browser tools when the profile wake service can open a paired profile. The requested profile still constrains selection. | Availability and existing wake/profile tests |
| “Stop and retry” only produced a new attempt | Structured task control identifies a task in the current chat, cancels its runtime and tool calls/holds, and waits before replacement. Follow-ups preserve original request words, profile context and scoped approvals. Unknown action outcomes block retry. | Stop, continuation and pending-secret cancellation tests |
| Reconnect treated any completed command handler as successful | Protocol 4 reports `done`, `failed` or `unknown`; timeout/cancellation can leave an action unknown. Reconnect never resends it. | JS outcome and bridge recovery tests |

## Validation and installation

Use `.venv/bin/python -m pytest -q`, `.venv/bin/python -m mypy ferb/core`, and the isolated browser lab:
`.venv/bin/python -m pytest -m real tests/test_browser_lab_real.py -q`.
The lab uses a throwaway Chrome directory and test daemon. It never uses the owner's browser profiles.
Full-suite and final lab results are recorded in the branch handoff/PROGRESS.md.

Runtime **0.19.0** requires extension **0.8.0 / protocol 4**. The owner runs `ferb setup --update` and reloads
the unpacked extension in each paired profile when ready to try this branch. This development session does
not update/restart the installed daemon, reload owner profiles, place an order or send an OTP request.

## Remaining limits and owner acceptance

- The supplied Zomato and WhatsApp flows have not been rerun against live accounts. Lab success proves these
  contracts, not that those apps expose every control or accept background input. Run the relevant checks in
  `OWNER_ACCEPTANCE.md`, including observing whether the owner's screen moves.
- AX depends on what Chromium exposes. Closed shadow roots, virtualized controls, scaled frames, stale references
  or debugger attachment failures can still produce a diagnostic/refusal. DOM binding uses temporary random
  attributes. AX enumeration may be slower on large pages; snapshots retain a 400-element cap.
- Screenshots refuse sensitive/truncated field inventories. Point resolution expires after 30 seconds and a new
  snapshot/navigation invalidates it. An app-window screenshot is not a substitute for the correct browser tab.
- OTP support is a hidden interactive-CLI handoff for recognizable code fields. Passwords, CAPTCHAs, unusual
  multi-field code widgets and expired codes can still require owner intervention. It does not prove an OTP was
  sent, and it never reads the owner's SMS or email inbox.
- Stop cannot undo input already delivered to an app. A cancelled in-flight action is conservatively unknown;
  fresh evidence is required before retry. Existing account checks, pay/destroy approvals and undo holds remain.
- Follow-up context is retained within the running daemon/chat session; approvals are not restored after daemon
  restart. Successful actions are not replayed to recover state. Loop detection is a state heuristic, not a
  semantic proof that a task is progressing.
- Label floors are a conservative backstop, not a language parser. The model's effect declaration remains required;
  comma-separated content descriptions do not establish an action's meaning.
- Procedure memory and broader owner acceptance remain future work. This patch does not claim a whole-repository
  correctness proof or an improved real-task success rate before those tasks are measured again.
