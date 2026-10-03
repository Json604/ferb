# 0.23.2 — preserve app switches during browser waits

## Observed failure

The owner reported one unexpected switch to VS Code during the search task on installed 0.23.0. Task
`1323faccf505402683d5755b4fe39d6b` used `ferb_browser.open` and read controls in FERB's Chrome `Profile 22`.
No native pointer or app-activation tool call was recorded in that task. Local trail/journal timestamps on
2026-10-03 (Asia/Kolkata) identify the switch:

- 13:06:26.998: Brave becomes the front app.
- 13:06:27.317: FERB's second Chrome search tab reports its loaded page.
- 13:06:27.330: `browser.focus_returned` records Brave → VS Code.
- 13:06:27.511: the focus watcher sees VS Code.
- 13:06:30.035: the focus watcher sees Brave again.

`_TabDriver._open_in` captured the front app before awaiting the extension. Its `finally` called
`Daemon._put_owner_back`, which activated that old app whenever any browser was now in front. It did not know
who caused the switch, and even treated Brave as focus stolen by an operation on Chrome. The same callback
ran after profile wakes, including failures. This establishes the source of this reported switch; it does not
attribute unrelated desktop movements to this bug. Private task content remains outside Git.

## Repair

Remove that callback and its daemon wiring from both browser drivers. Browser commands leave application focus
to the owner throughout asynchronous opens and wakes. Merely comparing the target browser would still be wrong
when the owner switches to another tab/profile of that same browser while FERB waits.

The actual background mechanisms remain: background/windowless profile launch, inactive task tabs and the
extension's minimized task window. No visible wake fallback or corrective app activation is added. This change
does not alter native input or claim that every macOS/Chromium window operation is incapable of taking focus.
Any remaining platform focus change needs its own observed diagnosis rather than a guessed restoration.

## Verification and rollout

Sixteen offline regression cases execute both daemon-wired browser drivers, with owner switches to Brave or
Chrome during opens/wakes that succeed or fail. All sixteen failed against the old code by reactivating VS Code;
all pass after the removal. The focused extension/background/identity suite passed 59 tests in 36.43 seconds.
These tests use fake app APIs and extension responses; they do not operate the owner's desktop.
The full offline suite passed 556 tests, 52 deselected, in 115.18 seconds. Core mypy and whitespace checks passed.

The running daemon remains 0.23.0. Extension 0.10.0 is unchanged. Installation and owner-observed acceptance
are pending. After an authorized update, repeat a search while the owner switches apps and verify that FERB
does not pull them back; pause for the owner's verdict before further live actions. The branch push is
authorized; no merge or daemon restart accompanies it.
