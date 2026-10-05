# Owner acceptance

Offline tests establish implementation contracts. Only the owner can establish whether a real task was correct and
whether their desktop, cursor or typing was disturbed. Do not mark a whole release accepted from one successful gesture.

## 0.25.0 — checkout complete, live recording check pending

Includes the pending 0.24.1 recovery changes below. No installed daemon or personal browser was updated during
implementation. New browser recording needs extension **0.12.0 / protocol 7**. This session reports Screen Recording
access unavailable, and even a disposable headless Chrome preflight exits with code -6 before answering its debug
pipe. No successful live recording/undisturbed-desktop result is claimed from this environment.

Run one check from a normal terminal in this checkout; no installation is needed for this test:

```sh
uv run --no-sync pytest -q -s -m real tests/test_recording_real.py
```

The test asks when you are ready, then starts after three seconds. Keep typing/working in another app. It creates
only a disposable Chrome profile, local test server, isolated daemon and two pages. It records both targets while
changing READY → COMPLETE, then closes its own resources. It prints the output folder containing two MP4s,
receipts and contact sheets. The main installed daemon and personal profiles are untouched.

| Check | Required evidence | Owner verdict |
|---|---|---|
| Two inactive/minimized tabs recorded while the owner works elsewhere | Each decoded video shows its own ALPHA/BETA page changing READY → COMPLETE; no unrelated desktop content | Pending |
| Cursor, keyboard focus and Space behavior | Owner reports whether typing/focus/desktop was disturbed; front-app sampling supplements that observation | Pending |
| Native window recording across window focus changes | Same observed window/PID is captured; owner confirms no disturbance; requires Screen Recording permission | Pending |
| Task-wide foreground direction across recovery | Explicit owner direction survives retries; no repeated per-step question; later background correction wins | Pending real-model check |

After the test, inspect the actual videos/contact sheets and report your observation. Automated file/timing/pixel
regressions do not substitute for this verdict. When current FERB work can be interrupted, `ferb setup --update`
installs the combined 0.25.0 checkout; verify extension 0.12.0 / protocol 7 before browser recording.

## 0.24.1 recovery changes — included in pending 0.25.0

The original recovery patch targeted extension 0.11.1 / protocol 6; install the combined version above instead. An already-running older extension cannot use its replacement updater until it has
reloaded; if it still reports an older protocol, reload FERB once in that profile's extensions page. No browser restart is
required by this repair. Do not start another personal task merely to validate it.

| Check | Expected evidence | Owner verdict |
|---|---|---|
| Disposable background task with a failed browser route and a recovery attempt | Planner and replacement worker retain background/profile direction; no app activation or foreground shell fallback; owner keeps typing normally | Pending |
| Idle managed extension with stale protocol but matching displayed version | One reload at most per version/protocol pair; reconnects on the installed protocol; pairing and kept-open tabs survive; no visible window or Space switch | Pending |

Offline prompt delivery tests cannot prove that a real model follows the shared instructions. Offline JavaScript
tests cannot prove Chrome's reload/cache behavior or an undisturbed desktop. Both need the separate checks above.

## 0.24.0 — installation observed; broader live acceptance pending

On 2026-10-05, installed source files matched the 0.24.0 checkout (hash `073d78f39b83ecd1`). The Discord task
`7263596687c04fb9b223e8fb6e881b8d` encountered extension 0.11.0 reporting protocol 5 while its on-disk bundle had
protocol 6. The owner reported repeated desktop switches; the trace contained foreground launches, a mistaken
scaler-profile launch, and repeated `Google Chrome activate` commands after recovery authorized foreground input.
This was a failed background task, not acceptance of the earlier pointer fix across all fallback paths.
The remaining 0.24.0 acceptance scenarios below are still pending. Record one requested check at a time.

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

## Local architecture walkthrough — 2026-10-05

- Open `docs/walkthrough/index.html` (or `open docs/walkthrough/index.html`). No server or install needed.
- Twelve example journeys cover 20 components in the updated HLD/LLD. The interactive Claude example uses
  task `2a2a77edd48f4f9793caf45f7fbcea18`; other entries explicitly distinguish illustrative paths from evidence.
- Navigation/data checks cover all 245 explained steps in both views, with source paths and component coverage
  checked. Mermaid grammar parsing passes with a Node-only text sanitizer stub; it does not establish layout.
- Browser visual inspection is pending: the in-app browser is unavailable and disposable Chrome rendering fails
  to launch here. Existing SVG exports remain labelled as historical 0.24.0. Owner check: readable on your screen,
  Next/Back and view switching keep your place, and source/component explanations are useful.
