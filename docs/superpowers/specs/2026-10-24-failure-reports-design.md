# Failure reports to the developer — design (2026-10-24)

Status: decisions made by the owner on 2026-10-24 (below); this written design awaits the owner's review before an
implementation plan. Decision id: L35. One input is still the owner's: the feedback address (a new inbox the owner
creates); it is one setting in the release build. Built with the L34 phases (it uses FERB.app notifications, Phase D).

## Why

The owner's builder list (2026-10-24): "listen to every error in their tasks and add option there to send this as
feedback" and "whenever a task fails, there's a system that sends everything for that time and prompts user that if
this report can be sent to the developer". The audit found none of it exists: a failed task leaves its record on the
user's Mac only (67 of 245 tasks failed in 30 days on the owner's Mac; none has a verdict), `ferb verdict` is CLI-only,
there is no diagnostic bundle, no telemetry, no report path (`docs/OWNER_ACCEPTANCE.md`, `ferb/cli/admin.py`).

## Owner decisions (2026-10-24)
- **Contents:** "Full record, user previews": the task's whole record with secrets removed; the user sees exactly what
  will be sent and can untick parts before sending.
- **Channel:** "User's own Mail app": FERB opens a new email in the user's Mail app, addressed to the developer, with the
  report attached; the user presses Send.
- **When:** after each failure, a notification ("a pop-up/notification that — hey everything's ready about the task
  failed. can i send this to the developer? that helps us improve and cater to your need better").
- **To:** "A new feedback address" — a dedicated inbox the owner creates.

## What a user experiences
1. A task ends `failed` (also `interrupted` and a planning failure that ended the turn). FERB builds the report at once,
   locally.
2. A FERB notification (and the same line under the failed answer in the notch): "Everything about the task that failed
   is ready. Can I send it to the developer? It helps us improve FERB and fit your needs better." Buttons: **Review &
   Send**, **Not now**. Settings → Notifications can turn these off; "Not now" leaves the button on the answer.
3. **Review & Send** opens a review sheet in the FERB window: the report's parts with sizes, each with a checkbox and a
   preview:
   - Summary (always included): FERB and macOS versions, AI service and model, the task's status, the failure reason
     (L11 wording), timings.
   - The request and FERB's answer.
   - Steps: the task's tool calls, results and errors (the trace).
   - Page and app text FERB read during the task.
   - Screenshots FERB took (already masked).
   - Settings that matter (redacted config: sections and switches, never keys).
   - Recent FERB logs (daemon error log tail, last 200 lines, redacted).
   Unticking a part removes it from the file. A short note field ("What were you trying to do?") is optional.
4. **Send** writes `FERB-report-<date>-<task>.zip` to `~/.ferb/reports/` and opens a Mail compose window
   (`NSSharingService(named: .composeEmail)`) addressed to the feedback address, subject "FERB report: <failure reason,
   short>", body the summary, the zip attached. The user presses Send in Mail. If no Mail account is set up, FERB shows
   the file in Finder and the address to send it to.
5. The report is kept locally for 30 days (the span retention) and listed in Settings → Advanced → Reports; nothing is
   uploaded by FERB itself.

## Design
- **Bundle builder (daemon):** a `reports.build` RPC takes a task id and the chosen parts and writes the zip. Sources:
  the task's trace (`_trace`/`SpanStore.outcome`, `ferb/daemon/main.py` ~4030), episode card, the failure reason
  (`worker.reason`), versions (`ferb.__version__`, FERB.app's Info.plist, extension version), the doctor checks as data
  (they print only today; `ferb/cli/setup.py`), a redacted config view, the daemon error-log tail, and the evidence
  images the trace cites. A `reports.preview` RPC returns each part's size and a text preview. Everything stored is
  already secret-redacted at capture (rule 14); the builder runs the redactor over every text part again before
  writing (defense in depth), and screenshots come from the evidence store, which is masked.
- **Trigger:** the daemon's task-end path (`supervisor.py` task outcome, `task.finished`) publishes `report.ready`
  `{task_id, reason}` for failed, interrupted and turn-ending planning failures; the app posts the notification.
- **App:** notification category `report` with actions Review & Send / Not now; the review sheet in the FERB window
  (L34 Phase E); the compose step via `NSSharingService`; a "Report this" button on failed answers in the notch.
- **Feedback address:** one constant set at build time (`FEEDBACK_ADDRESS`, in the app's Info.plist and the daemon's
  release config); a checkout build reads `[feedback] address` from config.toml and shows "No feedback address
  configured" when it's empty.
- **Privacy (rule 15):** reports are the first FERB data meant to leave a user's Mac to someone other than the AI
  service. Nothing leaves without the user's preview and their own Send; the README and the setup's Welcome step say
  so.

## Not in this design
- Automatic sending, telemetry, crash reporting without the user's Send.
- A web endpoint or GitHub issues.

## Testing
- Python: the bundle contains exactly the chosen parts; every text part passes the redactor (a planted secret never
  appears in the zip); versions and reason present; unticked parts absent; `report.ready` published for failed,
  interrupted and turn-ending planning failures, not for done or cancelled.
- Swift: the notification category and actions; the review sheet's checkboxes map to parts; the compose step gets the
  address, subject and attachment.
- Live, owner-observed: force a failure, review, send to the feedback inbox, open the zip.
- L35 in `docs/0.27_DECISIONS.toml`; README (privacy and reports), ARCHITECTURE.md, CHANGELOG.md.
