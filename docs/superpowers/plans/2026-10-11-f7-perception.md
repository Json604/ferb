# F7 — background perception

## Goal

Observe system activity without input or focus changes; retain redacted typed text per field and clipboard text;
group observations by app/window; batch them for Jev; record notification triage in shadow. All observations remain
data, never authority for a task. Owner instruction of 2026-10-11 authorizes implementation now and postpones tests
and reviews until the combined F5–F10 check. Installation and owner-observed acceptance remain pending.

## Architecture

Keep `Trail.record` as the only raw-observation persistence path. New passive watchers use the existing Sense
lifecycle, exclusions, redactor and event bus. Native Accessibility observers subscribe to running applications and
their windows; unsupported notifications are recorded as capability facts. Input uses the existing listen-only
event tap, holds field text for the decided 0.5 seconds and drops it on Secure Input, secure/unknown fields,
excluded apps or masked values. Clipboard markers are checked before text is read; exact change counts from
`mac.clipboard` identify FERB writes. Neither watcher sends native input.

Notification Center is read through a read-only SQLite connection under FDA; missing permissions/schema changes
are observable failures, not an empty successful read. Persistent external identifiers prevent replay on restart.
Jev batches reference trail IDs, split by app and token budget, retain oversize events locally with labeled wire
excerpts, and flush on age/interrupt. Per-notification context contains that conversation's history, owner-quoted
rules and told receipts. Scope mutes apply before Jev; triage stays shadow regardless of enable configuration.
Software inspection records code-signing, assessment, provenance, entitlements, TCC and persistence facts without
launching the app or classifying intent. Persona setup shows actual app-use evidence for owner confirmation/edit.

## Spec

- D4: workspace/application lifecycle, Accessibility notifications, existing filesystem/browser/idle sources;
  no log stream or continuous screenshot/OCR. A now-playing unsupported status is explicit pending platform probe.
- D7/D11/P4/P18/CQ3: per-field text and clipboard, redaction before publish/store/model, 7-day raw content retention;
  marked FERB input stays FERB's. Secure Input is checked at capture and throughout the hold window.
- D10/P1/P11/P14: ~10k-token batches; configurable 60-second oldest-event trigger; immediate notification/call trigger;
  one notification decision with conversation-scoped context; no silent omission, no automatic notification action.
- P12: installed software facts and subsequent privacy/persistence changes; agent interpretation stays shadow.
- P28: local setup usage evidence, confirmation/edit once; only the owner's confirmed words are owner-stated.

## Global constraints

No new dependencies, retention values or owner-visible limits. No real model call, clipboard read/write, desktop
observation, installation or restart during implementation. Use dependency-injected offline sources. Two app
prototype replays describe WhatsApp and VS Code Accessibility shapes; they are synthetic, not invented live captures.
Actual two-app feasibility, macOS 26 clipboard alerts, DND and now-playing access require owner acceptance.

Controller ruling (2026-10-11): use P1's documented 60-second example as the configurable scheduler cadence,
`sense.batch_age_s`, exposed in `sense.status.perception`. It flushes earlier without dropping data or limiting task
runtime, so this is implementation timing rather than a resource quota. D10 explicitly supplies the 10,000-token
batch target and 32,000-token Jev context guard; token counts are tokenizer estimates.

## Review focus

Secret data must never cross a queue or callback before redaction. Check restart deduplication, pause clears pending
input, field switches and FERB marks, short sensitive retention including derived activity copies, wire excerpts,
notification mute provenance, and no shadow-to-action path. Integration must remain inert when watchers are disabled.

## Tasks

1. Add regressions for capture guards, per-field grouping, FERB attribution, concealed clipboard, two-app AX
   events and notification SQLite schema/restarts. Implement `sense/capture.py` and passive watcher modules.
2. Add batching/rules/retention regressions; implement `0032_f7_perception.sql`, `sense/batches.py`,
   `sense/notifications.py` and the Jev token safety guard. Preserve full raw events; decisions reference IDs.
3. Add software/persona regressions; implement fact inspection and local confirmed setup preference capture.
4. Wire the modules through `sense/perception.py`, small daemon lifecycle hooks, the existing owner-input tap,
   setup and config. Update the registry and current architecture/acceptance docs. Compile and diff checks only;
   controller runs combined pytest/mypy and fresh review after integration. Commit citing decision IDs.

## Self-review

Keep production native adapters separate from pure capture/batching logic. Tests never touch personal clipboard,
Notification Center, keychain, or apps. Offline correctness is separate from platform feasibility and owner verdict.
