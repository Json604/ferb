# FERB 0.26 capability ledger

A row marked **verified** means the named operation and evidence passed, not that every workflow in that app is
proved. **Offline-tested only** means fixtures/contracts pass. **Untested** means no safe current live verdict.
**Not supported** means a route is deliberately absent or unavailable. Background status is similarly scoped:
“background with caveats” never promises no disturbance in an untested app.

Current Mac app inventory and per-app measurements: [app profiles](evidence/app-profiles-0.26.json).
All nine requested apps are installed. Static scripting dictionaries establish available vocabulary, not Automation
permission or harmless behavior. No personal account, profile or document was used in the disposable checks.

Real end-to-end RPC/model check: [request pipeline receipt](evidence/request-pipeline-live-0.26.json).
The real Codex planner, worker and checker preserved a 9,627-byte pasted Unicode source exactly in a disposable
file, verified it and exported the planner and sentinel records. 221 samples observed zero changes/errors;
full-screen state was unavailable and the login window was frontmost. This proves that pipeline on a test request,
not study-guide depth or active-owner-work acceptance.

## Native apps

Read/type/click/scroll/menu support is per control and mechanism. A delivered PID event is not a verified edit.
Unsupported or ambiguous targets return facts; there is no automatic foreground replay. A global keyboard/mouse
route needs focus and explicit permission. The table does not infer that such a route is the only possible one.

| App / operation | Status | Background status | Evidence and limits |
|---|---|---|---|
| TextEdit: read, set whole value, edit selected range | **Verified** | **True background for this fixture** | [Live receipt](evidence/native-live-0.26.json): isolated PID/temp document, independent readback, 19 samples, zero changes/errors/missing fields; owner’s front app was already full-screen |
| TextEdit: click, scroll, menus, PID typing | Offline-tested only | Background with caveats: AX/PID controls vary | `test_background_input.py`, `test_mac_shell.py`, `test_native_hardening.py`; live receipt does not cover these mechanisms |
| Finder: metadata read via scripting | Untested (grant unavailable) | Background with caveats: non-activating script requires Automation grant | Live preflight returned `not_determined`; no permission dialog was raised; installed Finder dictionary in app profiles |
| Finder: read/type/click/scroll/menus | Untested on 0.26 | Background with caveats: exact target AX/PID route must be measured | App inventory only; fixture regressions cover mechanism contracts |
| Notes: read/type/click/scroll/menus | Untested | Background with caveats: AX and scripting dictionary exist; account data and grants not tested | App profiles: Notes dictionary; app was not launched |
| Calendar: read/type/click/scroll/menus | Untested | Background with caveats: AX and scripting require measured grants/routes | App profiles: Calendar dictionary; no calendar/account accessed |
| Reminders: read/type/click/scroll/menus | Untested | Background with caveats: dictionary/AX availability is not outcome proof | App profiles; no reminder account accessed |
| Mail: read/type/click/scroll/menus | Untested | Background with caveats: scripting is available in principle; account/grant not tested | App profiles; no messages read or sent |
| WhatsApp: read/type/click/scroll/menus | Untested on 0.26 | Background with caveats: no installed scripting dictionary; AX/PID need per-control checks | Historical owner-confirmed scroll is in Git acceptance history; it is not acceptance of changed 0.26 mechanisms |
| VS Code: read/type/click/scroll/menus | Untested | Background with caveats: file/CLI routes available; live UI editing not exercised | App profiles; owner’s running editor was observed only by sentinel metadata |
| System Settings: read/type/click/scroll/menus | Untested | Background with caveats; protected controls may need focus/system approval | Installed dictionary inventoried; no settings changed or app launched |
| Calculator: hidden AX control interaction | Untested / target evidence unavailable | Background with caveats; hidden application did not expose a unique usable control | Disposable probe refused duplicate/non-window AX references; no guessed click; receipt and regression in native evidence |
| Native target on another Space | Untested | Background with caveats: AX target addressing exists; no private Space manipulation | No owner windows were moved to manufacture a test; acceptance task below |
| Full-screen VS Code frontmost during TextEdit edit | **Verified** for measured fixture | **True background for this fixture** | Native receipt baseline `fullscreen=true`, 19 samples, no transitions |
| Target app full-screen | Untested | Background with caveats: visibility and AX/screenshot availability vary | No desktop rearrangement was made; owner acceptance required |
| Minimized native target | Untested / fixture window unavailable | Background with caveats: exact AX window must first be reachable | [Measured refusal](evidence/minimized-native-live-0.26.json): hidden disposable TextEdit window unavailable; no minimize/edit attempted; 33 samples, zero changes/errors; full-screen attribute unavailable |
| Global HID input, permission prompts, unsupported secure controls | Not supported as unattended background route | **Needs focus if chosen** | Explicit foreground mechanism only; ask now / idle / skip; no automatic restoration |

