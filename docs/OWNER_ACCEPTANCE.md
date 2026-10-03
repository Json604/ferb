# Owner acceptance

Offline tests establish implementation contracts. Only the owner can establish whether a real task was correct and
whether their desktop, cursor or typing was disturbed. Do not mark a whole release accepted from one successful gesture.

## 0.24.0 — pending installation and live observation

Repository work only. The installed daemon has not been updated for this release. The owner normally runs
`ferb setup --update` when existing work can be interrupted, then checks `ferb doctor` and `ferb browsers` for the
matching build and extension 0.11.0. Perform one requested check at a time and record the owner's answer here.

| Check | Expected evidence | Owner verdict |
|---|---|---|
| Join the requested class and let it run | Correct class/account; explicit retained tab; cleanup detaches controls; class remains open after final reply | Pending |
| Keep working in another app during a background task | No independent FERB cursor, typing-focus or Space change; distinguish owner movement | Pending |
| Inspect a current list, then ask for it again | Fresh observation for a current-state request; recalled history labelled when used | Pending |
| Read official SST announcements for a date range | Correct sources, date coverage and honest gaps; no claim that a partial view covers everything | Pending |
| Use scaler Gmail then the requested coding session | Correct email/requirements, session/model, actual process completion and files; no unrequested submission | Pending |
| A worker encounters an unavailable method | Findings return to orchestrator; another supported method or a factual blocker; no routine approval loop | Pending |
| Retained/borrowed tab after reconnect or extension update | Page stays open; no leftover debugger state or page navigation from cleanup | Pending |
| Voice question answered with a correction/free text | Full intended answer reaches the pending question; OTP stays private | Pending |
| Forget memory sourced from today's activity | Memory source and matching daily-copy data are removed from retrieval | Pending |
| Real provider account and delivery | FERB Gmail/YouTube sign-in, inbox retrieval/OTP and an explicitly authorized disposable email delivery | Pending; local automated fixtures do not prove provider behavior |
| Late/repeatedly failing schedule | Full instructions and failure facts reach the model; no silent automatic skip/pause | Pending |

Use disposable files/pages for development checks. Everyday personal-app tasks run only when requested by the owner.
Do not send a real message or submit/pay merely to test authorization. For “prepare” versus “send”, use an owner-chosen
disposable destination and artifact. A model's answer alone does not prove it followed the request.

`ferb watch --dry-run` lists interaction checks. Explicit `ferb watch --mac` / `--browser` records owner verdicts beside
platform observations. `ferb trace TASK --full` / `--json` supplies recorded execution evidence. A task failure must
remain visible even if the owner confirms part of the requested behavior.

## Recorded observations before 0.24.0

These are historical checkpoints, not acceptance of this release. Detailed old reports are available at commit
`1367722` under `docs/`.

| Date / build | Owner observation and supporting evidence | Limits |
|---|---|---|
| 2026-10-02, early watch runs | Owner reported no disturbance for 25 disposable Mac/browser interaction steps | Specific tested setup and primitives |
| 2026-10-03, 0.21 branch WhatsApp scroll | First wheel movement worked without disturbance; owner rejected a slower batch. Viewport-sized scrolling then worked quickly without disturbance. A 5% step was confirmed very small | Owner explicitly requested WhatsApp; no claim about all gestures/apps |
| 2026-10-03, installed Brave YouTube | Owner confirmed the last available comment loaded. Independent read matched the final comment among 454 loaded top-level nodes | Current sort/loaded comments; not every reply. Checker incorrectly failed that historical run; no separate disturbance verdict |
| 2026-10-03, 0.22.1 native Chrome click | Owner: “No disturbance—I kept working normally.” Disposable receiving page recorded one trusted event. A corrected repeat reached exact (20,30) on another Space with unchanged cursor/front PID | Corrected-repeat owner verdict was not recorded; does not establish other apps/gestures |
| 2026-10-03, managed extension rollout | Owner authorized scaler repair and migration; scaler, FERB Chrome and Brave connected from the managed folder on 0.10.0. Owner completed Brave's setup-page step | Installation/connection evidence, not task acceptance. A closed Chrome was correctly disconnected |
| 2026-10-03, Coursera/FMI Claude run on 0.23 | Owner: “it didnt disturb me at all.” Saved sources, eight reminders, completed Claude session, output files and calculation checks supported the work | Reporting/process-monitoring defects still required fixes; no native pointer calls in that run |
| 2026-10-03, search on 0.23 | Owner reported unexpected focus to VS Code; journal recorded Brave → VS Code through stale browser restoration | A failure despite the earlier successful run; restoration removed in 0.23.2, live recheck not recorded |
| 2026-10-03, calendar/WhatsApp on 0.23.2 | Personal calendar evidence found three meetings but checker rejected unreadable account extraction. WhatsApp checks found useful items but incomplete source coverage | 0.23.3 added recorded evidence/checker navigation/recovery; comprehensive owner acceptance not recorded |
| 2026-10-03, Scaler CV class | Worker joined “Session 13 — Vision Foundation Models”; no worker close action. Task cleanup then closed its tab. Repeat claimed it stayed running and cleanup closed it again | Confirmed lifecycle defect. 0.24 adds retained-tab handoff and cleanup reporting; live class check pending |

The class task roots were `330f9b4445724de7a843ac6f1d1d6fc6` and `07ed78e64a3c41c38810e97f22a6226d`.
Keep future entries concise: date/build, task, observed outcome, owner verdict and what remains unverified.
