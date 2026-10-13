# Native daemon connection

Goal: replace example work with the installed FERB daemon while preserving the owner's current notch UI edits.

Architecture: a Foundation/Darwin newline JSON-RPC transport authenticates as the owner CLI. A main-actor coordinator attaches the conversation, observes the event stream, reconciles status on reconnect, and submits chat on separate sockets. The existing session model keeps presentation independent of task execution. Backend task IDs, not preview timers, drive Stop and completion. Only conversation/task IDs persist in UI preferences.

Contract: `auth.hello`, `chat.attach`, `chat.status`, `chat.send`, `logs.subscribe`, `task.stop`, `chat.answer`, `chat.approve`, and `trace.get`. Attachment support uses explicit daemon `input_paths` support; never silently drops files. Secret approval kinds use a separate secure field, cleared on send/dismiss, never the composer, saved history, or activity. Only daemon artifact paths become file actions. Voice remains clearly marked microphone-off until a real voice transport is implemented.

Constraints: preserve all pre-existing dirty visual changes, notch geometry, spring clock, pin/Spaces behavior, focus, sound and finite completion glow. Background events never call the focus/interaction callbacks. No install or daemon restart, and no live model call during checks. No automatic mutation retries after a socket disconnect; reconcile first.

Tasks: (1) socket contract tests against a temporary local server, then transport; (2) session regressions for backend IDs, actual outcomes and stale events, then coordinator; (3) real prompt/result surfaces and attachment integration; (4) Swift tests and app build. Owner evaluates visuals. Shipping signing/notarization remains separate.


## Completed implementation and self-review

Implemented the socket client, live session coordinator, real outcome/artifact rendering, secure scoped approval UI, exact option ordinals, attachment capability negotiation and reconnect recovery. No preview task timer or generated example document remains on the submission/result path. Voice stays explicitly microphone-off and preserves the owner's draft.

Read-only review found and fixed paste provenance loss after edits/reordering and forged delimiters. The final implementation retains a conservative edited-after-paste flag and requires the owner's explicit whole-draft authority/source choice when matching is no longer reliable. Clipboard escape controls are visibly encoded in reference payloads. Review also verified task identity disclosure, exact option selection, no mutation replay on reconnect, and no background focus request.

Validation: all 44 Swift tests pass; app bundle build/ad-hoc signature and `git diff --check` pass. Checks use fake RPCs and a temporary local Unix socket server; no model call, task submission or installed daemon mutation. Root controller handles installed-backend contract check and authorized app relaunch. Owner evaluates live UI/OTP behavior. Existing dirty owner changes remain uncommitted on `native-quiet-notch`.

## Controller installation check

The owner-authorized backend repair/API build (6c354bc) is installed and all four macOS grants verify. The actual native Swift transport authenticated to the daemon, received input_paths capability support and read task/prompt counts without submitting a model task. Root relaunched the built notch app in the background (PID56549) and verified an open Unix socket. Owner live task/OTP/visual acceptance remains pending; microphone remains off.