## Browser, files and agent services

| Capability | Status | Background status | Evidence and limits |
|---|---|---|---|
| Disposable Chrome: local upload to top frame, same-origin iframe and OOPIF | **Verified** | **True background for measured fields; full-screen read unavailable** | [Browser receipt](evidence/browser-live-0.26.json): native choosers intercepted in all three frames; selected SHA-256 matches local bytes, long snapshot tail available; 20 samples, zero changes/errors; login window frontmost; AXFullScreen unavailable |
| FERB’s profile: navigate/read/type/click/scroll/dialogs | Offline-tested only on 0.26 | Background with caveats: inactive/minimized task window; actual provider sites untested | Browser/extension suites; disposable CDP live checks. Identity profile was not opened |
| Owner browser profiles | 0.26.0 full-screen Brave creation **failed owner acceptance**; 0.26.1 route regression-tested, live owner rerun pending | Background with caveats: explicit profile and tab ownership; never inferred from history | Owner trace `68e70057`: first new-window open disturbed full-screen/focus. Patch refuses cold creation and supports exact existing windows; no personal profile used for patch tests |
| Gmail / Drive / Calendar / Forms via browser | Offline-tested only for primitives; end-to-end untested | Background with caveats: exact source/account evidence required | Local iframe upload proves transport, not Google service workflows; owner-run disposable Form test required |
| Sending email from FERB’s account | Untested on 0.26 | Background with caveats: inspect/type/click workflow | Hardcoded Gmail sender removed; no real email sent; final receipt/content must be checked |
| File read/write plus real worker/checker path | **Verified** for exact disposable source copy; edit/list/trash offline-tested | True background for file-level operations | Full UTF-8 pages, identity metadata, unbounded write content, unique edit/temp/target tests; literal path behavior changed |
| Shell and coding sessions | Offline-tested only on 0.26 | Background with caveats: PTY itself is background; arbitrary commands can activate apps | Managed-process, log paging, cancellation and runtime tests; sentinel covers tool calls and subprocess lifetime |
| Memory/intake receipts/history | Offline-tested only | True background | Full-request, durable-memory, paged source, conflicting-key and context-budget regressions |
| Reminders and scheduled work | Offline-tested only | True background for computation; notifications are visible | Full message/no one-year cap, explicit local-time requirement, positive recurrence, durable delivery and stop tests |
| Browser/native target recording | Offline-tested only on 0.26 | Background with caveats: target must remain capturable | Existing recording tests, exact target binding/partial failures; no new live video acceptance claimed |
| OTP from owner input / FERB inbox | Offline-tested only | Background with caveats: exact task/account/origin binding | Opaque candidate choice and sealed fill tests; unread numeric candidates only. Subject/body withheld to protect unsupported secret formats; insufficient identity evidence requires asking or another explicit route. No real mailbox read or login attempted |
| Stop, resume, chat reconnect | Offline-tested only | True background for orchestration | Token revocation, task stop, persistent stop marker and reconnect tests; no replay of unknown effect |
| Disturbance sentinel and trace export | **Verified** for disposable sessions; offline-tested for action/task integration | True background observation | Native/browser receipts, `test_disturbance.py`, outcome-export and cleanup integration tests |
| Future Android / replacement UI client | Not built | Not applicable | [RPC contract](RPC.md); local auth/stream/questions/stop/results remain the integration seam |
| Verified-approach learning loop / expanded Jev decisions | Not built intentionally | Not applicable | `Supervisor._approach`, typed Jev registry and tests retained; export owner verdict remains null |

The conservative limitation is deliberate: no account-based workflow is labelled live-verified from a local HTML
fixture, a dictionary listing or a successful tool return. Run the [acceptance checklist](OWNER_ACCEPTANCE.md) to
replace these statuses with dated task IDs and your verdict. Block 1’s implementation is reviewable; broad personal
workflow acceptance remains open until those checks pass with zero unauthorized disturbances.
